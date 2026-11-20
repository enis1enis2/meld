//! Meld error taxonomy. Every error carries enough information to map back
//! to a `.meld` source location; see spec/value-model.md §5.
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    pub byte_start: usize,
    pub byte_end: usize,
    pub line: usize,
    pub col: usize,
    pub region_id: String,
    pub lang: String,
}

#[derive(Debug, Error)]
pub enum MeldError {
    #[error("split error at byte {byte_start}: {message}")]
    Split { byte_start: usize, message: String },
    #[error("parse error in region {region_id}: {message}")]
    Parse { region_id: String, message: String },
    #[error("unknown language: {0}")]
    UnknownLang(String),
    #[error("toolchain missing: {0}")]
    ToolchainMissing(String),
    #[error("worker crashed: {0}")]
    WorkerCrashed(String),
    #[error("wasm component error: {0}")]
    Wasm(String),
    #[error("numeric overflow: {0}")]
    NumericOverflow(String),
    #[error("value is not an exact integer: {0}")]
    NotIntegral(String),
    #[error("non-finite float: {0}")]
    NonFinite(String),
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("handle expired: {0}")]
    HandleExpired(String),
    #[error("bridge cycle: {0}")]
    BridgeCycle(String),
    #[error("bridge name collision: {0}")]
    BridgeCollision(String),
    #[error("sandbox denied: {0}")]
    SandboxDenied(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MeldError>;
