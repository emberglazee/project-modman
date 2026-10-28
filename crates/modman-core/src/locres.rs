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

/// Bounded little-endian reader over the file bytes.
///
/// A `.locres` can arrive malformed — a corrupt download, a hand-edited file —
/// and `parse` has to return an error rather than panic or abort. Every read
/// goes through a checked slice, and an FString length is widened before it is
/// negated, because `i32::MIN` has no positive counterpart: `-len` overflows in
/// debug and wraps in release, where on a 32-bit target (wasm) the `len * 2`
/// byte count wrapped again to zero, slipped past the bounds check and panicked
/// on the slice.
struct Reader<'a> {
    d: &'a [u8],
    off: usize,
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8], off: usize) -> Self {
        Self { d, off }
    }

    /// Bytes left after the read cursor (0 once past the end).
    fn remaining(&self) -> usize {
        self.d.len().saturating_sub(self.off)
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], LocresError> {
        let end = self
            .off
            .checked_add(n)
            .ok_or(LocresError::Truncated(what))?;
        let bytes = self
            .d
            .get(self.off..end)
            .ok_or(LocresError::Truncated(what))?;
        self.off = end;
        Ok(bytes)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, LocresError> {
        Ok(u32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn fstring(&mut self) -> Result<String, LocresError> {
        let len = i32::from_le_bytes(self.take(4, "fstring length")?.try_into().unwrap());
        if len == 0 {
            return Ok(String::new());
        }
        if len > 0 {
            let n = len as usize;
            let body = self.take(n, "fstring body")?;
            return Ok(String::from_utf8_lossy(&body[..n - 1]).into_owned());
        }
        // UTF-16: `-len` counts u16 units, two bytes each. Widening first keeps
        // `i32::MIN` representable; refusing an un-representable byte count
        // keeps the multiply from wrapping on 32-bit.
        let units = len.unsigned_abs() as usize;
        let bytes = units
            .checked_mul(2)
            .ok_or(LocresError::Truncated("fstring utf16 body"))?;
        let body = self.take(bytes, "fstring utf16 body")?;
        let mut pairs = Vec::with_capacity(units - 1);
        for c in body[..bytes - 2].as_chunks::<2>().0 {
            pairs.push(u16::from_le_bytes(*c));
        }
        Ok(String::from_utf16_lossy(&pairs))
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
        // UE counts UTF-16 code units, not chars: an astral character is one
        // `char` but two units, and the length prefix must match the body.
        let units = s.encode_utf16().count() + 1;
        out.extend_from_slice(&(-(units as i32)).to_le_bytes());
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
        4 + s.encode_utf16().count() * 2 + 2
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
        let raw_offset = i64::from_le_bytes(d[17..25].try_into().unwrap());
        let string_table_offset = usize::try_from(raw_offset).map_err(|_| {
            LocresError::Structure(format!("negative string table offset {raw_offset}"))
        })?;
        if string_table_offset >= d.len() {
            return Err(LocresError::Structure(format!(
                "string table offset {string_table_offset} beyond file"
            )));
        }

        let mut r = Reader::new(d, 25);
        let entry_count = r.u32("entry count")? as usize;
        let ns_count = r.u32("namespace count")? as usize;

        // A forged count must not turn a small file into a huge reservation:
        // the smallest namespace is 12 bytes (hash, name length, key count).
        // Without the clamp a 33-byte file claiming u32::MAX namespaces tried
        // to reserve 240 GB and aborted the process.
        let mut namespaces = Vec::with_capacity(ns_count.min(r.remaining() / 12));
        let mut key_total = 0usize;
        for _ in 0..ns_count {
            let hash = r.u32("namespace hash")?;
            let name = r.fstring()?;
            let key_count = r.u32("key count")? as usize;
            // Smallest key: hash, key length, source hash, string index = 16.
            let mut keys = Vec::with_capacity(key_count.min(r.remaining() / 16));
            for _ in 0..key_count {
                let key_hash = r.u32("key hash")?;
                let key = r.fstring()?;
                let source_hash = r.u32("key source hash")?;
                let string_index = r.u32("key string index")?;
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
        if r.off != string_table_offset {
            return Err(LocresError::Structure(format!(
                "key section ends at {}, string table offset is {string_table_offset}",
                r.off
            )));
        }
        if key_total != entry_count {
            return Err(LocresError::Structure(format!(
                "entries count {entry_count} != keys {key_total}"
            )));
        }

        let mut r = Reader::new(d, string_table_offset);
        let string_count = r.u32("string table count")? as usize;
        // Smallest entry: string length + refcount = 8 bytes.
        let mut strings = Vec::with_capacity(string_count.min(r.remaining() / 8));
        let mut ref_counts = Vec::with_capacity(strings.capacity());
        for _ in 0..string_count {
            strings.push(r.fstring()?);
            ref_counts.push(r.u32("string table refcount")?);
        }
        if r.off != d.len() {
            return Err(LocresError::Structure(format!(
                "string table ends at {}, file is {} bytes",
                r.off,
                d.len()
            )));
        }

        // Every key must point at a string that exists. `patch_from_csv` writes
        // by index, so a forged or corrupt index would otherwise panic there.
        for ns in &namespaces {
            for k in &ns.keys {
                if k.string_index as usize >= strings.len() {
                    return Err(LocresError::Structure(format!(
                        "string index {} out of range ({} strings)",
                        k.string_index,
                        strings.len()
                    )));
                }
            }
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
                        // `parse` rejects out-of-range indices; this guard keeps
                        // a hand-built LocresFile from panicking here too.
                        let Some(slot) = new_strings.get_mut(k.string_index as usize) else {
                            continue;
                        };
                        if slot.as_str() != tr.as_str() {
                            *slot = tr.clone();
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
    fn astral_strings_round_trip() {
        let mut f = sample();
        f.strings[1] = "😀".into();
        f.strings[2] = "a😀b".into();
        let bytes = f.write();
        let parsed = LocresFile::parse(&bytes).expect("parse");
        assert_eq!(parsed.strings[1], "😀");
        assert_eq!(parsed.strings[2], "a😀b");
        assert_eq!(parsed.write(), bytes, "round trip must be byte-identical");
    }

    #[test]
    fn astral_names_and_keys_round_trip() {
        let mut f = sample();
        f.namespaces[1].name = "😀".into();
        f.namespaces[0].keys[0].key = "😀".into();
        let bytes = f.write();
        let parsed = LocresFile::parse(&bytes).expect("parse");
        assert_eq!(parsed.namespaces[1].name, "😀");
        assert_eq!(parsed.namespaces[0].keys[0].key, "😀");
        assert_eq!(parsed.write(), bytes);
    }

    #[test]
    fn out_of_range_string_index_is_rejected() {
        let mut f = sample();
        f.namespaces[0].keys[0].string_index = 5;
        let bytes = f.write();
        assert!(matches!(
            LocresFile::parse(&bytes),
            Err(LocresError::Structure(_))
        ));
    }

    #[test]
    fn patch_from_csv_does_not_panic_on_a_hand_built_index() {
        let mut f = sample();
        f.namespaces[0].keys[0].string_index = 5;
        let n = f.patch_from_csv("AABBCCDDEEFF00112233445566778899,src,new\n");
        assert_eq!(n, 0);
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

    /// Header-only scaffold: magic, version 3, string-table offset, zero entry
    /// count, `ns_count`, then whatever the caller appends.
    fn malformed(follow: &[u8], ns_count: u32, st_offset: i64) -> Vec<u8> {
        let mut v = Vec::with_capacity(33 + follow.len());
        v.extend_from_slice(&LOCRES_MAGIC);
        v.push(3);
        v.extend_from_slice(&st_offset.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&ns_count.to_le_bytes());
        v.extend_from_slice(follow);
        v
    }

    /// `i32::MIN` as an FString length is the shape that used to crash: `-len`
    /// overflows in a debug build, and in a 32-bit release (wasm) the `len * 2`
    /// byte count wraps to zero, so the bounds check passes and the slice
    /// panics. Reproduced on the real wasm module as a bare `unreachable` trap.
    #[test]
    fn i32_min_fstring_length_is_an_error() {
        let mut follow = 0u32.to_le_bytes().to_vec(); // namespace hash
        follow.extend_from_slice(&i32::MIN.to_le_bytes()); // FString length
        let bytes = malformed(&follow, 1, 37);
        assert_eq!(bytes.len(), 41);
        assert!(matches!(
            LocresFile::parse(&bytes),
            Err(LocresError::Truncated(_))
        ));
    }

    /// Every read in `parse` is length-checked, so no prefix of a valid file
    /// may panic (the fixed-width reads used to index unconditionally).
    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let bytes = sample().write();
        for i in 0..bytes.len() {
            assert!(
                LocresFile::parse(&bytes[..i]).is_err(),
                "a {i}-byte prefix must not parse"
            );
        }
    }

    /// A forged count must be refused by the bounds checks, not turned into a
    /// reservation: `u32::MAX` namespaces on a 33-byte file asked for 240 GB
    /// and aborted the process before the capacities were clamped.
    #[test]
    fn forged_counts_are_refused_without_huge_reservations() {
        let bytes = malformed(&[], u32::MAX, 24);
        assert!(matches!(
            LocresFile::parse(&bytes),
            Err(LocresError::Truncated(_))
        ));

        // A namespace claiming u32::MAX keys with no bytes left after it.
        let mut follow = 0u32.to_le_bytes().to_vec();
        follow.extend_from_slice(&1i32.to_le_bytes());
        follow.push(0);
        follow.extend_from_slice(&u32::MAX.to_le_bytes());
        let bytes = malformed(&follow, 1, 42);
        assert!(matches!(
            LocresFile::parse(&bytes),
            Err(LocresError::Truncated(_))
        ));

        // And a forged string-table count on an otherwise valid file.
        let mut bytes = sample().write();
        let st = i64::from_le_bytes(bytes[17..25].try_into().unwrap()) as usize;
        bytes[st..st + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            LocresFile::parse(&bytes),
            Err(LocresError::Truncated(_))
        ));
    }
}
