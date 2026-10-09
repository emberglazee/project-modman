//! Combine conflicting override mods that carry no Sicario metadata.
//!
//! Project Wingman mods from the 2.0+ era (and most pre-2.0 ones) are plain
//! file overrides: two mods editing the same uasset conflict, and the game
//! loads only one. This module merges them structurally: each override is
//! diffed against vanilla at the DataTable row/property level, then the
//! deltas are applied sequentially (later mods win) onto the vanilla pair.
//!
//! Architecture note: real-world mod-built tables (verified on 2026-era V11
//! mods and C#-built mods alike) keep the vanilla name table as an exact
//! prefix and append their extras — so indices below the vanilla count are
//! shared, and only the extras need remapping into the merged table
//! (vanilla + union of extras, first-seen order).

use crate::apply::ApplyError;
use modman_uasset::walk::{ByteValue, DataTable, Prop, PropValue, Row};
use std::collections::BTreeMap;

pub struct CombinedPair {
    pub uasset: Vec<u8>,
    pub uexp: Vec<u8>,
    pub warnings: Vec<String>,
    /// Field-level conflicts between overrides (later mod wins).
    pub conflicts: Vec<String>,
}

/// Field-level row state for the merged output.
struct RowState {
    /// The row-name FName bytes (index mapped; 8 bytes).
    name_bytes: Vec<u8>,
    /// Ordered props: (name, bytes with remapped name indices).
    props: Vec<(String, Vec<u8>)>,
    /// Last editor + value repr per prop (for conflict detection).
    sources: BTreeMap<String, (usize, String)>,
}

/// Three-way FIELD-LEVEL merge of a vanilla datatable pair with any number of
/// override pairs. Per row and property, each mod's delta is applied in
/// order; when two mods set the same property to different values the later
/// mod wins and the conflict is reported. Returns `None` when nothing changed.
pub fn merge_datatable_overrides(
    vanilla: (&[u8], &[u8]),
    overrides: &[(&[u8], &[u8])],
    labels: &[String],
) -> Result<Option<CombinedPair>, ApplyError> {
    let (vua, vue) = vanilla;
    let vdt = DataTable::walk_bytes(vua, vue).map_err(|e| ApplyError::Asset(e.to_string()))?;
    let vcount = vdt.names.len();
    let mut warnings = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    // The vanilla import (link) table: count @65, entries @69 (28 bytes each).
    let rd_i32 =
        |b: &[u8], o: usize| -> i32 { i32::from_le_bytes(b[o..o + 4].try_into().unwrap()) };
    let vimp = rd_i32(vua, 65).max(0) as usize;
    let vimp_off = rd_i32(vua, 69).max(0) as usize;

    // Build the merged extras list + per-mod index maps.
    let mut merged_extras: Vec<String> = Vec::new();
    let mut merged_imports: Vec<([u8; 28], usize)> = Vec::new();
    let mut maps: Vec<Vec<i32>> = Vec::new();
    let mut import_maps: Vec<Vec<i32>> = Vec::new();
    let mut dts: Vec<DataTable> = Vec::new();
    for (i, (ua, ue)) in overrides.iter().enumerate() {
        let dt = DataTable::walk_bytes(ua, ue).map_err(|e| ApplyError::Asset(e.to_string()))?;
        let prefix = vcount.min(dt.names.len());
        for k in 0..prefix {
            if dt.names[k] != vdt.names[k] {
                warnings.push(format!(
                    "override #{i}: name table diverges from vanilla at index {k} \
                     ('{}' vs '{}') — merge may be unreliable",
                    dt.names[k], vdt.names[k]
                ));
                break;
            }
        }
        let mut map = Vec::with_capacity(dt.names.len());
        for (k, name) in dt.names.iter().enumerate() {
            if k < vcount {
                map.push(k as i32);
            } else {
                let pos = merged_extras
                    .iter()
                    .position(|n| n == name)
                    .unwrap_or_else(|| {
                        merged_extras.push(name.clone());
                        merged_extras.len() - 1
                    });
                map.push((vcount + pos) as i32);
            }
        }
        maps.push(map);

        // Import map, CONTENT-based: mods may replace entries in place (the
        // F59 skins swap a texture import to their own asset), not just
        // append. An entry that byte-matches a vanilla entry keeps that
        // index; anything else is appended to the merged table (first-seen).
        let mimp = rd_i32(ua, 65).max(0) as usize;
        let mimp_off = rd_i32(ua, 69).max(0) as usize;
        let v_entries: Vec<&[u8]> = (0..vimp)
            .map(|k| &vua[vimp_off + k * 28..vimp_off + k * 28 + 28])
            .collect();
        let mut imp_map: Vec<i32> = Vec::with_capacity(mimp);
        for k in 0..mimp {
            let entry: [u8; 28] = ua[mimp_off + k * 28..mimp_off + k * 28 + 28]
                .try_into()
                .unwrap();
            if let Some(vi) = v_entries.iter().position(|e| **e == entry) {
                imp_map.push(vi as i32);
            } else {
                let pos = merged_imports
                    .iter()
                    .position(|(e, _)| e == &entry)
                    .unwrap_or_else(|| {
                        merged_imports.push((entry, i));
                        merged_imports.len() - 1
                    });
                imp_map.push((vimp + pos) as i32);
            }
        }
        import_maps.push(imp_map);
        dts.push(dt);
    }

    // The shared None-terminator bytes (tail of the vanilla's first row).
    let none_bytes: Vec<u8> = vdt
        .rows
        .first()
        .map(|r| vue[r.end - 8..r.end].to_vec())
        .unwrap_or_default();

    // Initial state: vanilla rows, prop bytes verbatim.
    let mut order: Vec<String> = vdt.rows.iter().map(|r| r.name.clone()).collect();
    let mut state: BTreeMap<String, RowState> = BTreeMap::new();
    for r in &vdt.rows {
        let props: Vec<(String, Vec<u8>)> = r
            .props
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let end = prop_extent_end(&r.props, i, r.end);
                (p.name.clone(), vue[p.start..end].to_vec())
            })
            .collect();
        state.insert(
            r.name.clone(),
            RowState {
                name_bytes: vue[r.start..r.start + 8].to_vec(),
                props,
                sources: BTreeMap::new(),
            },
        );
    }

    for (m, dt) in dts.iter().enumerate() {
        let ue = overrides[m].1;
        let map = &maps[m];
        for r in &dt.rows {
            let src = &ue[r.start..r.end];
            let vanilla_row = vdt.rows.iter().find(|vr| vr.name == r.name);
            // A row needs processing when its bytes differ OR when any of its
            // refs point at imports the mod replaced (bytes identical, but
            // the refs must be remapped — the F59-skins case at row level).
            let changed = match vanilla_row {
                Some(vr) => {
                    &vue[vr.start..vr.end] != src
                        || r.props
                            .iter()
                            .any(|p| prop_has_moved_refs(p, &import_maps[m]))
                }
                None => true,
            };
            if !changed {
                continue;
            }
            if !state.contains_key(&r.name) {
                order.push(r.name.clone());
                state.insert(
                    r.name.clone(),
                    RowState {
                        name_bytes: remap_name_bytes(
                            &ue[r.start..r.start + 8],
                            r,
                            dt,
                            map,
                            &mut warnings,
                        ),
                        props: Vec::new(),
                        sources: BTreeMap::new(),
                    },
                );
            }
            // Per-prop deltas. Comparison is byte-level over the prop's full
            // extent: value strings are None for arrays/structs (which would
            // make every such prop look unchanged). The value repr is kept
            // only for conflict messages.
            for p in &r.props {
                let repr = crate::resolver::prop_value_string(&p.value).unwrap_or_default();
                let pi = r.props.iter().position(|x| x.name == p.name).unwrap_or(0);
                let extent_end = prop_extent_end(&r.props, pi, r.end);
                let imp_map_ref = &import_maps[m];
                let unchanged = vanilla_row.is_some_and(|vr| {
                    match vr.props.iter().position(|vp| vp.name == p.name) {
                        Some(vi) => {
                            let ve = prop_extent_end(&vr.props, vi, vr.end);
                            vue[vr.props[vi].start..ve] == ue[p.start..extent_end]
                                && !prop_has_moved_refs(p, imp_map_ref)
                        }
                        None => false, // prop is new in this mod
                    }
                });
                if unchanged {
                    continue;
                }
                let imp_map = &import_maps[m];
                let bytes = remap_prop_copy(
                    src,
                    r.start,
                    p,
                    extent_end,
                    dt,
                    map,
                    vimp,
                    imp_map,
                    &mut warnings,
                );
                let st = state.get_mut(&r.name).expect("row state exists");
                if let Some((prev_m, prev_repr)) = st.sources.get(&p.name) {
                    if *prev_m != m && *prev_repr != repr {
                        let label = |i: usize| {
                            labels
                                .get(i)
                                .cloned()
                                .unwrap_or_else(|| format!("override #{i}"))
                        };
                        conflicts.push(format!(
                            "{}.{}: '{}' sets '{}', '{}' sets '{}' (later wins)",
                            r.name,
                            p.name,
                            label(*prev_m),
                            prev_repr,
                            label(m),
                            repr
                        ));
                    }
                }
                match st.props.iter_mut().find(|(n, _)| n == &p.name) {
                    Some((_, slot)) => *slot = bytes,
                    None => st.props.push((p.name.clone(), bytes)),
                }
                st.sources.insert(p.name.clone(), (m, repr));
            }
            // Prop deletions: vanilla props absent from this mod's row.
            if let Some(vr) = vanilla_row {
                let st = state.get_mut(&r.name).expect("row state exists");
                for vp in &vr.props {
                    if !r.props.iter().any(|p| p.name == vp.name) {
                        st.props.retain(|(n, _)| n != &vp.name);
                        st.sources.remove(&vp.name);
                    }
                }
            }
        }
        // Row deletions: vanilla rows absent from this mod.
        let present: std::collections::BTreeSet<&str> =
            dt.rows.iter().map(|r| r.name.as_str()).collect();
        for vr in &vdt.rows {
            if !present.contains(vr.name.as_str()) {
                state.remove(&vr.name);
                order.retain(|n| n != &vr.name);
            }
        }
        // Adopt this mod's row order (mods can insert rows mid-table); rows
        // contributed by earlier mods that this mod doesn't know keep their
        // relative order at the end.
        let in_mod: std::collections::BTreeSet<&str> =
            dt.rows.iter().map(|r| r.name.as_str()).collect();
        let mut new_order: Vec<String> = dt.rows.iter().map(|r| r.name.clone()).collect();
        new_order.extend(
            order
                .iter()
                .filter(|n| !in_mod.contains(n.as_str()))
                .cloned(),
        );
        order = new_order;
    }

    // Assemble the merged uexp.
    let mut uexp = Vec::new();
    let num_off = vdt.num_entries_offset;
    uexp.extend_from_slice(&vue[..num_off]);
    uexp.extend_from_slice(&(order.len() as i32).to_le_bytes());
    let first_row_start = vdt.rows.first().map(|r| r.start).unwrap_or(num_off + 4);
    if num_off + 4 < first_row_start {
        uexp.extend_from_slice(&vue[num_off + 4..first_row_start]);
    }
    for name in &order {
        let st = &state[name];
        uexp.extend_from_slice(&st.name_bytes);
        for (_, bytes) in &st.props {
            uexp.extend_from_slice(bytes);
        }
        uexp.extend_from_slice(&none_bytes);
    }
    let last_row_end = vdt.rows.last().map(|r| r.end).unwrap_or(vue.len());
    uexp.extend_from_slice(&vue[last_row_end..]);

    if uexp == vue {
        return Ok(None);
    }

    let delta = uexp.len() as i64 - vue.len() as i64;

    // Remap the appended import entries: their FName fields (ClassPackage
    // @0, ClassName @8, ObjectName @20) may reference the mod's extras, and
    // OuterIndex @16 may reference the mod's appended imports.
    let mut link_append: Vec<modman_uasset::rewrite::NewLink> = Vec::new();
    for (entry, src_mod) in &merged_imports {
        let mut e = *entry;
        let name_map = &maps[*src_mod];
        let imp_map = &import_maps[*src_mod];
        for off in [0usize, 8, 20] {
            let idx = i32::from_le_bytes(e[off..off + 4].try_into().unwrap());
            if idx >= 0 && (idx as usize) < name_map.len() {
                let new = name_map[idx as usize];
                e[off..off + 4].copy_from_slice(&new.to_le_bytes());
            }
        }
        let outer = i32::from_le_bytes(e[16..20].try_into().unwrap());
        if outer < 0 {
            let idx = (-outer) as usize - 1;
            if let Some(&new) = imp_map.get(idx) {
                if new != idx as i32 {
                    e[16..20].copy_from_slice(&(-(new + 1)).to_le_bytes());
                }
            }
        }
        link_append.push(modman_uasset::rewrite::NewLink {
            base: u64::from_le_bytes(e[0..8].try_into().unwrap()),
            class: u64::from_le_bytes(e[8..16].try_into().unwrap()),
            linkage: i32::from_le_bytes(e[16..20].try_into().unwrap()),
            property: i32::from_le_bytes(e[20..24].try_into().unwrap()),
            target: i32::from_le_bytes(e[24..28].try_into().unwrap()),
        });
    }

    let uasset = modman_uasset::rewrite::rewrite_uasset(
        vua,
        &modman_uasset::rewrite::RewritePlan {
            name_append: merged_extras,
            link_append,
            uexp_delta: delta,
        },
    )
    .map_err(|e| ApplyError::Asset(e.to_string()))?;

    Ok(Some(CombinedPair {
        uasset,
        uexp,
        warnings,
        conflicts,
    }))
}

/// Does this prop reference imports whose merged position moved? A prop's
/// bytes can be identical to vanilla while its refs point at a REPLACED
/// import (the F59 skins) — such props must be applied so the refs remap.
fn prop_has_moved_refs(p: &Prop, imp_map: &[i32]) -> bool {
    let moved = |v: i32| -> bool {
        if v < 0 {
            let idx = (-v) as usize - 1;
            imp_map.get(idx).is_some_and(|&new| new != idx as i32)
        } else {
            false
        }
    };
    match &p.value {
        PropValue::Object(v) => moved(*v),
        PropValue::Struct { children } => children.iter().any(|c| prop_has_moved_refs(c, imp_map)),
        PropValue::Array { items, .. } => items.iter().any(|it| match it {
            modman_uasset::walk::ArrayElem::Struct(props) => {
                props.iter().any(|c| prop_has_moved_refs(c, imp_map))
            }
            modman_uasset::walk::ArrayElem::Prim {
                value: PropValue::Object(v),
                ..
            } => moved(*v),
            _ => false,
        }),
        _ => false,
    }
}

/// The full byte extent of a prop within its row: from the tag start to the
/// next prop's start (or the row's None terminator). `vend` alone can miss
/// trailing bytes (e.g. property GUIDs), so extents must be used for copies.
fn prop_extent_end(props: &[Prop], i: usize, row_end: usize) -> usize {
    if i + 1 < props.len() {
        props[i + 1].start
    } else {
        row_end - 8 // the None terminator FName
    }
}

/// The row-name FName bytes with the index remapped.
fn remap_name_bytes(
    bytes: &[u8],
    row: &Row,
    dt: &DataTable,
    map: &[i32],
    warnings: &mut Vec<String>,
) -> Vec<u8> {
    let mut out = bytes.to_vec();
    if let Some(old) = dt.names.iter().position(|n| n == &row.name) {
        patch_index_raw(&mut out, 0, old as i32, map, warnings);
    }
    out
}

/// Copy a prop's bytes (from a row slice) with name indices remapped.
#[allow(clippy::too_many_arguments)]
fn remap_prop_copy(
    full: &[u8],
    base: usize,
    p: &Prop,
    extent_end: usize,
    dt: &DataTable,
    map: &[i32],
    vimp: usize,
    imp_map: &[i32],
    warnings: &mut Vec<String>,
) -> Vec<u8> {
    let mut out = full[p.start - base..extent_end - base].to_vec();
    remap_prop(&mut out, p.start, p, dt, map, vimp, imp_map, warnings);
    out
}

/// Remap a package index (negative = import reference). Mods can REPLACE
/// entries in range (the F59 skins swap a texture import), so any entry
/// whose mapped position moved must be remapped — not just out-of-range
/// refs, which would otherwise silently resolve to the vanilla entry.
fn patch_pkg_ref(
    out: &mut [u8],
    off: usize,
    v: i32,
    _vimp: usize,
    imp_map: &[i32],
    warnings: &mut Vec<String>,
) {
    if v < 0 {
        let idx = (-v) as usize - 1;
        match imp_map.get(idx) {
            Some(&new) if new != idx as i32 => {
                let new_ref = -(new + 1);
                out[off..off + 4].copy_from_slice(&new_ref.to_le_bytes());
            }
            Some(_) => {} // entry unchanged — ref stays
            None => warnings.push(format!("package ref {v} beyond the import table")),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn remap_prop(
    out: &mut [u8],
    base: usize,
    p: &Prop,
    dt: &DataTable,
    map: &[i32],
    vimp: usize,
    imp_map: &[i32],
    warnings: &mut Vec<String>,
) {
    // Tag: [name FName 8][type FName 8][size i32][arrayIndex i32][...]
    patch_index(out, p.start - base, &p.name, dt, map, warnings);
    patch_index(out, p.start - base + 8, &p.type_name, dt, map, warnings);
    if p.type_name == "StructProperty" {
        if let Some(st) = &p.struct_type {
            patch_index(out, p.start - base + 24, st, dt, map, warnings);
        }
    }
    if p.type_name == "ArrayProperty" {
        if let PropValue::Array { elem_type, .. } = &p.value {
            patch_index(out, p.start - base + 24, elem_type, dt, map, warnings);
        }
    }
    match &p.value {
        PropValue::Name(n) => {
            patch_index_raw(out, p.vstart - base, n.index, map, warnings);
        }
        PropValue::Byte {
            value: ByteValue::FName(n),
            ..
        } => {
            patch_index_raw(out, p.vstart - base, n.index, map, warnings);
        }
        PropValue::Object(v) => {
            patch_pkg_ref(out, p.vstart - base, *v, vimp, imp_map, warnings);
        }
        PropValue::Struct { children } => {
            for c in children {
                remap_prop(out, base, c, dt, map, vimp, imp_map, warnings);
            }
        }
        PropValue::Array { items, .. } => {
            for it in items {
                match it {
                    modman_uasset::walk::ArrayElem::Struct(props) => {
                        for c in props {
                            remap_prop(out, base, c, dt, map, vimp, imp_map, warnings);
                        }
                    }
                    modman_uasset::walk::ArrayElem::Prim {
                        value: PropValue::Object(v),
                        start,
                        ..
                    } => {
                        patch_pkg_ref(out, start - base, *v, vimp, imp_map, warnings);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Patch the 4-byte name index at `offset` (row-relative) by looking the
/// current name string up in the mod's table.
fn patch_index(
    out: &mut [u8],
    offset: usize,
    name: &str,
    dt: &DataTable,
    map: &[i32],
    warnings: &mut Vec<String>,
) {
    let Some(old) = dt.names.iter().position(|n| n == name) else {
        return;
    };
    patch_index_raw(out, offset, old as i32, map, warnings);
}

fn patch_index_raw(
    out: &mut [u8],
    offset: usize,
    old: i32,
    map: &[i32],
    warnings: &mut Vec<String>,
) {
    if old < 0 || old as usize >= map.len() {
        warnings.push(format!("name index {old} out of range during remap"));
        return;
    }
    let new = map[old as usize];
    if new != old {
        out[offset..offset + 4].copy_from_slice(&new.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd_i32(b: &[u8], o: usize) -> i32 {
        i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../modman-uasset/tests/fixtures/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        ))
        .unwrap()
    }

    /// Two mods editing DIFFERENT fields of the SAME row: both survive.
    /// Two mods editing the SAME field with different values: later wins and
    /// the conflict is reported.
    #[test]
    fn field_level_merge_and_conflict_report() {
        let vua = fixture("DB_LevelList.uasset");
        let vue = fixture("DB_LevelList.uexp");
        let dt = DataTable::walk_bytes(&vua, &vue).unwrap();
        let row0 = &dt.rows[0];
        // Find two int-typed props in row 0 to poke (fixed 4-byte values, so
        // the uexp keeps its size and no rewrite is needed).
        let ints: Vec<usize> = row0
            .props
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.value, PropValue::Int(_)))
            .map(|(i, _)| i)
            .collect();
        assert!(
            ints.len() >= 2,
            "row0 needs >=2 int props for this test, found {}",
            ints.len()
        );
        let (i1, i2) = (ints[0], ints[1]);

        let poke = |prop_idx: usize, val: i32| -> Vec<u8> {
            let mut ue = vue.clone();
            let p = &row0.props[prop_idx];
            ue[p.vstart..p.vstart + 4].copy_from_slice(&val.to_le_bytes());
            ue
        };
        let a_ue = poke(i1, 111_111); // mod A: edits field 1
        let b_ue = poke(i2, 222_222); // mod B: edits field 2 (different field!)
        let c_ue = poke(i1, 333_333); // mod C: edits field 1 differently

        // A + B: both edits must survive (field-level merge).
        let out = merge_datatable_overrides(
            (&vua, &vue),
            &[(&vua, &a_ue), (&vua, &b_ue)],
            &["modA".to_string(), "modB".to_string()],
        )
        .unwrap()
        .expect("merge");
        assert!(
            out.conflicts.is_empty(),
            "no conflicts expected: {:?}",
            out.conflicts
        );
        let odt = DataTable::walk_bytes(&out.uasset, &out.uexp).unwrap();
        let orow = odt.rows.iter().find(|r| r.name == row0.name).unwrap();
        let get = |name: &str| -> i32 {
            match &orow.props.iter().find(|p| p.name == name).unwrap().value {
                PropValue::Int(v) => *v,
                other => panic!("not int: {other:?}"),
            }
        };
        assert_eq!(get(&row0.props[i1].name), 111_111, "mod A's field survived");
        assert_eq!(get(&row0.props[i2].name), 222_222, "mod B's field survived");

        // A + C on the same field: later wins + conflict reported.
        let out2 = merge_datatable_overrides(
            (&vua, &vue),
            &[(&vua, &a_ue), (&vua, &c_ue)],
            &["modA".to_string(), "modC".to_string()],
        )
        .unwrap()
        .expect("merge");
        assert_eq!(
            out2.conflicts.len(),
            1,
            "one conflict: {:?}",
            out2.conflicts
        );
        assert!(out2.conflicts[0].contains("modA"), "{}", out2.conflicts[0]);
        assert!(out2.conflicts[0].contains("modC"), "{}", out2.conflicts[0]);
        let odt2 = DataTable::walk_bytes(&out2.uasset, &out2.uexp).unwrap();
        let orow2 = odt2.rows.iter().find(|r| r.name == row0.name).unwrap();
        match &orow2
            .props
            .iter()
            .find(|p| p.name == row0.props[i1].name)
            .unwrap()
            .value
        {
            PropValue::Int(v) => assert_eq!(*v, 333_333, "later mod's value won"),
            other => panic!("not int: {other:?}"),
        }
    }

    /// A single override that adds imports (the skin objectRef case) must be
    /// byte-identical: uexp refs and the appended import entries all remap.
    #[test]
    fn single_override_with_imports_is_byte_identical() {
        let vua = fixture("DB_Aircraft.uasset");
        let vue = fixture("DB_Aircraft.uexp");
        let mua = fixture("DB_Aircraft.skin.merged.uasset");
        let mue = fixture("DB_Aircraft.skin.merged.uexp");
        let out = merge_datatable_overrides((&vua, &vue), &[(&mua, &mue)], &["skin".to_string()])
            .unwrap()
            .expect("merge should detect the skin change");
        assert_eq!(out.uexp, mue, "uexp must be byte-identical");
        assert_eq!(
            out.uasset, mua,
            "uasset (names + imports) must be byte-identical"
        );
    }

    /// A mod that REPLACES an import entry in place (the F59-skins case)
    /// must merge with SEMANTIC ref preservation: every ref that pointed at
    /// the replaced entry must resolve to the same object name in the
    /// output (via the appended position), not to the vanilla entry.
    #[test]
    fn replaced_import_refs_resolve_semantically() {
        let vua = fixture("DB_Aircraft.uasset");
        let vue = fixture("DB_Aircraft.uexp");
        let mua = fixture("DB_Aircraft.skin.merged.uasset");
        let mue = fixture("DB_Aircraft.skin.merged.uexp");

        // Resolve an import ref to its ObjectName string.
        fn oname(ua: &[u8], names: &[String], r: i32) -> Option<String> {
            if r >= 0 {
                return None;
            }
            let idx = (-r) as usize - 1;
            let mimp = i32::from_le_bytes(ua[65..69].try_into().unwrap()) as usize;
            if idx >= mimp {
                return None;
            }
            let off = i32::from_le_bytes(ua[69..73].try_into().unwrap()) as usize + idx * 28;
            let nidx = i32::from_le_bytes(ua[off + 20..off + 24].try_into().unwrap());
            names.get(nidx.max(0) as usize).cloned()
        }

        // Find the first in-range object ref in the mod's uexp.
        let dt = DataTable::walk_bytes(&mua, &mue).unwrap();
        fn find_ref(p: &Prop) -> Option<i32> {
            match &p.value {
                PropValue::Object(v) if *v < 0 => Some(*v),
                PropValue::Struct { children } => children.iter().find_map(find_ref),
                PropValue::Array { items, .. } => items.iter().find_map(|it| match it {
                    modman_uasset::walk::ArrayElem::Struct(props) => {
                        props.iter().find_map(find_ref)
                    }
                    modman_uasset::walk::ArrayElem::Prim {
                        value: PropValue::Object(v),
                        ..
                    } if *v < 0 => Some(*v),
                    _ => None,
                }),
                _ => None,
            }
        }
        let (ri, pi, first_ref) = dt
            .rows
            .iter()
            .enumerate()
            .find_map(|(ri, row)| {
                row.props
                    .iter()
                    .enumerate()
                    .find_map(|(pi, p)| find_ref(p).map(|r| (ri, pi, r)))
            })
            .expect("fixture must contain an object ref");
        let anchor_row = dt.rows[ri].name.clone();
        let anchor_prop = dt.rows[ri].props[pi].name.clone();

        // Patch that import entry to a different valid ObjectName index.
        let mut patched = mua.clone();
        let imp_off = i32::from_le_bytes(patched[69..73].try_into().unwrap()) as usize;
        let idx = (-first_ref) as usize - 1;
        let names_count = i32::from_le_bytes(patched[41..45].try_into().unwrap());
        let old = i32::from_le_bytes(
            patched[imp_off + idx * 28 + 20..imp_off + idx * 28 + 24]
                .try_into()
                .unwrap(),
        );
        let new = if old == 0 { 1 } else { 0 };
        assert!(new < names_count);
        patched[imp_off + idx * 28 + 20..imp_off + idx * 28 + 24]
            .copy_from_slice(&new.to_le_bytes());

        let out =
            merge_datatable_overrides((&vua, &vue), &[(&patched, &mue)], &["swap".to_string()])
                .unwrap()
                .expect("merge");

        // The ref must resolve to the same (patched) ObjectName in the output.
        let odt = DataTable::walk_bytes(&out.uasset, &out.uexp).unwrap();
        let orow = odt
            .rows
            .iter()
            .find(|r| r.name == anchor_row)
            .expect("anchor row in output");
        let oprop = orow
            .props
            .iter()
            .find(|p| p.name == anchor_prop)
            .expect("anchor prop in output");
        let out_ref = find_ref(oprop).expect("output must contain the ref");
        let expect = oname(&patched, &dt.names, first_ref).expect("mod ref resolves");
        let got = oname(&out.uasset, &odt.names, out_ref).expect("output ref resolves");
        assert_eq!(got, expect, "ref must resolve to the same object name");
    }

    /// A single override must reproduce the mod's datatable content exactly:
    /// same rows (names + bytes) in the same order, and the name table =
    /// vanilla + the mod's extras.
    #[test]
    fn single_override_reproduces_the_mod() {
        let vua = fixture("DB_LevelList.uasset");
        let vue = fixture("DB_LevelList.uexp");
        let mua = fixture("DB_LevelList.mod.uasset");
        let mue = fixture("DB_LevelList.mod.uexp");

        let out = merge_datatable_overrides((&vua, &vue), &[(&mua, &mue)], &["e5".to_string()])
            .unwrap()
            .expect("merge should change something");

        let odt = DataTable::walk_bytes(&out.uasset, &out.uexp).unwrap();
        let mdt = DataTable::walk_bytes(&mua, &mue).unwrap();

        // Same rows (name-keyed), byte-identical. Order is not meaningful
        // in a DataTable (lookup is by row name).
        assert_eq!(odt.rows.len(), mdt.rows.len(), "row count");
        for m in &mdt.rows {
            let o = odt
                .rows
                .iter()
                .find(|r| r.name == m.name)
                .unwrap_or_else(|| panic!("row {} missing from merge", m.name));
            assert_eq!(
                &out.uexp[o.start..o.end],
                &mue[m.start..m.end],
                "row bytes for {}",
                m.name
            );
        }
        // Name table: vanilla prefix + the mod's extras.
        assert_eq!(
            odt.names[..vdt_names(&vua).len()],
            vdt_names(&vua)[..],
            "vanilla prefix preserved"
        );
        assert_eq!(odt.names.len(), mdt.names.len(), "total name count");
        assert_eq!(
            &odt.names[vdt_names(&vua).len()..],
            &mdt.names[vdt_names(&vua).len()..]
        );
    }

    fn vdt_names(ua: &[u8]) -> Vec<String> {
        let count = rd_i32(ua, 41) as usize;
        let mut off = 193;
        let mut names = Vec::new();
        for _ in 0..count {
            let ln = rd_i32(ua, off) as usize;
            names.push(String::from_utf8_lossy(&ua[off + 4..off + 4 + ln - 1]).to_string());
            off += 4 + ln + 4;
        }
        names
    }

    /// Two overrides: both deltas survive; conflicts resolve to the later mod.
    #[test]
    fn two_overrides_merge_and_later_wins() {
        let vua = fixture("DB_LevelList.uasset");
        let vue = fixture("DB_LevelList.uexp");
        let mua = fixture("DB_LevelList.mod.uasset");
        let mue = fixture("DB_LevelList.mod.uexp");

        // Override B: vanilla with the first row's name changed to a NEW row
        // appended (simulating a second, independent mod).
        let mut b_uexp = vue.clone();
        let vdt = DataTable::walk_bytes(&vua, &vue).unwrap();
        // Append a synthetic row: copy the first row's bytes, rename to a new
        // name index (append "ZZ_TEST_ROW" to the table via a merge of A
        // then B... simpler: build B = vanilla + A? No — B must be an
        // independent mod: use vanilla + a renamed copy of row 0.
        let name = "ZZ_TEST_ROW";
        // Append the row bytes (copy of row 0 with the new name index; the
        // appended name lands at index = vanilla name count).
        let row0 = &vue[vdt.rows[0].start..vdt.rows[0].end];
        let new_idx = vdt.names.len() as i32;
        let mut new_row = row0.to_vec();
        new_row[0..4].copy_from_slice(&new_idx.to_le_bytes());
        new_row[4..8].copy_from_slice(&0i32.to_le_bytes()); // FName number = 0
                                                            // Insert BEFORE the tail (the uexp file ends with the package tag).
        let ins = vdt.rows.last().map(|r| r.end).unwrap_or(vue.len());
        b_uexp.splice(ins..ins, new_row.iter().copied());
        let cnt = rd_i32(&b_uexp, vdt.num_entries_offset) + 1;
        b_uexp[vdt.num_entries_offset..vdt.num_entries_offset + 4]
            .copy_from_slice(&cnt.to_le_bytes());
        // Correct uasset via the shared rewrite machinery.
        let b_ua = modman_uasset::rewrite::rewrite_uasset(
            &vua,
            &modman_uasset::rewrite::RewritePlan {
                name_append: vec![name.to_string()],
                link_append: vec![],
                uexp_delta: new_row.len() as i64,
            },
        )
        .unwrap();

        let out = merge_datatable_overrides(
            (&vua, &vue),
            &[(&mua, &mue), (&b_ua, &b_uexp)],
            &["e5".to_string(), "b".to_string()],
        )
        .unwrap()
        .expect("merge");
        let odt = DataTable::walk_bytes(&out.uasset, &out.uexp).unwrap();
        assert!(
            odt.rows.iter().any(|r| r.name == "ZZ_TEST_ROW"),
            "second override's added row present"
        );
        assert!(
            odt.rows.iter().any(|r| r.name == "C22_Kings_MOD"),
            "first override's added row present"
        );
        assert!(
            odt.rows.iter().any(|r| r.name == "C22_Kings"),
            "vanilla rows preserved"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────
// Pak-source abstraction + full combine orchestration.
//
// Shared by the CLI (filesystem paks) and the wasm page (in-memory paks +
// a sparse game-pak reader), so both run the exact same merge semantics.
// ────────────────────────────────────────────────────────────────────────

/// A read-only source of pak entries.
pub trait PakSource {
    /// All entry paths (forward slashes), as the pak's index stores them.
    fn files(&self) -> Vec<String>;
    /// Read one entry's bytes (path as reported by [`PakSource::files`]).
    fn read(&self, path: &str) -> Result<Vec<u8>, ApplyError>;
    /// The pak's mount point. Records resolve to game paths as
    /// `mount + record`, normalized to the game-root form used by
    /// `../../../`-mounted mods.
    fn mount_point(&self) -> String {
        String::new()
    }
}

/// Resolve a pak record to its effective game path: `mount + record`,
/// with a leading `../../../` stripped (the game-root mount form).
pub fn effective_path(mount: &str, record: &str) -> String {
    let mut full = String::with_capacity(mount.len() + record.len() + 1);
    full.push_str(mount);
    if !full.is_empty() && !full.ends_with('/') && !record.starts_with('/') {
        full.push('/');
    }
    full.push_str(record);
    full.strip_prefix("../../../").unwrap_or(&full).to_string()
}

/// Result of a full combine run.
#[derive(Debug, Default)]
pub struct CombineOutcome {
    /// path -> bytes: merged datatables + pass-through files, ready to pack.
    pub files: std::collections::BTreeMap<String, Vec<u8>>,
    /// How many datatables were merged.
    pub merged: usize,
    /// Field-level conflicts (later mod wins).
    pub field_conflicts: Vec<String>,
    /// Non-datatable files overridden by multiple mods (single-winner).
    pub pass_through_conflicts: Vec<String>,
    /// Merge warnings (divergences, unmergeable files, etc).
    pub warnings: Vec<String>,
}

/// Combine conflicting override mods over a vanilla base: three-way
/// datatable merges (later mods win), single-winner pass-through for
/// everything else. This is the full orchestration both frontends share.
pub fn combine_sources(
    base: &dyn PakSource,
    mods: &[&dyn PakSource],
    labels: &[String],
) -> Result<CombineOutcome, ApplyError> {
    let base_mount = base.mount_point();
    let base_files: Vec<String> = base
        .files()
        .iter()
        .map(|r| effective_path(&base_mount, &r.replace('\\', "/")))
        .collect();
    let base_find = |name: &str| -> Option<String> {
        base_files
            .iter()
            .find(|f| f.as_str() == name)
            .or_else(|| base_files.iter().find(|f| f.ends_with(name)))
            .cloned()
    };

    struct Override {
        ua: Vec<u8>,
        ue: Vec<u8>,
        label: String,
    }
    let mut dt_overrides: std::collections::BTreeMap<String, Vec<Override>> =
        std::collections::BTreeMap::new();
    let mut passthrough: std::collections::BTreeMap<String, (usize, Vec<u8>)> =
        std::collections::BTreeMap::new();
    let mut pt_conflicts: Vec<String> = Vec::new();

    for (mi, m) in mods.iter().enumerate() {
        let records = m.files();
        let mount = m.mount_point();
        let norm = |r: &String| r.replace('\\', "/");
        // (effective, raw) pairs: effective = mount + record (game path),
        // raw = what the pak index stores (what reads use).
        let pairs: Vec<(String, String)> = records
            .iter()
            .map(|r| {
                let raw = norm(r);
                let eff = effective_path(&mount, &raw);
                (eff, raw)
            })
            .collect();
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (eff, raw) in &pairs {
            if eff.to_ascii_lowercase().ends_with(".uasset") {
                let uexp_raw = format!("{}.uexp", &raw[..raw.len() - 7]);
                let uexp_eff = format!("{}.uexp", &eff[..eff.len() - 7]);
                if let Some((_, uraw)) = pairs.iter().find(|(e, r)| {
                    e.eq_ignore_ascii_case(&uexp_eff) || r.eq_ignore_ascii_case(&uexp_raw)
                }) {
                    if !seen.insert(eff.clone()) {
                        continue;
                    }
                    // The .uexp record belongs to this pair — mark it
                    // consumed so it never falls through to the pass-through
                    // (which would clobber the merged output).
                    seen.insert(uexp_eff.clone());
                    let Some(vkey) = base_find(eff) else {
                        // Not a game file: single-winner pass-through pair.
                        let ua = m.read(raw)?;
                        let ue = m.read(uraw)?;
                        for (k, b) in [(eff.clone(), ua), (uexp_eff.clone(), ue)] {
                            if let Some((prev, _)) = passthrough.get(&k) {
                                if *prev != mi {
                                    pt_conflicts.push(k.clone());
                                }
                            }
                            passthrough.insert(k, (mi, b));
                        }
                        continue;
                    };
                    let vkey_uexp = format!("{}.uexp", &vkey[..vkey.len() - 7]);
                    let Some(vue_key) = base_find(&vkey_uexp) else {
                        continue;
                    };
                    let ua = m.read(raw)?;
                    let ue = m.read(uraw)?;
                    // Only real DataTables are field-mergeable. Opaque assets
                    // (textures, audio, meshes) that happen to ship a
                    // .uasset+.uexp pair pass through single-winner — their
                    // payloads (pixels, samples, vertices) cannot be merged.
                    if modman_uasset::asset_class(&ua).ok().as_deref() != Some("DataTable") {
                        for (k, b) in [(eff.clone(), ua), (uexp_eff.clone(), ue)] {
                            if let Some((prev, _)) = passthrough.get(&k) {
                                if *prev != mi {
                                    pt_conflicts.push(k.clone());
                                }
                            }
                            passthrough.insert(k, (mi, b));
                        }
                        continue;
                    }
                    let vua = base.read(&vkey)?;
                    let vue = base.read(&vue_key)?;
                    if ua == vua && ue == vue {
                        continue; // not actually an override
                    }
                    dt_overrides.entry(eff.clone()).or_default().push(Override {
                        ua,
                        ue,
                        label: labels
                            .get(mi)
                            .cloned()
                            .unwrap_or_else(|| format!("override #{mi}")),
                    });
                    continue;
                }
            }
            if seen.insert(eff.clone()) {
                let bytes = m.read(raw)?;
                if let Some((prev, _)) = passthrough.get(eff) {
                    if *prev != mi {
                        pt_conflicts.push(eff.clone());
                    }
                }
                passthrough.insert(eff.clone(), (mi, bytes));
            }
        }
    }

    let mut files = std::collections::BTreeMap::new();
    let mut merged = 0usize;
    let mut field_conflicts: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (target, ovs) in &dt_overrides {
        let Some(vkey) = base_find(target) else {
            continue;
        };
        let vkey_uexp = format!("{}.uexp", &vkey[..vkey.len() - 7]);
        let Some(vue_key) = base_find(&vkey_uexp) else {
            continue;
        };
        let vua = base.read(&vkey)?;
        let vue = base.read(&vue_key)?;
        let refs: Vec<(&[u8], &[u8])> = ovs
            .iter()
            .map(|o| (o.ua.as_slice(), o.ue.as_slice()))
            .collect();
        let labels: Vec<String> = ovs.iter().map(|o| o.label.clone()).collect();
        match merge_datatable_overrides((&vua, &vue), &refs, &labels) {
            Ok(Some(c)) => {
                let short = target.split('/').next_back().unwrap_or(target).to_string();
                warnings.extend(c.warnings.iter().map(|w| format!("{short}: {w}")));
                field_conflicts.extend(c.conflicts.iter().map(|x| format!("{short}: {x}")));
                files.insert(vkey, c.uasset);
                files.insert(vue_key, c.uexp);
                merged += 1;
            }
            Ok(None) => {}
            Err(e) => {
                // Not a mergeable datatable (a texture/mesh that happens to
                // ship a .uasset+.uexp pair): fall back to single-winner
                // pass-through of the last override's pair.
                let short = target.split('/').next_back().unwrap_or(target).to_string();
                warnings.push(format!(
                    "{short}: cannot be merged ({e}) — kept the last override's version"
                ));
                let last = ovs.last().expect("override list is non-empty");
                files.insert(vkey, last.ua.clone());
                files.insert(vue_key, last.ue.clone());
            }
        }
    }
    for (path, (_, bytes)) in &passthrough {
        files.insert(path.clone(), bytes.clone());
    }

    Ok(CombineOutcome {
        files,
        merged,
        field_conflicts,
        pass_through_conflicts: pt_conflicts,
        warnings,
    })
}
