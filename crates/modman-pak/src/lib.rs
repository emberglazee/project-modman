//! modman-pak — PAK file operations for Project Wingman.
//!
//! Wraps repak with PW-specific convenience methods for
//! reading, unpacking, and creating .pak files.

use std::path::Path;

pub use repak::{Compression, Version, VersionMajor};

/// Errors from PAK operations
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("repak error: {0}")]
    Repak(#[from] repak::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// An opened PAK archive for reading
pub struct PakArchive {
    reader: repak::PakReader,
    path: String,
}

/// Metadata about a PAK archive
#[derive(Debug)]
pub struct PakInfo {
    pub path: String,
    pub version: Version,
    pub version_major: VersionMajor,
    pub mount_point: String,
    pub encrypted_index: bool,
    pub encryption_guid: Option<u128>,
    pub path_hash_seed: Option<u64>,
    pub file_count: usize,
    pub compression: Vec<Compression>,
}

impl PakArchive {
    /// Open a .pak file for reading
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let file = std::fs::File::open(path)?;
        let reader = repak::PakBuilder::new().reader(&mut std::io::BufReader::new(file))?;
        Ok(Self {
            reader,
            path: path.to_string_lossy().to_string(),
        })
    }

    /// Get metadata about the PAK archive
    pub fn info(&self) -> PakInfo {
        PakInfo {
            path: self.path.clone(),
            version: self.reader.version(),
            version_major: self.reader.version().version_major(),
            mount_point: self.reader.mount_point().to_string(),
            encrypted_index: self.reader.encrypted_index(),
            encryption_guid: self.reader.encryption_guid(),
            path_hash_seed: self.reader.path_hash_seed(),
            file_count: self.reader.files().len(),
            compression: self.reader.used_compression(),
        }
    }

    /// List all files in the PAK archive
    pub fn files(&self) -> Vec<String> {
        self.reader.files()
    }

    /// List files with mount point prefix stripped
    pub fn files_stripped(&self, strip_prefix: &str) -> Vec<String> {
        let mount = std::path::PathBuf::from(self.reader.mount_point());
        let prefix = std::path::Path::new(strip_prefix);
        let full_paths: Vec<_> = self
            .reader
            .files()
            .into_iter()
            .map(|f| mount.join(&f))
            .collect();

        full_paths
            .iter()
            .filter_map(|f| f.strip_prefix(prefix).ok())
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    /// Unpack all files from the PAK archive to an output directory
    pub fn unpack(
        &self,
        output_dir: impl AsRef<Path>,
        strip_prefix: &str,
        _verbose: bool,
    ) -> Result<(), Error> {
        let output_dir = output_dir.as_ref();
        let mount = std::path::PathBuf::from(self.reader.mount_point());
        let prefix = std::path::Path::new(strip_prefix);

        for entry_path in self.reader.files() {
            let full_path = mount.join(&entry_path);
            let relative = full_path.strip_prefix(prefix).unwrap_or(&full_path);
            let out_path = output_dir.join(relative);

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let mut pak_file = std::fs::File::open(&self.path)?;
            let mut out_file = std::fs::File::create(&out_path)?;
            self.reader
                .read_file(&entry_path, &mut pak_file, &mut out_file)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_version_returns_string() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
