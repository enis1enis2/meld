//! Meld language server: virtual documents, source maps, region-aware completion.
//!
//! JUDGMENT CALL (project.md §7): the LSP links only bundled grammars. Pack
//! metadata decides whether a region uses static keywords or an external
//! language server, so adding a language still requires no core change.
pub mod backend;
pub mod external;
pub mod packs;
pub mod virtual_doc;

pub use backend::Backend;

use std::path::PathBuf;

/// Project root used for pack discovery and rebaseing pack-relative commands.
///
/// Canonicalized exactly like the CLI so policy paths, pack rebasing and
/// `file:` URIs do not depend on a caller's `..` segments or symlinks.
pub fn project_root() -> PathBuf {
    meld_lib::workspace_root(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ROOT_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn meld_root_is_canonicalized_like_the_cli() {
        let _guard = ROOT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("meld-lsp-root-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("project.md"), "# test\n").unwrap();
        let expected = std::fs::canonicalize(&root).unwrap();
        let previous = std::env::var_os("MELD_ROOT");
        // Include a `..` segment to verify the shared resolver normalizes and
        // canonicalizes MELD_ROOT instead of returning the raw value.
        std::env::set_var("MELD_ROOT", root.join("sub/.."));
        let actual = project_root();
        match previous {
            Some(value) => std::env::set_var("MELD_ROOT", value),
            None => std::env::remove_var("MELD_ROOT"),
        }
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(actual, expected);
    }
}
