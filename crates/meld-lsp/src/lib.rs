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
pub fn project_root() -> PathBuf {
    if let Ok(root) = std::env::var("MELD_ROOT") {
        return PathBuf::from(root);
    }
    std::env::current_dir()
        .ok()
        .and_then(|c| {
            c.ancestors()
                .find(|p| p.join("project.md").is_file() || p.join("Cargo.toml").is_file())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("."))
}
