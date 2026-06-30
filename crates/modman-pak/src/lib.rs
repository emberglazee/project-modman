//! modman-pak — PAK file operations for Project Wingman.
//!
//! Wraps repak with PW-specific convenience methods for
//! reading, unpacking, and creating .pak files.

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
