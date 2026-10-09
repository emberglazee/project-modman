//! HexPatch-compatible raw file patch engine.
//!
//! Ports the observable behavior of `HexPatch` 0.0.0-preview.0.18 (the library
//! the C# merger uses for `filePatches`):
//!
//! * Replacement types run in a fixed order per patch set: `before` →
//!   `inPlace` → `valueBefore` (NOT patch array order). `none`/unknown types
//!   are silently ignored.
//! * Match offsets are always computed against the bytes at the START of the
//!   patch set; replacements then apply sequentially against the evolving
//!   buffer.
//! * `inPlace` uses a stream-walk: gap copy (if positive), write value (empty
//!   value writes nothing), then seek forward by the template length **from
//!   the current position** — overlapping matches can therefore drop bytes,
//!   faithfully reproducing the C# behavior (including the destructive case
//!   where a patch's `value` is absent).
//! * `before` overwrites the `value.len()` bytes immediately before each
//!   match; `valueBefore` does the same but skips matches whose preceding
//!   bytes are all zero.
//! * Windows: `after`/`before` anchors with the C# region/break semantics.
//! * After a `.uexp` size change the engine hex-swaps the old serial size
//!   (`len - 4`, little-endian i32) for the new one in the sibling `.uasset`.

use crate::manifest::{FilePatch, FilePatchSet};

/// Errors surfaced by the hex engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HexPatchError {
    /// Template string missing entirely (C# would NRE; we fail cleanly).
    MissingTemplate,
    /// Invalid hex/int/text encoding in a template or value.
    BadEncoding(String),
    /// A `before`/`valueBefore` write would start before offset 0.
    WriteBeforeStart { offset: usize, len: usize },
}

impl std::fmt::Display for HexPatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HexPatchError::MissingTemplate => write!(f, "hex patch has no template"),
            HexPatchError::BadEncoding(s) => write!(f, "invalid hex patch encoding: {s}"),
            HexPatchError::WriteBeforeStart { offset, len } => write!(
                f,
                "hex patch write of {len} bytes before offset {offset} would underflow"
            ),
        }
    }
}

impl std::error::Error for HexPatchError {}

/// Decode a HexPatch string to bytes.
///
/// Mirrors `ByteExtensions.ToByteArray`: `int:` prefix → little-endian i32,
/// `text:` prefix → UTF-8, otherwise whitespace/dash-stripped hex with the
/// C# odd-length truncation quirk.
pub fn to_byte_array(s: &str) -> Result<Vec<u8>, HexPatchError> {
    if let Some(rest) = s.strip_prefix("int:") {
        let n: i32 = rest
            .trim()
            .parse()
            .map_err(|_| HexPatchError::BadEncoding(s.to_string()))?;
        return Ok(n.to_le_bytes().to_vec());
    }
    if let Some(rest) = s.strip_prefix("text:") {
        return Ok(rest.as_bytes().to_vec());
    }
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    let n = cleaned.len() / 2; // C# `template.Length / 2` truncates odd lengths.
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let byte = u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16)
            .map_err(|_| HexPatchError::BadEncoding(s.to_string()))?;
        out.push(byte);
    }
    Ok(out)
}

/// Find every (possibly overlapping) occurrence of `needle` in `hay`.
///
/// An empty needle matches at every position `0..hay.len()`, matching the
/// C# `GetPatterns` behavior with `Take(0)` windows.
pub fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return (0..hay.len()).collect();
    }
    if needle.len() > hay.len() {
        return out;
    }
    for i in 0..=(hay.len() - needle.len()) {
        if &hay[i..i + needle.len()] == needle {
            out.push(i);
        }
    }
    out
}

/// The C# `PatternAt`: collect match offsets honoring the window.
pub fn pattern_at(src: &[u8], patch: &FilePatch) -> Result<Vec<usize>, HexPatchError> {
    let template = patch
        .template
        .as_deref()
        .ok_or(HexPatchError::MissingTemplate)?;
    let tmpl = to_byte_array(template)?;

    let window = patch.window.as_ref();
    let after = window
        .and_then(|w| w.after.as_deref())
        .filter(|a| !a.trim().is_empty());
    let before = window
        .and_then(|w| w.before.as_deref())
        .filter(|b| !b.trim().is_empty());

    // `window.after == null` (or no window at all) => all matches.
    let after_missing = window.and_then(|w| w.after.as_deref()).is_none();
    let mut results: Vec<usize> = if after_missing {
        find_all(src, &tmpl)
    } else {
        // `after` present but blank => no matches (C# falls through to empty).
        let Some(after) = after else {
            return Ok(Vec::new());
        };

        let a_bytes = to_byte_array(after)?;
        let mut results: Vec<usize> = Vec::new();

        if let Some(before) = before {
            let b_bytes = to_byte_array(before)?;
            let a_offs = find_all(src, &a_bytes);

            // Build regions: for each after-anchor, the first before-anchor after
            // it (break at the next after-anchor offset or the break pattern).
            let mut regions: Vec<(usize, usize)> = Vec::new();
            for (idx, &astart) in a_offs.iter().enumerate() {
                let next_a = a_offs.get(idx + 1).copied();
                let mut bend = None;
                for p in astart..src.len() {
                    let first = &src[p..(p + b_bytes.len()).min(src.len())];
                    if !b_bytes.is_empty()
                        && first.len() == a_bytes.len()
                        && first == a_bytes.as_slice()
                    {
                        break;
                    }
                    if next_a == Some(p) {
                        break;
                    }
                    if first.len() == b_bytes.len() && first == b_bytes.as_slice() {
                        bend = Some(p);
                        break;
                    }
                }
                if let Some(be) = bend {
                    regions.push((astart, be));
                }
            }

            // Template matches per region, starting just past the after-anchor.
            for (start, end) in regions {
                let base = start + a_bytes.len();
                let mut p = base;
                while p < src.len() {
                    let first = &src[p..(p + tmpl.len()).min(src.len())];
                    if !tmpl.is_empty()
                        && first.len() == a_bytes.len()
                        && first == a_bytes.as_slice()
                    {
                        break;
                    }
                    if p == end {
                        break;
                    }
                    if first.len() == tmpl.len() && first == tmpl.as_slice() {
                        results.push(p);
                    }
                    p += 1;
                }
            }
        } else {
            // after-only: scan from each anchor to the next anchor.
            let a_offs = find_all(src, &a_bytes);
            for (idx, &astart) in a_offs.iter().enumerate() {
                let next_a = a_offs.get(idx + 1).copied();
                let mut p = astart;
                while p < src.len() {
                    if next_a == Some(p) {
                        break;
                    }
                    let first = &src[p..(p + tmpl.len()).min(src.len())];
                    if first.len() == tmpl.len() && first == tmpl.as_slice() {
                        results.push(p);
                    }
                    p += 1;
                }
            }
        }
        results
    };

    // MaxMatches applies to every branch (C# applies TakeTo at collection).
    if let Some(max) = window.and_then(|w| w.max_matches) {
        if max > 0 {
            results.truncate(max as usize);
        }
    }
    Ok(results)
}

/// One planned replacement: match offset, template length, replacement bytes.
#[derive(Debug, Clone)]
struct Replacement {
    offset: usize,
    key_len: usize,
    value: Vec<u8>,
}

/// `inPlace`: stream-walk with relative seeks (faithful to the C#).
fn apply_inplace(src: &[u8], repls: &[Replacement]) -> Vec<u8> {
    let mut sorted: Vec<&Replacement> = repls.iter().collect();
    sorted.sort_by_key(|r| r.offset);

    let mut out = Vec::with_capacity(src.len() + src.len() / 4);
    let mut pos: usize = 0;
    for r in sorted {
        let gap = r.offset as i64 - pos as i64;
        if gap > 0 {
            let n = (gap as usize).min(src.len().saturating_sub(pos));
            out.extend_from_slice(&src[pos..pos + n]);
            pos += n;
        }
        out.extend_from_slice(&r.value);
        // Seek key length from the CURRENT position (not from the match).
        pos = pos.saturating_add(r.key_len);
    }
    if pos < src.len() {
        out.extend_from_slice(&src[pos..]);
    }
    out
}

/// `before` / `valueBefore`: overwrite the value-length bytes before each match.
fn apply_before(
    src: &[u8],
    repls: &[Replacement],
    require_value: bool,
) -> Result<Vec<u8>, HexPatchError> {
    let mut sorted: Vec<&Replacement> = repls.iter().collect();
    sorted.sort_by_key(|r| r.offset);

    let mut out = src.to_vec();
    for r in sorted {
        if r.value.is_empty() {
            continue;
        }
        if r.offset < r.value.len() {
            return Err(HexPatchError::WriteBeforeStart {
                offset: r.offset,
                len: r.value.len(),
            });
        }
        let start = r.offset - r.value.len();
        if require_value && src[start..r.offset].iter().all(|&b| b == 0) {
            continue;
        }
        out[start..r.offset].copy_from_slice(&r.value);
    }
    Ok(out)
}

/// Replacement type names in the fixed C# order.
const REPLACEMENT_ORDER: [&str; 3] = ["before", "inplace", "valuebefore"];

/// Apply a list of file patch sets to raw file bytes.
pub fn run_file_patches(src: &[u8], sets: &[FilePatchSet]) -> Result<Vec<u8>, HexPatchError> {
    let mut file_bytes = src.to_vec();
    for set in sets {
        // Offsets are computed against the set-start bytes for every patch.
        let set_start = file_bytes.clone();
        let mut array = file_bytes.clone();
        for type_name in REPLACEMENT_ORDER {
            let mut repls: Vec<Replacement> = Vec::new();
            for patch in &set.patches {
                let matches_type = patch
                    .patch_type
                    .as_deref()
                    .map(|t| t.eq_ignore_ascii_case(type_name))
                    .unwrap_or(false);
                if !matches_type {
                    continue;
                }
                let offsets = pattern_at(&set_start, patch)?;
                let key_len = match patch.template.as_deref() {
                    Some(t) => to_byte_array(t)?.len(),
                    None => return Err(HexPatchError::MissingTemplate),
                };
                // Missing/absent value => empty replacement (C# writes an
                // empty span — a no-op in the stream-walk).
                let value = match patch.value.as_deref() {
                    Some(v) if !v.is_empty() => to_byte_array(v)?,
                    _ => Vec::new(),
                };
                for offset in offsets {
                    repls.push(Replacement {
                        offset,
                        key_len,
                        value: value.clone(),
                    });
                }
            }
            array = match type_name {
                "inplace" => apply_inplace(&array, &repls),
                "valuebefore" => apply_before(&array, &repls, true)?,
                _ => apply_before(&array, &repls, false)?,
            };
        }
        file_bytes = array;
    }
    Ok(file_bytes)
}

/// Hex string with no separators, as the C# builds it for the length fixup.
fn hex_no_sep(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect()
}

/// The `Length auto-correct` patch: swap the old serial size for the new one
/// in the sibling `.uasset` after a `.uexp` size change.
pub fn apply_length_fixup(
    uasset: &[u8],
    old_uexp_len: usize,
    new_uexp_len: usize,
) -> Result<Vec<u8>, HexPatchError> {
    let template = hex_no_sep(&((old_uexp_len as i64 - 4) as i32).to_le_bytes());
    let value = hex_no_sep(&((new_uexp_len as i64 - 4) as i32).to_le_bytes());
    let patch = FilePatch {
        description: "uexp Length".to_string(),
        template: Some(template),
        value: Some(value),
        patch_type: Some("inPlace".to_string()),
        window: None,
    };
    let set = FilePatchSet {
        name: "Length auto-correct".to_string(),
        patches: vec![patch],
    };
    run_file_patches(uasset, &[set])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(template: &str, value: &str, ty: &str) -> FilePatch {
        FilePatch {
            description: String::new(),
            template: Some(template.to_string()),
            value: Some(value.to_string()),
            patch_type: Some(ty.to_string()),
            window: None,
        }
    }

    fn set(patches: Vec<FilePatch>) -> FilePatchSet {
        FilePatchSet {
            name: "test".to_string(),
            patches,
        }
    }

    #[test]
    fn byte_array_decoding() {
        assert_eq!(to_byte_array("00 48 02").unwrap(), vec![0x00, 0x48, 0x02]);
        assert_eq!(
            to_byte_array("B4-43-00-00").unwrap(),
            vec![0xB4, 0x43, 0x00, 0x00]
        );
        assert_eq!(to_byte_array("int:5").unwrap(), vec![5, 0, 0, 0]);
        assert_eq!(to_byte_array("text:AB").unwrap(), b"AB".to_vec());
        // Odd length truncates (C# quirk).
        assert_eq!(to_byte_array("ABC").unwrap(), vec![0xAB]);
    }

    #[test]
    fn inplace_same_size() {
        let src = b"xxrgpsyy";
        let out = run_file_patches(
            src,
            &[set(vec![fp("72 67 70 73", "72 67 70 64", "inPlace")])],
        )
        .unwrap();
        assert_eq!(out, b"xxrgpdyy");
    }

    #[test]
    fn inplace_all_matches() {
        let src = b"AAAA";
        let out = run_file_patches(src, &[set(vec![fp("41", "42", "inPlace")])]).unwrap();
        assert_eq!(out, b"BBBB");
    }

    #[test]
    fn inplace_grow() {
        let src = b"aabbaa";
        let out = run_file_patches(src, &[set(vec![fp("6262", "626262", "inPlace")])]).unwrap();
        assert_eq!(out, b"aabbbaa");
    }

    #[test]
    fn inplace_empty_value_drops_template() {
        // The destructive path: absent value + matching template.
        let src = b"AAxxBBBB";
        let patch = FilePatch {
            description: String::new(),
            template: Some("7878".to_string()),
            value: None,
            patch_type: Some("inPlace".to_string()),
            window: None,
        };
        let out = run_file_patches(src, &[set(vec![patch])]).unwrap();
        assert_eq!(out, b"AABBBB");
    }

    #[test]
    fn before_type() {
        let src = b"AAxxBBBB";
        // Value is 2 bytes, so the 2 bytes before the match are overwritten.
        let out = run_file_patches(src, &[set(vec![fp("7878", "43 43", "before")])]).unwrap();
        assert_eq!(out, b"CCxxBBBB");
    }

    #[test]
    fn value_before_skips_zero_regions() {
        let src = b"\x00\x00xx\x01\x01yy";
        let out = run_file_patches(
            src,
            &[set(vec![
                fp("7878", "43 43", "valueBefore"),
                fp("7979", "44 44", "valueBefore"),
            ])],
        )
        .unwrap();
        // First match preceded by zeros => untouched; second replaced.
        assert_eq!(out, b"\x00\x00xx\x44\x44yy");
    }

    #[test]
    fn none_type_ignored() {
        let src = b"AAAA";
        let out = run_file_patches(src, &[set(vec![fp("41", "42", "none")])]).unwrap();
        assert_eq!(out, src.to_vec());
    }

    #[test]
    fn replacement_type_order_before_then_inplace() {
        // `before` runs first even though `inPlace` is listed first.
        let src = b"AAxxBBBB";
        let out = run_file_patches(
            src,
            &[set(vec![
                fp("7878", "78 78", "inPlace"),
                fp("7878", "43 43", "before"),
            ])],
        )
        .unwrap();
        assert_eq!(out, b"CCxxBBBB");
    }

    #[test]
    fn window_after_before() {
        // Note the C# quirk: if `before` and `after` anchors are the same
        // length, the break-pattern check fires at the anchor itself and no
        // region ever forms. Use different lengths (like the real docs do).
        let src = b"AAAxxBB";
        let patch = FilePatch {
            description: String::new(),
            template: Some("7878".to_string()),
            value: Some("7a7a".to_string()),
            patch_type: Some("inPlace".to_string()),
            window: Some(crate::manifest::HexWindow {
                after: Some("414141".to_string()),
                before: Some("4242".to_string()),
                max_matches: None,
            }),
        };
        let out = run_file_patches(src, &[set(vec![patch])]).unwrap();
        assert_eq!(out, b"AAAzzBB");
    }

    #[test]
    fn window_equal_anchor_lengths_never_match() {
        // Faithful C# quirk: equal-length anchors abort the region scan.
        let src = b"AAAxxBBB";
        let patch = FilePatch {
            description: String::new(),
            template: Some("7878".to_string()),
            value: Some("7a7a".to_string()),
            patch_type: Some("inPlace".to_string()),
            window: Some(crate::manifest::HexWindow {
                after: Some("414141".to_string()),
                before: Some("424242".to_string()),
                max_matches: None,
            }),
        };
        let out = run_file_patches(src, &[set(vec![patch])]).unwrap();
        assert_eq!(out, src.to_vec());
    }

    #[test]
    fn window_after_only() {
        let src = b"xxAAAyyzzxx";
        let patch = FilePatch {
            description: String::new(),
            template: Some("7979".to_string()),
            value: Some("7a7a".to_string()),
            patch_type: Some("inPlace".to_string()),
            window: Some(crate::manifest::HexWindow {
                after: Some("414141".to_string()),
                before: None,
                max_matches: None,
            }),
        };
        let out = run_file_patches(src, &[set(vec![patch])]).unwrap();
        assert_eq!(out, b"xxAAAzzzzxx");
    }

    #[test]
    fn max_matches_limits() {
        let src = b"AAAAAA";
        let patch = FilePatch {
            description: String::new(),
            template: Some("41".to_string()),
            value: Some("42".to_string()),
            patch_type: Some("inPlace".to_string()),
            window: Some(crate::manifest::HexWindow {
                after: None,
                before: None,
                max_matches: Some(2),
            }),
        };
        let out = run_file_patches(src, &[set(vec![patch])]).unwrap();
        // maxMatches applies even without anchors (window present).
        assert_eq!(out, b"BBAAAA");
    }

    #[test]
    fn length_fixup_swaps_serial_size() {
        let mut uasset = vec![0u8; 16];
        let old: i32 = 99_322;
        let new: i32 = 99_323;
        uasset[4..8].copy_from_slice(&old.to_le_bytes());
        let out = apply_length_fixup(&uasset, 99_326, 99_327).unwrap();
        assert_eq!(&out[4..8], &new.to_le_bytes());
    }
}
