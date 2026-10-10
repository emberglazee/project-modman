//! Unreal Engine `.locres` (FTextLocalizationResource) reader/writer.
//!
//! Supports the "optimized" formats the modern engine ships (PW 2.x uses
//! version 3, CityHash64-over-UTF16 hashes): a header with a string-table
//! offset, a key section (namespaces → keys with preserved hashes and string
//! indices) and a reference-counted string table.
//!
//! The writer reproduces the parsed structure byte-for-byte when the hashes
//! and string order are preserved — the round-trip gate — and can also build
//! fresh files from CSV (hashing computed per the engine's algorithms).

use std::fmt::Write as _;

/// 16-byte locres magic (the modern format).
pub const LOCRES_MAGIC: [u8; 16] = [
    0x0E, 0x14, 0x74, 0x75, 0x67, 0x4A, 0x03, 0xFC, 0x4A, 0x15, 0x90, 0x9D, 0xC3, 0x37, 0x7F, 0x1B,
];

/// Locres format versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocresVersion {
    /// v2: optimized, FCrc::StrCrc32 hashes.
    OptimizedCrc32,
    /// v3: optimized, CityHash64-over-UTF16 hashes (UE4.27+, PW 2.x).
    OptimizedCityHash64,
}

impl LocresVersion {
    pub fn as_byte(self) -> u8 {
        match self {
            LocresVersion::OptimizedCrc32 => 2,
            LocresVersion::OptimizedCityHash64 => 3,
        }
    }
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            2 => Some(LocresVersion::OptimizedCrc32),
            3 => Some(LocresVersion::OptimizedCityHash64),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocresError {
    #[error("bad magic (not a locres file)")]
    BadMagic,
    #[error("unsupported locres version {0} (supported: 2, 3)")]
    UnsupportedVersion(u8),
    #[error("truncated locres: {0}")]
    Truncated(&'static str),
    #[error("structural mismatch: {0}")]
    Structure(String),
}

#[derive(Debug, Clone)]
pub struct LocresKey {
    pub hash: u32,
    pub key: String,
    pub source_hash: u32,
    pub string_index: u32,
}

#[derive(Debug, Clone)]
pub struct LocresNamespace {
    pub hash: u32,
    pub name: String,
    pub keys: Vec<LocresKey>,
}

#[derive(Debug, Clone)]
pub struct LocresFile {
    pub version: LocresVersion,
    pub namespaces: Vec<LocresNamespace>,
    /// The string table (referenced by `string_index`).
    pub strings: Vec<String>,
    /// Per-string reference counts (parallel to `strings`; the engine stores
    /// how many keys share each string — preserve them for byte-exact
    /// round-trips, default 1 for freshly built files).
    pub ref_counts: Vec<u32>,
}

// ── FString helpers (UE convention: positive len = UTF-8, negative = UTF-16) ──

fn read_fstring(d: &[u8], off: &mut usize) -> Result<String, LocresError> {
    if *off + 4 > d.len() {
        return Err(LocresError::Truncated("fstring length"));
    }
    let len = i32::from_le_bytes(d[*off..*off + 4].try_into().unwrap());
    *off += 4;
    if len == 0 {
        return Ok(String::new());
    }
    if len > 0 {
        let n = len as usize;
        if *off + n > d.len() {
            return Err(LocresError::Truncated("fstring body"));
        }
        let s = String::from_utf8_lossy(&d[*off..*off + n.saturating_sub(1)]).into_owned();
        *off += n;
        Ok(s)
    } else {
        let n = (-len) as usize;
        if *off + n * 2 > d.len() {
            return Err(LocresError::Truncated("fstring utf16 body"));
        }
        let pairs: Vec<u16> = d[*off..*off + (n - 1) * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let s = String::from_utf16_lossy(&pairs);
        *off += n * 2;
        Ok(s)
    }
}

fn write_fstring(out: &mut Vec<u8>, s: &str) {
    if s.is_empty() {
        // UE serializes an "empty" FString as length 1 + the null terminator
        // (5 bytes), not as a null string (length 0) — PW's files confirm.
        out.extend_from_slice(&1i32.to_le_bytes());
        out.push(0);
        return;
    }
    if s.is_ascii() {
        out.extend_from_slice(&((s.len() + 1) as i32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    } else {
        out.extend_from_slice(&(-((s.chars().count() + 1) as i32)).to_le_bytes());
        for c in s.encode_utf16() {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
}

fn fstring_size(s: &str) -> usize {
    if s.is_empty() {
        5
    } else if s.is_ascii() {
        4 + s.len() + 1
    } else {
        4 + s.chars().count() * 2 + 2
    }
}

impl LocresFile {
    /// Parse an optimized (v2/v3) locres file.
    pub fn parse(d: &[u8]) -> Result<LocresFile, LocresError> {
        if d.len() < 25 || d[..16] != LOCRES_MAGIC {
            return Err(LocresError::BadMagic);
        }
        let version =
            LocresVersion::from_byte(d[16]).ok_or(LocresError::UnsupportedVersion(d[16]))?;
        let string_table_offset = i64::from_le_bytes(d[17..25].try_into().unwrap()) as usize;
        if string_table_offset >= d.len() {
            return Err(LocresError::Structure(format!(
                "string table offset {string_table_offset} beyond file"
            )));
        }

        let mut off = 25usize;
        let entry_count = u32::from_le_bytes(d[off..off + 4].try_into().unwrap()) as usize;
        let ns_count = u32::from_le_bytes(d[off + 4..off + 8].try_into().unwrap()) as usize;
        off += 8;

        let mut namespaces = Vec::with_capacity(ns_count);
        let mut key_total = 0usize;
        for _ in 0..ns_count {
            let hash = u32::from_le_bytes(d[off..off + 4].try_into().unwrap());
            off += 4;
            let name = read_fstring(d, &mut off)?;
            let key_count = u32::from_le_bytes(d[off..off + 4].try_into().unwrap()) as usize;
            off += 4;
            let mut keys = Vec::with_capacity(key_count);
            for _ in 0..key_count {
                let key_hash = u32::from_le_bytes(d[off..off + 4].try_into().unwrap());
                off += 4;
                let key = read_fstring(d, &mut off)?;
                let source_hash = u32::from_le_bytes(d[off..off + 4].try_into().unwrap());
                off += 4;
                let string_index = u32::from_le_bytes(d[off..off + 4].try_into().unwrap());
                off += 4;
                keys.push(LocresKey {
                    hash: key_hash,
                    key,
                    source_hash,
                    string_index,
                });
            }
            key_total += keys.len();
            namespaces.push(LocresNamespace { hash, name, keys });
        }
        if off != string_table_offset {
            return Err(LocresError::Structure(format!(
                "key section ends at {off}, string table offset is {string_table_offset}"
            )));
        }
        if key_total != entry_count {
            return Err(LocresError::Structure(format!(
                "entries count {entry_count} != keys {key_total}"
            )));
        }

        let mut soff = string_table_offset;
        if soff + 4 > d.len() {
            return Err(LocresError::Truncated("string table count"));
        }
        let string_count = u32::from_le_bytes(d[soff..soff + 4].try_into().unwrap()) as usize;
        soff += 4;
        let mut strings = Vec::with_capacity(string_count);
        let mut ref_counts = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            strings.push(read_fstring(d, &mut soff)?);
            if soff + 4 > d.len() {
                return Err(LocresError::Truncated("string table refcount"));
            }
            ref_counts.push(u32::from_le_bytes(d[soff..soff + 4].try_into().unwrap()));
            soff += 4;
        }
        if soff != d.len() {
            return Err(LocresError::Structure(format!(
                "string table ends at {soff}, file is {} bytes",
                d.len()
            )));
        }

        Ok(LocresFile {
            version,
            namespaces,
            strings,
            ref_counts,
        })
    }

    /// Total entry count.
    pub fn entry_count(&self) -> usize {
        self.namespaces.iter().map(|n| n.keys.len()).sum()
    }

    /// Serialize. With preserved hashes/string order this reproduces the
    /// original bytes exactly (the round-trip gate).
    pub fn write(&self) -> Vec<u8> {
        // Pre-calculate the key section size (string table follows it).
        let mut key_section = 8;
        for ns in &self.namespaces {
            key_section += 4 + fstring_size(&ns.name) + 4;
            for k in &ns.keys {
                key_section += 4 + fstring_size(&k.key) + 4 + 4;
            }
        }
        let string_table_offset = (25 + key_section) as i64;

        let mut out = Vec::with_capacity(25 + key_section + self.strings.len() * 16);
        out.extend_from_slice(&LOCRES_MAGIC);
        out.push(self.version.as_byte());
        out.extend_from_slice(&string_table_offset.to_le_bytes());
        out.extend_from_slice(&(self.entry_count() as u32).to_le_bytes());
        out.extend_from_slice(&(self.namespaces.len() as u32).to_le_bytes());
        for ns in &self.namespaces {
            out.extend_from_slice(&ns.hash.to_le_bytes());
            write_fstring(&mut out, &ns.name);
            out.extend_from_slice(&(ns.keys.len() as u32).to_le_bytes());
            for k in &ns.keys {
                out.extend_from_slice(&k.hash.to_le_bytes());
                write_fstring(&mut out, &k.key);
                out.extend_from_slice(&k.source_hash.to_le_bytes());
                out.extend_from_slice(&k.string_index.to_le_bytes());
            }
        }
        debug_assert_eq!(out.len(), 25 + key_section);
        out.extend_from_slice(&(self.strings.len() as u32).to_le_bytes());
        for (i, s) in self.strings.iter().enumerate() {
            write_fstring(&mut out, s);
            let rc = self.ref_counts.get(i).copied().unwrap_or(1);
            out.extend_from_slice(&rc.to_le_bytes());
        }
        out
    }

    /// Merge identical strings into single entries, summing their reference
    /// counts and repointing every key (matches how community tools like
    /// UEExtractor compact locres output). Not used by the patch path, which
    /// preserves the game's original structure exactly.
    pub fn dedup_strings(&mut self) {
        use std::collections::HashMap;
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut new_strings: Vec<String> = Vec::new();
        let mut new_refs: Vec<u32> = Vec::new();
        let mut remap: Vec<u32> = Vec::with_capacity(self.strings.len());
        for (i, text) in self.strings.iter().enumerate() {
            let rc = self.ref_counts.get(i).copied().unwrap_or(1);
            if let Some(&idx) = seen.get(text.as_str()) {
                remap.push(idx);
                new_refs[idx as usize] += rc;
            } else {
                let idx = new_strings.len() as u32;
                seen.insert(text.clone(), idx);
                new_strings.push(text.clone());
                new_refs.push(rc);
                remap.push(idx);
            }
        }
        for ns in &mut self.namespaces {
            for k in &mut ns.keys {
                if let Some(&ni) = remap.get(k.string_index as usize) {
                    k.string_index = ni;
                }
            }
        }
        self.strings = new_strings;
        self.ref_counts = new_refs;
    }
    /// Entry list with namespace-composite keys (UEExtractor's convention:
    /// `Namespace::Key`, or just `Key` when the namespace is empty).
    pub fn entries(&self) -> Vec<(String, u32, &str)> {
        let mut out = Vec::with_capacity(self.entry_count());
        for ns in &self.namespaces {
            for k in &ns.keys {
                let composite = if ns.name.is_empty() {
                    k.key.clone()
                } else {
                    format!("{}::{}", ns.name, k.key)
                };
                let text = self
                    .strings
                    .get(k.string_index as usize)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                out.push((composite, k.source_hash, text));
            }
        }
        out
    }

    /// Compare against another locres (e.g. the game's original vs a mod).
    /// Entries are matched by their composite key; order is the modified
    /// file's order, so output is deterministic.
    pub fn diff(&self, other: &LocresFile) -> LocresDiff {
        use std::collections::HashMap;
        let mine: HashMap<String, String> = self
            .entries()
            .into_iter()
            .map(|(k, _, text)| (k, text.to_string()))
            .collect();
        let mut out = LocresDiff::default();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (key, _hash, text) in other.entries() {
            seen.insert(key.clone());
            match mine.get(&key) {
                Some(old) => {
                    if old != text {
                        out.changed.push((key, old.clone(), text.to_string()));
                    }
                }
                None => out.added.push((key, text.to_string())),
            }
        }
        for (key, _hash, text) in self.entries() {
            if !seen.contains(&key) {
                out.removed.push((key, text.to_string()));
            }
        }
        out
    }

    /// Serialize to the UEExtractor-compatible CSV (key,source,Translation),
    /// with RFC-4180 quoting. The Translation column is left empty; use
    /// [`LocresFile::patch_from_csv`] or [`LocresFile::from_csv`] to apply one.
    pub fn to_csv(&self) -> String {
        let mut out = String::from("key,source,Translation\n");
        for (composite, _hash, text) in self.entries() {
            let _ = writeln!(out, "{},{},", csv_escape(&composite), csv_escape(text));
        }
        out
    }

    /// Apply translations from a CSV (first column = key, third = translation;
    /// a missing/empty translation keeps the source). Matches rows by the
    /// composite key; unknown rows are ignored.
    pub fn patch_from_csv(&mut self, csv: &str) -> usize {
        let map = parse_csv(csv);
        let mut patched = 0usize;
        let mut new_strings = self.strings.clone();
        for ns in &self.namespaces {
            for k in &ns.keys {
                let composite = if ns.name.is_empty() {
                    k.key.clone()
                } else {
                    format!("{}::{}", ns.name, k.key)
                };
                if let Some(tr) = map.get(&composite) {
                    if !tr.is_empty() {
                        let existing = self
                            .strings
                            .get(k.string_index as usize)
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        if existing != tr.as_str() {
                            new_strings[k.string_index as usize] = tr.clone();
                            patched += 1;
                        }
                    }
                }
            }
        }
        self.strings = new_strings;
        patched
    }
}

/// Differences between two locres files (matched by composite key).
#[derive(Debug, Default)]
pub struct LocresDiff {
    /// (key, original text, modified text)
    pub changed: Vec<(String, String, String)>,
    /// (key, text) — present only in the modified file
    pub added: Vec<(String, String)>,
    /// (key, text) — present only in the original
    pub removed: Vec<(String, String)>,
}

impl LocresDiff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

/// RFC-4180 escaping: quote when the field contains a comma, quote, or newline.
pub fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Parse `key,source,Translation` rows (RFC-4180 quoted fields — including
/// newlines inside quotes — `#` comment lines skipped). Returns key →
/// translation (non-empty third column only).
pub fn parse_csv(csv: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut row_start = true;
    let mut comment = false;
    let mut chars = csv.chars().peekable();
    while let Some(c) = chars.next() {
        if comment {
            if c == '\n' {
                comment = false;
                row_start = true;
            }
            continue;
        }
        match c {
            '#' if row_start && !in_quotes => comment = true,
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    cur.push('"'); // "" escape
                    chars.next();
                } else {
                    in_quotes = false;
                }
            }
            '"' => {
                in_quotes = true;
            }
            ',' if !in_quotes => fields.push(std::mem::take(&mut cur)),
            '\n' if !in_quotes => {
                cur = cur.trim_end_matches('\r').to_string();
                fields.push(std::mem::take(&mut cur));
                let first = fields.first().cloned().unwrap_or_default();
                if first != "key" && fields.len() >= 3 && !fields[2].is_empty() {
                    map.insert(fields[0].clone(), fields[2].clone());
                }
                fields.clear();
                row_start = true;
            }
            _ => {
                cur.push(c);
                row_start = false;
            }
        }
    }
    if !cur.is_empty() || !fields.is_empty() {
        cur = cur.trim_end_matches('\r').to_string();
        fields.push(cur);
        let first = fields.first().cloned().unwrap_or_default();
        if first != "key" && fields.len() >= 3 && !fields[2].is_empty() {
            map.insert(fields[0].clone(), fields[2].clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LocresFile {
        LocresFile {
            version: LocresVersion::OptimizedCityHash64,
            namespaces: vec![
                LocresNamespace {
                    hash: 0,
                    name: String::new(),
                    keys: vec![
                        LocresKey {
                            hash: 0x11111111,
                            key: "AABBCCDDEEFF00112233445566778899".into(),
                            source_hash: 0x22222222,
                            string_index: 0,
                        },
                        LocresKey {
                            hash: 0x33333333,
                            key: "112233445566778899AABBCCDDEEFF00".into(),
                            source_hash: 0x44444444,
                            string_index: 1,
                        },
                    ],
                },
                LocresNamespace {
                    hash: 0x55555555,
                    name: "C_M01_BRIEF".into(),
                    keys: vec![LocresKey {
                        hash: 0x66666666,
                        key: "99887766554433221100FFEEDDCCBBAA".into(),
                        source_hash: 0x77777777,
                        string_index: 2,
                    }],
                },
            ],
            strings: vec![
                "And risk this fracture creating another exclusion zone?".into(),
                "Apodock Fracture".into(),
                "Emissions, commas, \"quotes\" and\nnewlines are fine".into(),
            ],
            ref_counts: vec![1, 4, 1],
        }
    }

    #[test]
    fn round_trip_is_byte_identical() {
        let f = sample();
        let bytes = f.write();
        let parsed = LocresFile::parse(&bytes).expect("parse");
        assert_eq!(parsed.version, f.version);
        assert_eq!(parsed.entry_count(), 3);
        assert_eq!(parsed.write(), bytes, "round trip must be byte-identical");
    }

    #[test]
    fn utf16_strings_round_trip() {
        let mut f = sample();
        f.strings[1] = "Сбить подкрепление Федерации".into();
        f.strings[2] = "日本語テキスト".into();
        let bytes = f.write();
        let parsed = LocresFile::parse(&bytes).expect("parse");
        assert_eq!(parsed.strings[1], "Сбить подкрепление Федерации");
        assert_eq!(parsed.strings[2], "日本語テキスト");
        assert_eq!(parsed.write(), bytes);
    }

    #[test]
    fn csv_escaping_round_trip() {
        let f = sample();
        let csv = f.to_csv();
        // The quoted source field keeps its commas/quotes/newlines escaped.
        assert!(csv.contains("\"Emissions, commas, \"\"quotes\"\" and\nnewlines are fine\""));

        // A translation with commas, quotes and a newline must survive parsing.
        let messy = "with, comma \"and\" quotes\nplus newline";
        let manual = format!(
            "key,source,Translation\nC_M01_BRIEF::99887766554433221100FFEEDDCCBBAA,src,{}\n",
            csv_escape(messy)
        );
        let parsed = parse_csv(&manual);
        assert_eq!(
            parsed.get("C_M01_BRIEF::99887766554433221100FFEEDDCCBBAA"),
            Some(&messy.to_string())
        );

        // And it applies through patch_from_csv intact.
        let mut g = sample();
        g.patch_from_csv(&manual);
        assert_eq!(g.strings[2], messy);
    }

    #[test]
    fn patch_applies_translations() {
        let mut f = sample();
        let csv = "key,source,Translation\nAABBCCDDEEFF00112233445566778899,And risk this fracture,Перевод строки\n";
        let n = f.patch_from_csv(csv);
        assert_eq!(n, 1);
        assert_eq!(f.strings[0], "Перевод строки");
    }

    #[test]
    fn dedup_merges_identical_strings() {
        let mut f = sample();
        f.strings = vec!["same".into(), "same".into(), "other".into()];
        f.ref_counts = vec![2, 3, 1];
        f.namespaces[0].keys[0].string_index = 0;
        f.namespaces[0].keys[1].string_index = 1;
        f.namespaces[1].keys[0].string_index = 2;
        f.dedup_strings();
        assert_eq!(f.strings, vec!["same".to_string(), "other".to_string()]);
        assert_eq!(f.ref_counts, vec![5, 1]);
        assert_eq!(f.namespaces[0].keys[0].string_index, 0);
        assert_eq!(f.namespaces[0].keys[1].string_index, 0);
        assert_eq!(f.namespaces[1].keys[0].string_index, 1);
        // and it still round-trips
        let bytes = f.write();
        assert_eq!(LocresFile::parse(&bytes).unwrap().write(), bytes);
    }

    #[test]
    fn diff_reports_changes() {
        let a = sample();
        let mut b = sample();
        b.strings[0] = ":3".into(); // changed
        b.namespaces[1].keys[0].string_index = 0; // ...and the third entry now points at index 0 => value ":3"
        let d = a.diff(&b);
        assert_eq!(d.added.len(), 0);
        assert_eq!(d.removed.len(), 0);
        // key1 changed (:3), key3 changed (repounted to index 0)
        assert_eq!(d.changed.len(), 2);
        assert!(d
            .changed
            .iter()
            .any(|(k, _, new)| k.starts_with("AABB") && new == ":3"));

        // added/removed
        let mut c = sample();
        c.namespaces[0].keys.pop();
        let d2 = a.diff(&c);
        assert_eq!(d2.removed.len(), 1);
        assert_eq!(d2.added.len(), 0);
        let d3 = c.diff(&a);
        assert_eq!(d3.added.len(), 1);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = sample().write();
        bytes[0] ^= 0xFF;
        assert!(matches!(
            LocresFile::parse(&bytes),
            Err(LocresError::BadMagic)
        ));
    }
}
