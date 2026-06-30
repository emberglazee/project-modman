//! modman-uasset — UE4 .uasset/.uexp binary parser for Project Wingman.
//!
//! Standalone crate with no internal modman dependencies.
//! Handles UE4.24 (v1.0.4d) and UE4.27 (v2.1.1A) uasset formats.

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
