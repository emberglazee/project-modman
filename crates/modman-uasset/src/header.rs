use crate::{read_fstring, Error};
use std::io::{Read, Seek, SeekFrom};

pub const PACKAGE_FILE_TAG: u32 = 0x9E2A83C1;

/// UE4 package header for cooked assets
#[derive(Debug, Clone)]
pub struct PackageHeader {
    pub magic: u32,
    pub legacy_file_version: i32,
    pub file_version_ue4: u32,
    pub package_flags: u32,
    pub name_count: i32,
    pub name_offset: i32,
    pub export_count: i32,
    pub export_offset: i32,
    pub import_count: i32,
    pub import_offset: i32,
    pub folder_name: Option<String>,
    pub package_guid: [u8; 16],
}

impl PackageHeader {
    /// Read the package header from a cooked UE4 uasset file
    pub fn read<R: Read + Seek>(reader: &mut R) -> Result<Self, Error> {
        reader.seek(SeekFrom::Start(0))?;
        let mut buf4 = [0u8; 4];

        // Magic number
        reader.read_exact(&mut buf4)?;
        let magic = u32::from_le_bytes(buf4);
        if magic != PACKAGE_FILE_TAG {
            return Err(Error::InvalidMagic(magic));
        }

        // Legacy file version
        reader.read_exact(&mut buf4)?;
        let legacy_file_version = i32::from_le_bytes(buf4);

        // LegacyUE3Version (only if legacy_file_version != -4)
        if legacy_file_version != -4 {
            reader.read_exact(&mut buf4)?;
        }

        // FileVersionUE4 (ObjectVersion)
        reader.read_exact(&mut buf4)?;
        let file_version_ue4 = u32::from_le_bytes(buf4);

        // FileVersionUE5 (only if legacy_file_version <= -8)
        if legacy_file_version <= -8 {
            reader.read_exact(&mut buf4)?;
        }

        // FileVersionLicenseeUE
        reader.read_exact(&mut buf4)?;

        // Custom version container (if legacy_file_version <= -2)
        if legacy_file_version <= -2 {
            reader.read_exact(&mut buf4)?; // count (typically 0 for cooked)
            let count = i32::from_le_bytes(buf4);
            if count > 0 {
                // For non-zero counts, read entries (guids + versions + names)
                // But cooked assets typically have 0 here
                for _ in 0..count {
                    let mut guid = [0u8; 16];
                    reader.read_exact(&mut guid)?;
                    reader.read_exact(&mut buf4)?; // version
                    read_fstring(reader)?; // name
                }
            }
        }

        // SectionSixOffset
        reader.read_exact(&mut buf4)?;

        // FolderName
        let folder_name = read_fstring(reader)?;

        // PackageFlags
        reader.read_exact(&mut buf4)?;
        let package_flags = u32::from_le_bytes(buf4);

        // NameCount, NameOffset
        reader.read_exact(&mut buf4)?;
        let name_count = i32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?;
        let name_offset = i32::from_le_bytes(buf4);

        // GatherableTextData (if file_version_ue4 >= 342)
        if file_version_ue4 >= 342 {
            reader.read_exact(&mut buf4)?; // count
            reader.read_exact(&mut buf4)?; // offset
        } else {
            // Even for unversioned (fv_ue4=0), cooked assets may have these
            // Read them anyway — if count=0 the offset doesn't matter
            reader.read_exact(&mut buf4)?; // count (typically 0)
            reader.read_exact(&mut buf4)?; // offset
        }

        // ExportCount, ExportOffset
        reader.read_exact(&mut buf4)?;
        let export_count = i32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?;
        let export_offset = i32::from_le_bytes(buf4);

        // ImportCount, ImportOffset
        reader.read_exact(&mut buf4)?;
        let import_count = i32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?;
        let import_offset = i32::from_le_bytes(buf4);

        // DependsOffset
        reader.read_exact(&mut buf4)?;

        // SoftPackageReferences (if file_version_ue4 >= 276)
        if file_version_ue4 >= 276 {
            reader.read_exact(&mut buf4)?; // count
            reader.read_exact(&mut buf4)?; // offset
        } else {
            // Cooked assets may have these even when unversioned
            reader.read_exact(&mut buf4)?; // count (typically 0)
            reader.read_exact(&mut buf4)?; // offset
        }

        // SearchableNamesOffset (if file_version_ue4 >= 289)
        if file_version_ue4 >= 289 {
            reader.read_exact(&mut buf4)?;
        } else {
            reader.read_exact(&mut buf4)?; // typically 0
        }

        // ThumbnailTableOffset
        reader.read_exact(&mut buf4)?;

        // PackageGuid
        let mut package_guid = [0u8; 16];
        reader.read_exact(&mut package_guid)?;

        // We've read the essential header fields needed for navigation
        Ok(PackageHeader {
            magic,
            legacy_file_version,
            file_version_ue4,
            package_flags,
            name_count,
            name_offset,
            export_count,
            export_offset,
            import_count,
            import_offset,
            folder_name,
            package_guid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::path::PathBuf;

    #[test]
    fn read_small_uasset_header() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("tests/fixtures/test.uasset");

        if !path.exists() {
            eprintln!("Skipping test: no test.uasset fixture at {:?}", path);
            return;
        }

        let mut file = File::open(&path).unwrap();
        let header = PackageHeader::read(&mut file).unwrap();
        assert_eq!(header.magic, PACKAGE_FILE_TAG);
        assert!(header.name_count > 0, "should have names");
        assert!(header.export_count > 0, "should have exports");
        println!(
            "Parsed header: {} names, {} exports, {} imports",
            header.name_count, header.export_count, header.import_count
        );
        println!("  folder: {:?}", header.folder_name);
        println!("  flags: 0x{:08X}", header.package_flags);
    }
}
