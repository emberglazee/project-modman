//! uasset rewrite for row-adding patches — replicates UAssetAPI's writer output
//! (see `sicario-oracle/M4-SPEC.md`): append name entries at the name-table end,
//! bump the duplicated name counts, shift post-insert offsets, and update the
//! export entry + size fields.
//!
//! Verified byte-exact against the oracle's merged uasset for the Improved
//! Chimera case (see the `rewrite_matches_oracle` gate).

use crate::header::PackageHeader;
use crate::Error;
use std::io::Cursor;

// Header field offsets for PW's cooked format (legacy -7, no custom versions;
// names start at 193, UAssetAPI's extra fields at fixed positions within the
// 193-byte header). All verified against the oracle diff.
const OFF_SECTION6: usize = 24; // sectionSixOffset (== uasset length for this format)
const OFF_NAME_COUNT: usize = 41; // sectionOneStringCount
const OFF_SECTION3: usize = 61; // sectionThreeOffset (export map start)
const OFF_SECTION2: usize = 69; // sectionTwoOffset (imports start == name-table end)
const OFF_SECTION4: usize = 73; // sectionFourOffset
const OFF_NAME_COUNT2: usize = 117; // headerIndexList.Count (name count, again)
const OFF_UEXP_DATA: usize = 165; // uexpDataOffset
const OFF_FILE_SIZE_MINUS4: usize = 169; // uasset_len + uexp_serial_size
const OFF_UEXP_PRELOAD: usize = 189; // uexpDataOffset + preloadDataOffset

/// The rewrite plan for adding rows to a DataTable asset pair.
#[derive(Debug, Clone)]
pub struct RewritePlan {
    /// Names to append to the NameMap (in order).
    pub name_append: Vec<String>,
    /// Change in the export's serial size (the uexp export-data delta).
    pub uexp_delta: i64,
}

/// Rewrite a cooked uasset for an appended-name row patch.
pub fn rewrite_uasset(uasset: &[u8], plan: &RewritePlan) -> Result<Vec<u8>, Error> {
    let mut cur = Cursor::new(uasset);
    let header = PackageHeader::read(&mut cur)?;
    if header.legacy_file_version != -7 {
        return Err(Error::Edit(format!(
            "unsupported package format (legacy file version {})",
            header.legacy_file_version
        )));
    }
    let insert_at = header.import_offset as usize; // name-table end
    if insert_at == 0 || insert_at > uasset.len() {
        return Err(Error::Edit(format!("invalid import offset {insert_at}")));
    }

    // Build the appended name entries: [len i32][bytes + NUL][hash u32]
    let mut entries = Vec::new();
    for name in &plan.name_append {
        let bytes = name.as_bytes();
        entries.extend_from_slice(&(bytes.len() as i32 + 1).to_le_bytes());
        entries.extend_from_slice(bytes);
        entries.push(0);
        entries.extend_from_slice(&crate::hash::name_hash(name).to_le_bytes());
    }
    let insert_delta = entries.len() as i64;
    let new_len = uasset.len() as i64 + insert_delta;

    // Assemble: [0..insert_at) + entries + [insert_at..]
    let mut out = Vec::with_capacity(new_len as usize);
    out.extend_from_slice(&uasset[..insert_at]);
    out.extend_from_slice(&entries);
    out.extend_from_slice(&uasset[insert_at..]);

    let rd_i32 =
        |b: &[u8], o: usize| -> i64 { i32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as i64 };
    let rd_i64 =
        |b: &[u8], o: usize| -> i64 { i64::from_le_bytes(b[o..o + 8].try_into().unwrap()) };
    let wr_i32 = |b: &mut [u8], o: usize, v: i64| {
        b[o..o + 4].copy_from_slice(&(v as i32).to_le_bytes());
    };
    let wr_i64 = |b: &mut [u8], o: usize, v: i64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());

    // Name counts += appended names.
    let n = plan.name_append.len() as i64;
    let nc = rd_i32(&out, OFF_NAME_COUNT) + n;
    wr_i32(&mut out, OFF_NAME_COUNT, nc);
    let nc2 = rd_i32(&out, OFF_NAME_COUNT2) + n;
    wr_i32(&mut out, OFF_NAME_COUNT2, nc2);

    // Post-insert offsets shift by the insert delta.
    for off in [
        OFF_SECTION6,
        OFF_SECTION3,
        OFF_SECTION2,
        OFF_SECTION4,
        OFF_UEXP_DATA,
    ] {
        let v = rd_i32(&out, off) + insert_delta;
        wr_i32(&mut out, off, v);
    }

    // Export entry: serial_size += uexp delta; serial_offset += insert delta.
    // NOTE: the entry itself sits after the insert point, so its position in
    // the output shifts by the insert delta.
    let export_entry = header.export_offset as usize + insert_delta as usize;
    let serial_size_off = export_entry + 28;
    let serial_offset_off = export_entry + 36;
    let new_serial_size = rd_i64(uasset, header.export_offset as usize + 28) + plan.uexp_delta;
    wr_i64(&mut out, serial_size_off, new_serial_size);
    let new_serial_offset = rd_i64(uasset, header.export_offset as usize + 36) + insert_delta;
    wr_i64(&mut out, serial_offset_off, new_serial_offset);

    // [169] = uasset_len + uexp_serial_size (recomputed).
    wr_i32(&mut out, OFF_FILE_SIZE_MINUS4, new_len + new_serial_size);

    // [189] = uexpDataOffset + preloadDataOffset (preload preserved from vanilla).
    let preload = rd_i32(uasset, OFF_UEXP_PRELOAD) - rd_i32(uasset, OFF_UEXP_DATA);
    let uexp_data = rd_i32(&out, OFF_UEXP_DATA);
    wr_i32(&mut out, OFF_UEXP_PRELOAD, uexp_data + preload);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        std::fs::read(path).unwrap()
    }

    fn vanilla() -> Vec<u8> {
        fixture("DB_Aircraft.uasset")
    }

    #[test]
    fn rewrite_matches_oracle() {
        let merged = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/DB_Aircraft.merged.uasset"
        ))
        .unwrap();
        let out = rewrite_uasset(
            &vanilla(),
            &RewritePlan {
                name_append: vec!["ACG-01X".to_string()],
                uexp_delta: 2653,
            },
        )
        .unwrap();
        assert_eq!(out.len(), merged.len());
        if out != merged {
            let diffs: Vec<usize> = (0..out.len()).filter(|&i| out[i] != merged[i]).collect();
            panic!(
                "rewrite differs from oracle at {} bytes: {:?}",
                diffs.len(),
                &diffs[..diffs.len().min(20)]
            );
        }
    }

    #[test]
    fn rejects_other_formats() {
        let mut fake = vanilla();
        // legacy file version at offset 4 — anything but -7 must be rejected
        // (either by the format check or by a parse failure).
        fake[4..8].copy_from_slice(&(-4i32).to_le_bytes());
        let result = rewrite_uasset(
            &fake,
            &RewritePlan {
                name_append: vec!["X".to_string()],
                uexp_delta: 0,
            },
        );
        assert!(result.is_err());
    }
}
