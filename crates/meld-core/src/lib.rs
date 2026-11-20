//! Meld core: splitter, syntax layer, binding resolver, scheduler.
pub mod binding;
pub mod compose;
pub mod error;
pub mod region_end_spec;
pub mod scheduler;
pub mod scanners;
pub mod source_map;
pub mod splitter;
pub mod syntax;

pub use error::{MeldError, Result};
