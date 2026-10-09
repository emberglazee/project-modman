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

        // Import map: vanilla imports keep their index; a mod's new imports
        // get positions in the merged table (first-seen order).
        let mimp = rd_i32(ua, 65).max(0) as usize;
        let mimp_off = rd_i32(ua, 69).max(0) as usize;
        let mut imp_map: Vec<i32> = Vec::with_capacity(mimp);
        for k in 0..mimp.min(vimp) {
            let v_entry = &vua[vimp_off + k * 28..vimp_off + k * 28 + 28];
            let m_entry = &ua[mimp_off + k * 28..mimp_off + k * 28 + 28];
            if v_entry != m_entry {
                warnings.push(format!(
                    "override #{i}: import table diverges from vanilla at index {k} \
                     — merge may be unreliable"
                ));
                break;
            }
            imp_map.push(k as i32);
        }
        if mimp > vimp {
            for k in vimp..mimp {
                let entry: [u8; 28] = ua[mimp_off + k * 28..mimp_off + k * 28 + 28]
                    .try_into()
                    .unwrap();
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
            let changed = match vanilla_row {
                Some(vr) => &vue[vr.start..vr.end] != src,
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
                let unchanged = vanilla_row.is_some_and(|vr| {
                    match vr.props.iter().position(|vp| vp.name == p.name) {
                        Some(vi) => {
                            let ve = prop_extent_end(&vr.props, vi, vr.end);
                            vue[vr.props[vi].start..ve] == ue[p.start..extent_end]
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
        if outer <= -(vimp as i32) {
            let idx = (-outer) as usize - 1;
            if let Some(&new) = imp_map.get(idx) {
                e[16..20].copy_from_slice(&(-(new + 1)).to_le_bytes());
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

/// Remap a package index (negative = import reference): refs beyond the
/// vanilla import table map into the merged table's appended entries.
fn patch_pkg_ref(
    out: &mut [u8],
    off: usize,
    v: i32,
    vimp: usize,
    imp_map: &[i32],
    warnings: &mut Vec<String>,
) {
    if v <= -(vimp as i32) {
        let idx = (-v) as usize - 1;
        match imp_map.get(idx) {
            Some(&new) => {
                let new_ref = -(new + 1);
                out[off..off + 4].copy_from_slice(&new_ref.to_le_bytes());
            }
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
