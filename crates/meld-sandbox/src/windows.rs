//! Windows backend (cfg-gated; COMPILE-ONLY on Linux).
//!
//! Job Object with `KILL_ON_JOB_CLOSE` for process-tree teardown and limits. A
//! child is assigned to the job immediately after spawn; if assignment fails
//! the child is killed and the spawn call fails closed. AppContainer profile
//! and DACL helpers remain for future use, but this backend cannot apply an
//! AppContainer token to a `std::process::Command`, so it does not claim
//! AppContainer enforcement and policies requiring it fail closed. There is no
//! Windows runtime verification on the Linux development host.

use crate::backend::{
    require_enforcement, BackendFeatures, BackendKind, BackendProbe, SandboxBackend,
};
use crate::error::{Result, SandboxError};
use crate::io::IoPolicy;
use crate::policy::SandboxPolicy;
use crate::process::{CommandSpec, SpawnedProcess};
use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::ptr;
use std::sync::Arc;
use windows_sys::core::{BOOL, HRESULT};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile,
};
use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, PSID, SID_AND_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_JOB_TIME,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};

fn to_wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn last_error(operation: &str) -> SandboxError {
    SandboxError::Denied(format!(
        "{operation} failed: {}",
        std::io::Error::last_os_error()
    ))
}

/// Owns an AppContainer profile SID and removes the profile on drop.
pub struct AppContainer {
    name: Vec<u16>,
    sid: PSID,
}

// SAFETY: the SID is owned by this struct and is only used through the Windows
// APIs while the struct is alive; it is never shared as a borrowed pointer.
unsafe impl Send for AppContainer {}
unsafe impl Sync for AppContainer {}

impl AppContainer {
    pub fn create(name: &str) -> Result<Self> {
        let wide = to_wide(OsStr::new(name));
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: all pointers are either valid null-terminated strings or
        // out-parameters; capability array is explicitly empty.
        let hr: HRESULT = unsafe {
            CreateAppContainerProfile(
                wide.as_ptr(),
                wide.as_ptr(),
                to_wide(OsStr::new("Meld sandbox worker")).as_ptr(),
                ptr::null::<SID_AND_ATTRIBUTES>(),
                0,
                &mut sid,
            )
        };
        if hr < 0 || sid.is_null() {
            return Err(SandboxError::Denied(format!(
                "CreateAppContainerProfile failed: HRESULT {hr:#x}"
            )));
        }
        Ok(Self { name: wide, sid })
    }

    /// Add a DACL entry granting this AppContainer access to a safe root.
    pub fn allow_path(&self, path: &Path) -> Result<()> {
        let path_wide = to_wide(path.as_os_str());
        let explicit = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: 0,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: self.sid as *mut u16,
            },
        };
        let mut new_acl: *mut ACL = ptr::null_mut();
        // SAFETY: one valid explicit entry, no old ACL, output pointer owned by us.
        let status = unsafe { SetEntriesInAclW(1, &explicit, ptr::null(), &mut new_acl) };
        if status != 0 || new_acl.is_null() {
            return Err(SandboxError::Denied(format!(
                "SetEntriesInAclW failed: {status}"
            )));
        }
        // SAFETY: path_wide is null-terminated; new_acl is released below.
        let result = unsafe {
            SetNamedSecurityInfoW(
                path_wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                new_acl,
                ptr::null_mut(),
            )
        };
        // SAFETY: LocalFree releases the ACL allocated by SetEntriesInAclW.
        unsafe { LocalFree(new_acl as *mut c_void) };
        if result != 0 {
            return Err(SandboxError::Denied(format!(
                "SetNamedSecurityInfoW failed: {result}"
            )));
        }
        Ok(())
    }

    pub fn delete(&mut self) {
        if !self.sid.is_null() {
            // SAFETY: FreeSid releases a SID allocated by CreateAppContainerProfile.
            unsafe { windows_sys::Win32::Security::FreeSid(self.sid) };
            self.sid = ptr::null_mut();
        }
        // SAFETY: name is a valid null-terminated profile name.
        let _hr = unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
    }
}

impl Drop for AppContainer {
    fn drop(&mut self) {
        self.delete();
    }
}

/// Job Object with kill-on-close. Dropping it terminates the whole tree.
pub struct JobObject(HANDLE);

// SAFETY: the kernel handle is owned by this struct. It is safe to move between
// threads; all mutations happen through the owning `&mut self` or `&self`.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    pub fn create(policy: &SandboxPolicy) -> Result<Self> {
        // SAFETY: default security attributes; named jobs are not used.
        let handle: HANDLE = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateJobObjectW"));
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        let limits = &policy.limits;
        let mut flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(mem) = limits.memory_bytes {
            info.JobMemoryLimit = mem as usize;
            info.ProcessMemoryLimit = mem as usize;
            flags |= JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        }
        if let Some(proc) = limits.processes {
            info.BasicLimitInformation.ActiveProcessLimit = proc as u32;
            flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        }
        if let Some(cpu) = limits.cpu_time_ms {
            // Per-job user time is in 100ns units.
            info.BasicLimitInformation.PerJobUserTimeLimit = (cpu.saturating_mul(10_000)) as i64;
            flags |= JOB_OBJECT_LIMIT_JOB_TIME;
        }
        info.BasicLimitInformation.LimitFlags = flags;
        // SAFETY: info struct and length match the information class.
        let ok: BOOL = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            // SAFETY: handle was created above.
            unsafe { CloseHandle(handle) };
            return Err(last_error("SetInformationJobObject"));
        }
        Ok(Self(handle))
    }

    pub fn assign(&self, child: &std::process::Child) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: as_raw_handle is the child's process handle, valid while `child` lives.
        let ok: BOOL = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            return Err(last_error("AssignProcessToJobObject"));
        }
        Ok(())
    }

    pub fn terminate(&self) {
        // SAFETY: handle is owned and open for the lifetime of this struct.
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        self.terminate();
        // SAFETY: handle is owned by this struct.
        unsafe { CloseHandle(self.0) };
    }
}

pub struct WindowsBackend;

impl WindowsBackend {
    pub fn probe(&self) -> BackendProbe {
        BackendProbe {
            kind: BackendKind::WindowsJobObject,
            available: cfg!(target_os = "windows"),
            features: BackendFeatures {
                seatbelt: false,
                job_object: cfg!(target_os = "windows"),
                // JUDGMENT CALL 40: creating a profile and editing DACLs does
                // not put the child in an AppContainer. Claiming this feature
                // let an ordinary `Command::spawn` satisfy policies that need
                // network denial, fs isolation or syscall filtering. Report
                // the truth so `require_enforcement` fails closed instead.
                appcontainer: false,
                rlimits: false,
                process_group_kill: false,
                ..Default::default()
            },
            notes: vec!["Windows backend is compile-only on Linux".to_string()],
        }
    }
}

impl SandboxBackend for WindowsBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::WindowsJobObject
    }

    fn probe(&self) -> BackendProbe {
        WindowsBackend::probe(self)
    }

    fn spawn(&self, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<SpawnedProcess> {
        require_enforcement(self, policy)?;
        let job = Arc::new(JobObject::create(policy)?);
        let mut command: Command = spec.to_command();
        let mut child = command.spawn()?;
        if let Err(e) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Denied("stdout pipe missing".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::Denied("stderr pipe missing".to_string()))?;
        let hook_job = Arc::clone(&job);
        Ok(
            SpawnedProcess::new(child, stdout, stderr, IoPolicy::from_limits(&policy.limits))?
                .with_kill_hook(move || {
                    hook_job.terminate();
                    Ok(())
                })
                .with_keep_alive(job),
        )
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn job_object_is_kill_on_close() {
        let job = JobObject::create(&SandboxPolicy::deny_all("test")).unwrap();
        assert!(!job.0.is_null());
    }

    #[test]
    fn app_container_profile_lifecycle() {
        let mut container = AppContainer::create("meld-sandbox-test-profile").unwrap();
        container.delete();
    }

    #[test]
    #[ignore = "runtime test only on Windows"]
    fn windows_runtime_spawn_fails_closed_without_appcontainer() {
        let spec = CommandSpec::new("cmd.exe", ["/c", "echo ok"]);
        let error = match WindowsBackend.spawn(&spec, &SandboxPolicy::deny_all("runtime")) {
            Ok(_) => panic!("deny-all policy spawned without AppContainer enforcement"),
            Err(error) => error,
        };
        assert!(
            matches!(error, SandboxError::Unsupported(_)),
            "unexpected error: {error}"
        );
    }
}
