//! Length-changing row operations (M4b): row duplication, FText edits, and
//! grown string values — replicating the C# merger's output byte-for-byte
//! (modulo regenerated FText keys, which are random in the C# itself).
//!
//! See `sicario-oracle/M4-SPEC.md` for the decoded transformation:
//! - new row appended after the last row, before the 4-byte package-tag trailer
//! - row = `[nameIndex][dupIndex][props…][None]`, copied from the source row
//!   with the sub-patches applied (offset-translated) and size fields fixed up
//! - `numEntries` +1; uasset rewritten separately (`modman_uasset::rewrite`)

use crate::apply::{encode_scalar, ApplyError};
use crate::fragment::Fragment;
use crate::manifest::{Patch, WingmanMod};
use crate::patch::{parse_patch_value, PatchOp};
use crate::resolver::{resolve, Node};
use modman_uasset::walk::{ArrayElem, DataTable, Prop, PropValue};

/// Result of a length-changing apply: new uexp bytes + the uasset rewrite inputs.
#[derive(Debug)]
pub struct LengthChangingResult {
    pub uexp: Vec<u8>,
    /// Names to append to the NameMap (for `modman_uasset::rewrite`).
    pub name_append: Vec<String>,
    /// Change in the export serial size (== appended rows' total size).
    pub uexp_delta: i64,
}

/// One planned byte edit (coordinates refer to the original buffer space).
#[derive(Debug, Clone)]
struct PlannedEdit {
    span: (usize, usize),
    bytes: Vec<u8>,
    /// i32 size fields to bump by the delta (prop tags along the chain).
    fixups: Vec<usize>,
    /// FString length prefix to update when the value resizes.
    len_prefix: Option<usize>,
}

struct Dup {
    source: String,
    target: String,
    source_row: usize,
}

/// Apply a mod's length-changing patches for one target file.
pub fn apply_length_changing(
    dt: &DataTable,
    uexp: &[u8],
    modm: &WingmanMod,
    target: &str,
) -> Result<LengthChangingResult, ApplyError> {
    // The C# applies Liquid templating at load time; mirror that here.
    let mut substituted = modm.clone();
    crate::template::apply_variables_to_mod(&mut substituted);
    let Some(sets) = substituted.asset_patches.get(target) else {
        return Err(ApplyError::Unsupported(format!("no patches for {target}")));
    };
    let patches: Vec<&Patch> = sets.iter().flat_map(|s| s.patches.iter()).collect();

    // Phase 1: register duplicateEntry operations.
    let mut dups: Vec<Dup> = Vec::new();
    for p in &patches {
        if p.patch_type == "duplicateEntry" {
            match parse_patch_value(&p.patch_type, &p.value) {
                Ok(PatchOp::DuplicateEntry { source, target }) => {
                    let source_row =
                        dt.rows
                            .iter()
                            .position(|r| r.name == source)
                            .ok_or_else(|| {
                                ApplyError::Value(format!(
                                    "duplicateEntry source not found: {source}"
                                ))
                            })?;
                    dups.push(Dup {
                        source,
                        target,
                        source_row,
                    });
                }
                _ => {
                    return Err(ApplyError::Value(format!(
                        "bad duplicateEntry value: {}",
                        p.value
                    )))
                }
            }
        }
    }
    // No duplicateEntry: pure in-place length-changing mod — the append phase
    // is simply skipped below.

    // Phase 2: classify + plan edits (copy edits vs in-place edits).
    let mut in_place: Vec<PlannedEdit> = Vec::new();
    let mut copy_edits: Vec<Vec<PlannedEdit>> = vec![Vec::new(); dups.len()];
    for p in &patches {
        if p.patch_type == "duplicateEntry" {
            continue;
        }
        let ctx = crate::fragment::parse_template(&p.template)
            .map_err(|e| ApplyError::Parse(e.to_string()))?;
        let mut fragments = ctx.fragments.clone();
        // Virtual-row substitution: a chain starting at a duplicate target
        // resolves against its source row (the copy is edited, not the source).
        let mut copy_index: Option<usize> = None;
        if let Some(Fragment::StructName { name, .. }) = fragments.first() {
            if let Some(i) = dups.iter().position(|d| d.target == *name) {
                copy_index = Some(i);
                if let Some(Fragment::StructName { name, .. }) = fragments.first_mut() {
                    *name = dups[i].source.clone();
                }
            }
        }
        let nodes = resolve(dt, &fragments);
        let op = parse_patch_value(&p.patch_type, &p.value)
            .map_err(|e| ApplyError::Value(e.to_string()))?;
        match op {
            PatchOp::PropertyValue { value_type, value } => {
                for node in &nodes {
                    let edit = plan_value_edit(dt, node, &value_type, &value)?;
                    match copy_index {
                        Some(i) => copy_edits[i].push(edit),
                        None => in_place.push(edit),
                    }
                }
            }
            PatchOp::TextProperty { value, .. } => {
                for node in &nodes {
                    let edits = plan_text_edits(dt, uexp, node, &value)?;
                    match copy_index {
                        Some(i) => copy_edits[i].extend(edits),
                        None => in_place.extend(edits),
                    }
                }
            }
            other => {
                return Err(ApplyError::Unsupported(format!(
                    "patch type '{}' ({})",
                    other.label(),
                    p.description
                )))
            }
        }
    }

    // Phase 3: base buffer with in-place edits applied.
    let mut base = uexp.to_vec();
    apply_edits(&mut base, 0, &in_place)?;
    let inplace_delta: i64 = in_place
        .iter()
        .map(|e| e.bytes.len() as i64 - (e.span.1 - e.span.0) as i64)
        .sum();

    // Phase 4: build the duplicated rows.
    let mut name_append: Vec<String> = Vec::new();
    let mut appended: Vec<u8> = Vec::new();
    let base_names = dt.names.len();
    for (i, dup) in dups.iter().enumerate() {
        let src = &dt.rows[dup.source_row];
        let mut copy = uexp[src.start..src.end].to_vec();
        let new_idx = (base_names + name_append.len()) as i32;
        copy[0..4].copy_from_slice(&new_idx.to_le_bytes());
        copy[4..8].copy_from_slice(&0i32.to_le_bytes());
        apply_edits(&mut copy, src.start, &copy_edits[i])?;
        appended.extend_from_slice(&copy);
        name_append.push(dup.target.clone());
    }
    let uexp_delta = appended.len() as i64 + inplace_delta;

    // Phase 5: assemble — [0..rows_end) + appended rows + [rows_end..trailer).
    let rows_end = dt.rows.last().map(|r| r.end).unwrap_or(0);
    let rows_end_shifted = (rows_end as i64 + inplace_delta) as usize;
    let mut out = Vec::with_capacity(base.len() + appended.len());
    out.extend_from_slice(&base[..rows_end_shifted]);
    out.extend_from_slice(&appended);
    out.extend_from_slice(&base[rows_end_shifted..]);

    // numEntries += new rows.
    let n_off = dt.num_entries_offset;
    let cur = i32::from_le_bytes(out[n_off..n_off + 4].try_into().unwrap());
    out[n_off..n_off + 4].copy_from_slice(&(cur + dups.len() as i32).to_le_bytes());

    Ok(LengthChangingResult {
        uexp: out,
        name_append,
        uexp_delta,
    })
}

fn plan_value_edit(
    dt: &DataTable,
    node: &Node<'_>,
    value_type: &str,
    value: &str,
) -> Result<PlannedEdit, ApplyError> {
    if node.type_name() != value_type {
        return Err(ApplyError::Value(format!(
            "value type {value_type} does not match target type {} ({})",
            node.type_name(),
            node.describe()
        )));
    }
    let span = node
        .value_span()
        .ok_or_else(|| ApplyError::Unsupported(format!("no span for {}", node.describe())))?;
    let bytes = encode_scalar(value_type, value)?;
    let (fixups, _) = fixup_chain(dt, span);
    let len_prefix = if value_type == "StrProperty" {
        Some(span.0 - 4)
    } else {
        None
    };
    Ok(PlannedEdit {
        span,
        bytes,
        fixups,
        len_prefix,
    })
}

fn plan_text_edits(
    dt: &DataTable,
    uexp: &[u8],
    node: &Node<'_>,
    new_source: &str,
) -> Result<Vec<PlannedEdit>, ApplyError> {
    let Node::Prop(p) = node else {
        return Err(ApplyError::Unsupported(format!(
            "textProperty on {}",
            node.describe()
        )));
    };
    let PropValue::Text(_) = &p.value else {
        return Err(ApplyError::Value(format!(
            "textProperty target is not a text: {}",
            p.name
        )));
    };
    let (key_span, src_span) = text_key_source_spans(uexp, p.vstart)
        .ok_or_else(|| ApplyError::Value(format!("cannot parse text layout for {}", p.name)))?;
    let (fixups, _) = fixup_chain(dt, (p.vstart, p.vend));

    let mut edits = Vec::new();
    // Source replacement (may resize; fixup chain bumps the enclosing sizes).
    let mut src_bytes = new_source.as_bytes().to_vec();
    src_bytes.push(0);
    edits.push(PlannedEdit {
        span: src_span,
        bytes: src_bytes,
        fixups,
        len_prefix: Some(src_span.0 - 4),
    });
    // Key regeneration (same size; the C# generates a random GUID here too).
    let key = gen_text_key();
    let mut key_bytes = key.as_bytes().to_vec();
    key_bytes.push(0);
    if key_bytes.len() == key_span.1 - key_span.0 {
        edits.push(PlannedEdit {
            span: key_span,
            bytes: key_bytes,
            fixups: Vec::new(),
            len_prefix: None,
        });
    }
    Ok(edits)
}

/// Size fields of the enclosing prop tags (walker `start + 16`) for a span.
fn fixup_chain(dt: &DataTable, span: (usize, usize)) -> (Vec<usize>, ()) {
    let mut chain: Vec<usize> = Vec::new();
    for row in &dt.rows {
        if span.0 >= row.start && span.1 <= row.end {
            descend(&row.props, span, &mut chain);
            break;
        }
    }
    (chain, ())
}

fn descend(props: &[Prop], span: (usize, usize), chain: &mut Vec<usize>) -> bool {
    for p in props {
        if span.0 >= p.vstart && span.1 <= p.vend {
            chain.push(p.start + 16);
            match &p.value {
                PropValue::Struct { children } => {
                    descend(children, span, chain);
                }
                PropValue::Array { items, .. } => {
                    for it in items {
                        if let ArrayElem::Struct(children) = it {
                            descend(children, span, chain);
                        }
                    }
                }
                _ => {}
            }
            return true;
        }
    }
    false
}

/// Parse a history-0 FText: `[flags u32][history i8][ns][key][source]`.
/// Returns ((key span), (source span)) with spans covering chars + NUL.
fn text_key_source_spans(uexp: &[u8], vstart: usize) -> Option<((usize, usize), (usize, usize))> {
    let mut pos = vstart + 4;
    let history = *uexp.get(pos)? as i8;
    pos += 1;
    if history != 0 {
        return None;
    }
    let (_, _, p2) = fstr_span(uexp, pos)?;
    let (k_start, k_end, p3) = fstr_span(uexp, p2)?;
    let (s_start, s_end, _) = fstr_span(uexp, p3)?;
    Some(((k_start, k_end), (s_start, s_end)))
}

/// FString span: returns (body_start, body_end, next_pos).
fn fstr_span(b: &[u8], pos: usize) -> Option<(usize, usize, usize)> {
    let len = i32::from_le_bytes(b.get(pos..pos + 4)?.try_into().ok()?);
    if len < 0 {
        return None; // UTF-16 strings unsupported here
    }
    let body = pos + 4;
    let end = body + len as usize;
    if end > b.len() {
        return None;
    }
    Some((body, end, end))
}

/// Apply edits to a buffer. Edits are processed from the end of the buffer
/// backwards so earlier offsets stay valid; fixups/len-prefixes always sit
/// before their edit's span.
fn apply_edits(buf: &mut Vec<u8>, base: usize, edits: &[PlannedEdit]) -> Result<(), ApplyError> {
    let mut sorted: Vec<&PlannedEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.span.0));
    for e in sorted {
        let (a, b) = (e.span.0 - base, e.span.1 - base);
        if a > b || b > buf.len() {
            return Err(ApplyError::Splice(format!(
                "edit span {a}..{b} out of range (len {})",
                buf.len()
            )));
        }
        let delta = e.bytes.len() as i64 - (b - a) as i64;
        buf.splice(a..b, e.bytes.iter().copied());
        if delta != 0 {
            if let Some(lp) = e.len_prefix {
                let lo = lp - base;
                let v = i32::from_le_bytes(buf[lo..lo + 4].try_into().unwrap());
                buf[lo..lo + 4].copy_from_slice(&(v + delta as i32).to_le_bytes());
            }
            for &f in &e.fixups {
                let fo = f - base;
                let v = i32::from_le_bytes(buf[fo..fo + 4].try_into().unwrap());
                buf[fo..fo + 4].copy_from_slice(&(v + delta as i32).to_le_bytes());
            }
        }
    }
    Ok(())
}

/// 32 uppercase hex chars (FText key format); random like the C#'s GUIDs.
fn gen_text_key() -> String {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let mut out = String::with_capacity(32);
    for _ in 0..16 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.push_str(&format!("{:02X}", (seed & 0xFF) as u8));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{parse_meta_request_json, parse_preset_json};

    const TARGET: &str =
        "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp";

    fn fixture_stem() -> &'static str {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft"
        )
    }

    fn minimal_chimera() -> WingmanMod {
        let json = r#"{
            "version": 1,
            "mods": [{
                "_id": "",
                "_vars": { "aircraftName": "ACG-01X" },
                "assetPatches": {
                    "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp": [{
                        "name": "chimera-min",
                        "patches": [
                            {
                                "description": "clone row",
                                "template": "datatable:[*]",
                                "value": "'ACG-01'>'ACG-01X'",
                                "type": "duplicateEntry"
                            },
                            {
                                "description": "indicator name",
                                "template": "datatable:['ACG-01X'].[0].{'IndicatorName*'}.<TextProperty>",
                                "value": "*:'ACG-01X'",
                                "type": "textProperty"
                            },
                            {
                                "description": "max speed",
                                "template": "datatable:['ACG-01X'].[0].{'BaseStats*'}.{'MaxSpeed*'}.<FloatProperty='2500'>",
                                "value": "FloatProperty:3000",
                                "type": "propertyValue"
                            },
                            {
                                "description": "grow an item",
                                "template": "datatable:['ACG-01X'].[0].{'HardpointCompatibilityList*'}.[[1]]",
                                "value": "StrProperty:'0,saa,mlaa,mlaa2,mlaa3,rgp,mgp,hgp,asm,rdbm,mlag,mlag2,mstm'",
                                "type": "propertyValue"
                            }
                        ]
                    }]
                },
                "_meta": { "displayName": "Chimera (min)" },
                "filePatches": {}
            }]
        }"#;
        parse_preset_json(json).unwrap().mods.remove(0)
    }

    #[test]
    fn minimal_chimera_self_walks() {
        let dt = DataTable::load(fixture_stem()).unwrap();
        let uexp = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft.uexp"
        ))
        .unwrap();
        let uasset = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft.uasset"
        ))
        .unwrap();

        let result = apply_length_changing(&dt, &uexp, &minimal_chimera(), TARGET).unwrap();
        assert_eq!(result.name_append, vec!["ACG-01X".to_string()]);
        assert!(result.uexp_delta > 0);

        // Rewrite the uasset (name append + size updates) and re-walk the pair.
        let new_uasset = modman_uasset::rewrite::rewrite_uasset(
            &uasset,
            &modman_uasset::rewrite::RewritePlan {
                name_append: result.name_append.clone(),
                uexp_delta: result.uexp_delta,
            },
        )
        .unwrap();
        let dt2 = DataTable::walk_bytes(&new_uasset, &result.uexp).unwrap();
        assert_eq!(dt2.leftover, 0, "leftover after apply");
        assert!(dt2.size_mismatches.is_empty(), "{:?}", dt2.size_mismatches);
        assert_eq!(dt2.rows.len(), 40);
        assert_eq!(dt2.rows[39].name, "ACG-01X");
        assert_eq!(dt2.rows[38].name, "AV-8_2");

        // The edits landed on the copy.
        let new_row = &dt2.rows[39];
        let max_speed = new_row
            .props
            .iter()
            .find(|p| p.name.starts_with("BaseStats"))
            .and_then(|p| match &p.value {
                PropValue::Struct { children } => children
                    .iter()
                    .find(|c| c.name.starts_with("MaxSpeed"))
                    .and_then(|c| match &c.value {
                        PropValue::Float(f) => Some(*f),
                        _ => None,
                    }),
                _ => None,
            })
            .unwrap();
        assert_eq!(max_speed, 3000.0);
        let item = new_row
            .props
            .iter()
            .find(|p| p.name.starts_with("HardpointCompatibilityList"))
            .and_then(|p| match &p.value {
                PropValue::Array { items, .. } => Some(items.clone()),
                _ => None,
            })
            .unwrap();
        match &item[1] {
            ArrayElem::Prim {
                value: PropValue::Str(s),
                ..
            } => assert_eq!(
                s,
                "0,saa,mlaa,mlaa2,mlaa3,rgp,mgp,hgp,asm,rdbm,mlag,mlag2,mstm"
            ),
            other => panic!("unexpected item: {other:?}"),
        }
        // The source row is untouched.
        let src = dt2.rows.iter().find(|r| r.name == "ACG-01").unwrap();
        let src_item = src
            .props
            .iter()
            .find(|p| p.name.starts_with("HardpointCompatibilityList"))
            .and_then(|p| match &p.value {
                PropValue::Array { items, .. } => Some(items.clone()),
                _ => None,
            })
            .unwrap();
        match &src_item[1] {
            ArrayElem::Prim {
                value: PropValue::Str(s),
                ..
            } => assert_eq!(s, "0,saa,mlaa,mlaa2,mlaa3,rgp,mgp,hgp,asm,rdbm,mlag,mlag2"),
            other => panic!("unexpected item: {other:?}"),
        }
    }

    /// Full byte-parity vs the banked C# merger output (corpus-gated):
    /// uasset identical; uexp identical modulo the regenerated FText keys.
    #[test]
    fn chimera_matches_oracle() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let corpus = std::path::Path::new(&home)
            .join("modding/project-wingman/sicario-corpus/256-improved-chimera");
        let oracle_dir = std::path::Path::new(&home)
            .join("modding/project-wingman/sicario-oracle/run2-improved-chimera");
        let meta = corpus.join("ImprovedChimera_P-sicario-meta.json");
        if !meta.exists() || !oracle_dir.exists() {
            return;
        }

        let raw = std::fs::read_to_string(&meta).unwrap();
        let req = parse_meta_request_json(&raw).unwrap();
        let modm = &req.request.mods[0];

        let dt = DataTable::load(fixture_stem()).unwrap();
        let uexp = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft.uexp"
        ))
        .unwrap();
        let uasset = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft.uasset"
        ))
        .unwrap();

        let result = apply_length_changing(&dt, &uexp, modm, TARGET).unwrap();
        assert_eq!(result.uexp_delta, 2653, "appended row size");

        let new_uasset = modman_uasset::rewrite::rewrite_uasset(
            &uasset,
            &modman_uasset::rewrite::RewritePlan {
                name_append: result.name_append.clone(),
                uexp_delta: result.uexp_delta,
            },
        )
        .unwrap();
        let oracle_uasset = std::fs::read(oracle_dir.join("oracle-DB_Aircraft.uasset")).unwrap();
        assert!(
            new_uasset == oracle_uasset,
            "uasset differs from oracle ({} vs {} bytes)",
            new_uasset.len(),
            oracle_uasset.len()
        );

        let oracle_uexp = std::fs::read(oracle_dir.join("oracle-DB_Aircraft.uexp")).unwrap();
        assert_eq!(result.uexp.len(), oracle_uexp.len());
        let diffs: Vec<usize> = (0..result.uexp.len())
            .filter(|&i| result.uexp[i] != oracle_uexp[i])
            .collect();
        if !diffs.is_empty() {
            // All diffs must lie in regenerated FText keys (32 hex chars each).
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for &i in &diffs {
                if let Some(last) = ranges.last_mut() {
                    if i <= last.1 + 2 {
                        last.1 = i;
                        continue;
                    }
                }
                ranges.push((i, i));
            }
            assert!(
                ranges.len() <= 3,
                "too many diff ranges: {ranges:?} ({} bytes)",
                diffs.len()
            );
            for (a, b) in &ranges {
                assert!(b - a <= 36, "diff range too large: {a}..{b}");
                let ok = |c: u8| c.is_ascii_digit() || (b'A'..=b'F').contains(&c);
                for (off, (&x, &y)) in result.uexp[*a..=*b]
                    .iter()
                    .zip(oracle_uexp[*a..=*b].iter())
                    .enumerate()
                {
                    assert!(
                        ok(x) && ok(y),
                        "non-key diff byte at {}: {:02x} vs {:02x}",
                        a + off,
                        x,
                        y
                    );
                }
            }
        }
    }
}
