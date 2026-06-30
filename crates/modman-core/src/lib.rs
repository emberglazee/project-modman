//! modman-core — Core data models, patch engine, and mod merging logic for Project Modman.
//!
//! This crate handles:
//! - Sicario .dtm JSON format parsing and serialization
//! - Patch types and value interpretation
//! - Fragment DSL parsing and execution
//! - Mod merging

pub mod fragment;
pub mod manifest;
pub mod patch;
pub mod template;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_returns_string() {
        assert!(!version().is_empty());
    }
}
