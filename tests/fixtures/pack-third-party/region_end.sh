;; Pack-owned region-end scanner stub for POSIX shell.
;; JUDGMENT CALL: until a real scanner is supplied, return -1 so the host uses
;; the deterministic raw-hash delimiter fallback (project.md §3.2). This is
;; data for the host, not code executed by packtool.
(func $find_region_end (param i32 i32) (result i32)
  i32.const -1)
