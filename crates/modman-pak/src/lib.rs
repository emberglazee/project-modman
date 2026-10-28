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
    #[error("unsafe entry path escapes destination: {0}")]
    UnsafePath(String),
}

/// Normalize a pak or archive entry name to a relative path, or `None` when it
/// would escape the destination.
///
/// Both separators are treated as separators because Windows-style entry names
/// appear in archives on every platform. Absolute paths, drive letters (and
/// NTFS alternate streams), and `..` components are refused. The returned path
/// has no empty or `.` components, so `dest.join(..)` cannot climb out.
pub fn safe_rel(name: &str) -> Option<std::path::PathBuf> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.contains(':') {
        return None;
    }
    let mut out = std::path::PathBuf::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p => out.push(p),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Join `name` onto `dest`, refusing anything `safe_rel` rejects.
pub fn safe_join(dest: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    safe_rel(name).map(|rel| dest.join(rel))
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
            let relative = if strip_prefix.is_empty() {
                std::path::Path::new(&entry_path)
            } else {
                full_path.strip_prefix(prefix).unwrap_or(&full_path)
            };
            let Some(out_path) = safe_join(output_dir, &relative.to_string_lossy()) else {
                return Err(Error::UnsafePath(entry_path.clone()));
            };

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

    /// Extract a single entry (raw pak path, e.g. "ProjectWingman/Content/...") to an
    /// output file. Much cheaper than unpacking the whole archive.
    pub fn extract_entry(&self, entry_path: &str, out_path: impl AsRef<Path>) -> Result<(), Error> {
        let out_path = out_path.as_ref();
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let entry = entry_path.to_string();
        let mut pak_file = std::fs::File::open(&self.path)?;
        let mut out_file = std::fs::File::create(out_path)?;
        self.reader
            .read_file(&entry, &mut pak_file, &mut out_file)?;
        Ok(())
    }

    /// Read a single entry's bytes into memory (raw pak path).
    pub fn read_entry(&self, entry_path: &str) -> Result<Vec<u8>, Error> {
        let mut pak_file = std::fs::File::open(&self.path)?;
        let mut buf = Vec::new();
        self.reader.read_file(entry_path, &mut pak_file, &mut buf)?;
        Ok(buf)
    }
}

/// Create a .pak file from a directory. Entries are stored uncompressed: the
/// pak reader has no decompression support compiled in (see Cargo.toml), and a
/// compressed entry would export a pak this build cannot read back.
pub fn pack(
    input_dir: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    version: Version,
    mount_point: String,
) -> Result<(), Error> {
    let input_dir = input_dir.as_ref();
    let output_path = output_path.as_ref();

    let mut paths = Vec::new();
    collect_files(&mut paths, input_dir)?;
    paths.sort();

    let mut pak = repak::PakBuilder::new().writer(
        std::io::BufWriter::new(std::fs::File::create(output_path)?),
        version,
        mount_point,
        None,
    );

    for path in &paths {
        let relative = path.strip_prefix(input_dir).expect("path under input dir");
        let data = std::fs::read(path)?;
        let path_str = relative.to_string_lossy().replace('\\', "/");
        pak.write_file(&path_str, false, data)?;
    }

    pak.write_index()?;
    Ok(())
}

fn collect_files(paths: &mut Vec<std::path::PathBuf>, dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(paths, &path)?;
        } else {
            paths.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_returns_string() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }

    #[test]
    fn safe_rel_refuses_escapes() {
        for bad in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "C:/x",
            "C:\\x",
            "\\\\server\\share\\x",
            "a\\..\\..\\x",
            "",
            ".",
        ] {
            assert!(safe_rel(bad).is_none(), "{bad:?} must be refused");
        }
        assert_eq!(
            safe_rel("ProjectWingman/Content/x.txt").unwrap(),
            std::path::Path::new("ProjectWingman/Content/x.txt")
        );
        assert_eq!(
            safe_rel("a\\b\\c.txt").unwrap(),
            std::path::Path::new("a/b/c.txt")
        );
    }

    #[test]
    fn unpack_refuses_an_escaping_entry() {
        let root = std::env::temp_dir().join(format!("modman-pak-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let pak_path = root.join("hostile.pak");
        {
            let mut pak = repak::PakBuilder::new().writer(
                std::io::BufWriter::new(std::fs::File::create(&pak_path).unwrap()),
                repak::Version::V3,
                "../../../".to_string(),
                None,
            );
            pak.write_file("../../evil.txt", false, b"payload").unwrap();
            pak.write_index().unwrap();
        }
        let archive = PakArchive::open(&pak_path).unwrap();
        let out = root.join("a/b/out");
        std::fs::create_dir_all(&out).unwrap();
        let err = archive.unpack(&out, "", false).unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)));
        assert!(!root.join("a/evil.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
