//! FName-entry hash for UE4 name tables — port of UAssetAPI's
//! `CRCGenerator.GenerateHash` (the 4 bytes written after each name string).
//!
//! Verified against oracle-written name entries:
//! `ACG-01X → 0xE32292A9`, `YawSpeed_37_33AA996848D24FBF807F609931FDC135 → 0x8CF4583A`.

/// Hash written after each name-table string entry (4 bytes, little-endian).
pub fn name_hash(text: &str) -> u32 {
    let a = strihash_deprecated(text.as_bytes());
    let b = strcrc32_wide(text);
    (a & 0xFFFF) | ((b & 0xFFFF) << 16)
}

/// UAssetAPI `Strihash_DEPRECATED`: uppercase-ASCII CRC, MSB-first poly 0x04C11DB7.
fn strihash_deprecated(data: &[u8]) -> u32 {
    let table = stri_table();
    let mut h: u32 = 0;
    for &b in data {
        let ch = (b as char).to_ascii_uppercase() as u32;
        h = ((h >> 8) & 0x00FF_FFFF) ^ table[((h ^ ch) & 0xFF) as usize];
    }
    h
}

/// UAssetAPI `StrCrc32`: standard CRC32 over a 4-bytes-per-char little-endian
/// sequence ("accurate for both WIDECHAR and ANSICHAR").
fn strcrc32_wide(text: &str) -> u32 {
    let table = crc32_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for ch in text.chars() {
        let mut c = ch as u32;
        for _ in 0..4 {
            crc = (crc >> 8) ^ table[((crc ^ (c & 0xFF)) & 0xFF) as usize];
            c >>= 8;
        }
    }
    !crc
}

fn stri_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let mut crc = (i as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
        *e = crc;
    }
    t
}

fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let mut crc = i as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
        *e = crc;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_hash_vectors() {
        assert_eq!(name_hash("ACG-01X"), 0xE322_92A9);
        assert_eq!(
            name_hash("YawSpeed_37_33AA996848D24FBF807F609931FDC135"),
            0x8CF4_583A
        );
    }

    #[test]
    fn table_generation_matches_reference_entries() {
        assert_eq!(stri_table()[1], 0x04C1_1DB7);
        assert_eq!(crc32_table()[1], 0x7707_3096);
    }
}
