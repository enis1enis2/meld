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
    let canonical = |path: PathBuf| std::fs::canonicalize(&path).unwrap_or(path);
    if let Ok(root) = std::env::var("MELD_ROOT") {
        return canonical(PathBuf::from(root));
    }
    std::env::current_dir()
        .ok()
        .and_then(|c| {
            c.ancestors()
                .find(|p| p.join("project.md").is_file() || p.join("Cargo.toml").is_file())
                .map(PathBuf::from)
        })
        .map(canonical)
        .unwrap_or_else(|| canonical(PathBuf::from(".")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ROOT_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn meld_root_is_canonicalized_like_the_cli() {
        let _guard = ROOT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::current_dir().unwrap().join("..");
        let expected = std::fs::canonicalize(&root).unwrap();
        let previous = std::env::var_os("MELD_ROOT");
        std::env::set_var("MELD_ROOT", &root);
        assert_eq!(project_root(), expected);
        match previous {
            Some(value) => std::env::set_var("MELD_ROOT", value),
            None => std::env::remove_var("MELD_ROOT"),
        }
    }
}
