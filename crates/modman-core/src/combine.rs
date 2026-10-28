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
use modman_uasset::walk::{ArrayElem, ByteValue, DataTable, Prop, PropValue};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The combine output filename. A `combine` input carrying this name is a
/// previous run's output, never a fresh mod — merging it back in shifts every
/// import index, so each vanilla row reads as "changed" and the report floods
/// with phantom conflicts. Both frontends' outputs share this name, so the
/// core orchestration owns the constant (and the skip).
pub const COMBINE_OUTPUT_NAME: &str = "SicarioCombine_P.pak";

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
    /// Last editor + semantic conflict key + display repr per prop.
    sources: BTreeMap<String, (usize, String, String)>,
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
    // Row lookup is by name, once per row instead of once per mod row.
    let mut vanilla_rows = std::collections::HashMap::with_capacity(vdt.rows.len());
    for r in &vdt.rows {
        vanilla_rows.entry(r.name.as_str()).or_insert(r);
    }
    let mut warnings = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    // The vanilla import (link) table: count @65, entries @69 (28 bytes each).
    let rd_i32 = |b: &[u8], o: usize| -> Option<i32> {
        b.get(o..o + 4)
            .and_then(|s| s.try_into().ok())
            .map(i32::from_le_bytes)
    };
    let header_i32 = |b: &[u8], o: usize, what: &str| -> Result<i32, ApplyError> {
        rd_i32(b, o)
            .ok_or_else(|| ApplyError::Asset(format!("corrupt datatable header: {what} missing")))
    };
    let vimp = header_i32(vua, 65, "import count")?.max(0) as usize;
    let vimp_off = header_i32(vua, 69, "import offset")?.max(0) as usize;
    if vimp_off
        .checked_add(vimp.saturating_mul(28))
        .is_none_or(|end| end > vua.len())
    {
        return Err(ApplyError::Asset(
            "corrupt datatable import table".to_string(),
        ));
    }

    // Build the merged extras list + per-mod index maps. Name mapping is
    // STRING-based at every index: mods cooked against a different base
    // carry a wholly different table order (DB_MF: `A-10/A-10` @0 vs
    // vanilla's `f15_icon` @0), so positional identity silently re-points
    // prop tags and FNames at wrong strings — an unwalkable, crashing
    // table. A name present in vanilla keeps its vanilla index; anything
    // else appends first-seen (same rule as the content-based import map).
    let mut vanilla_by_name: std::collections::HashMap<&str, i32> =
        std::collections::HashMap::with_capacity(vcount);
    for (k, name) in vdt.names.iter().enumerate() {
        vanilla_by_name.entry(name.as_str()).or_insert(k as i32);
    }
    let mut merged_extras: Vec<String> = Vec::new();
    let mut merged_imports: Vec<([u8; 28], usize)> = Vec::new();
    let mut maps: Vec<Vec<i32>> = Vec::new();
    let mut import_maps: Vec<Vec<i32>> = Vec::new();
    let mut dts: Vec<DataTable> = Vec::new();
    for (i, (ua, ue)) in overrides.iter().enumerate() {
        let dt = DataTable::walk_bytes(ua, ue).map_err(|e| ApplyError::Asset(e.to_string()))?;
        let map = map_names(&vanilla_by_name, vcount, &dt.names, &mut merged_extras);
        maps.push(map);

        // Import map, CONTENT-based: mods may replace entries in place (the
        // F59 skins swap a texture import to their own asset), not just
        // append. An entry that byte-matches a vanilla entry keeps that
        // index; anything else is appended to the merged table (first-seen).
        let mimp = header_i32(ua, 65, "import count")?.max(0) as usize;
        let mimp_off = header_i32(ua, 69, "import offset")?.max(0) as usize;
        if mimp_off
            .checked_add(mimp.saturating_mul(28))
            .is_none_or(|end| end > ua.len())
        {
            return Err(ApplyError::Asset(
                "corrupt datatable import table".to_string(),
            ));
        }
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
            let vanilla_row = vanilla_rows.get(r.name.as_str()).copied();
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
            // Fast path only while the row is still IN the merged state.
            // An earlier mod's deletion pass can remove a row that this mod
            // re-carries unchanged; skipping it there would leave `order`
            // naming a row `state` no longer holds (assembly indexes
            // `state[name]` and panics), and the table would lose a row the
            // mod ships.
            if !changed && state.contains_key(&r.name) {
                continue;
            }
            if !state.contains_key(&r.name) {
                order.push(r.name.clone());
                // A row deleted by an earlier mod's deletion pass, then
                // re-added here, must be seeded with the VANILLA props:
                // the per-prop loop below skips vanilla-identical props,
                // and an empty seed would drop them mid-row — shifting the
                // stream and producing an unwalkable table (the DB_MF
                // Import_Create crash: FE15's 7-row table deletes rows that
                // SK35S re-adds). Truly new rows (no vanilla counterpart)
                // start empty; every prop then counts as changed.
                let seed: Vec<(String, Vec<u8>)> = vanilla_row
                    .map(|vr| {
                        vr.props
                            .iter()
                            .enumerate()
                            .map(|(i, vp)| {
                                let ve = prop_extent_end(&vr.props, i, vr.end);
                                (vp.name.clone(), vue[vp.start..ve].to_vec())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                state.insert(
                    r.name.clone(),
                    RowState {
                        name_bytes: remap_name_bytes(&ue[r.start..r.start + 8], map, &mut warnings),
                        props: seed,
                        sources: BTreeMap::new(),
                    },
                );
            }
            // Per-prop deltas. Comparison is byte-level over the prop's full
            // extent: value strings are None for arrays/structs (which would
            // make every such prop look unchanged). Conflict comparison uses
            // the semantic key (object refs resolved to the asset they name)
            // while messages show the raw repr — index shifts from appended
            // imports must not read as conflicts.
            for p in &r.props {
                // Display value (readable summary) vs conflict key
                // (semantic repr — refs resolved to the asset they name).
                let raw = display_repr(overrides[m].0, &dt.names, p);
                let key = semantic_repr(overrides[m].0, &dt.names, p);
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
                if let Some((prev_m, prev_key, prev_raw)) = st.sources.get(&p.name) {
                    if *prev_m != m && *prev_key != key {
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
                            prev_raw,
                            label(m),
                            raw
                        ));
                    }
                }
                match st.props.iter_mut().find(|(n, _)| n == &p.name) {
                    Some((_, slot)) => *slot = bytes,
                    None => st.props.push((p.name.clone(), bytes)),
                }
                st.sources.insert(p.name.clone(), (m, key, raw));
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

/// Map one override's name-table indices into the merged table: a name that
/// already exists in vanilla keeps its vanilla index; anything else takes the
/// slot already allocated for it in `extras`, or appends a new one (first-seen
/// order, so the merged table is `vanilla ++ union-of-extras`).
///
/// Never positional: a mod cooked against a different base carries a wholly
/// different order, and index identity would silently re-point prop tags and
/// FNames at the wrong strings.
fn map_names(
    vanilla_by_name: &std::collections::HashMap<&str, i32>,
    vcount: usize,
    names: &[String],
    extras: &mut Vec<String>,
) -> Vec<i32> {
    let mut map = Vec::with_capacity(names.len());
    for name in names {
        if let Some(&vi) = vanilla_by_name.get(name.as_str()) {
            map.push(vi);
        } else {
            let pos = extras.iter().position(|n| n == name).unwrap_or_else(|| {
                extras.push(name.clone());
                extras.len() - 1
            });
            map.push((vcount + pos) as i32);
        }
    }
    map
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
            ArrayElem::Struct(props) => props.iter().any(|c| prop_has_moved_refs(c, imp_map)),
            ArrayElem::Prim {
                value: PropValue::Object(v),
                ..
            } => moved(*v),
            _ => false,
        }),
        _ => false,
    }
}

/// Resolve an import reference to the (ObjectName, ClassName) pair it names
/// in its OWN file's table — the semantic identity of a ref. Two refs that
/// resolve to the same pair name the same asset even when their raw indices
/// differ (a merged table's appended imports shift every index). Returns
/// `None` for non-import refs or out-of-range entries.
fn import_target(ua: &[u8], names: &[String], r: i32) -> Option<(String, String)> {
    if r >= 0 {
        return None;
    }
    let idx = (-r) as usize - 1;
    let mimp = i32::from_le_bytes(ua.get(65..69)?.try_into().ok()?) as usize;
    if idx >= mimp {
        return None;
    }
    let off = i32::from_le_bytes(ua.get(69..73)?.try_into().ok()?) as usize + idx * 28;
    let entry = ua.get(off..off + 28)?;
    let cls = i32::from_le_bytes(entry[8..12].try_into().ok()?);
    let obj = i32::from_le_bytes(entry[20..24].try_into().ok()?);
    if cls < 0 || obj < 0 {
        return None;
    }
    Some((
        names.get(obj as usize)?.clone(),
        names.get(cls as usize)?.clone(),
    ))
}

/// Collect every import reference inside a prop value (recursing into
/// structs and arrays), in walk order.
fn collect_refs(p: &Prop, out: &mut Vec<i32>) {
    match &p.value {
        PropValue::Object(v) => out.push(*v),
        PropValue::Struct { children } => children.iter().for_each(|c| collect_refs(c, out)),
        PropValue::Array { items, .. } => items.iter().for_each(|it| match it {
            ArrayElem::Struct(props) => props.iter().for_each(|c| collect_refs(c, out)),
            ArrayElem::Prim {
                value: PropValue::Object(v),
                ..
            } => out.push(*v),
            _ => {}
        }),
        _ => {}
    }
}

/// A readable one-line summary for conflict messages. Scalars and text use
/// the human repr; arrays/structs (which stringify to "") summarize as
/// `kind[count]` plus the resolved ref targets, so a message never reads
/// `'' vs ''` for two genuinely different values.
fn display_repr(ua: &[u8], names: &[String], p: &Prop) -> String {
    let raw = crate::resolver::prop_value_string(&p.value).unwrap_or_default();
    let shape = match &p.value {
        PropValue::Array {
            elem_type, count, ..
        } => Some(format!("{elem_type}[{count}]")),
        PropValue::Struct { children } => Some(format!("struct[{}]", children.len())),
        PropValue::Custom(_) => Some("custom".to_string()),
        _ => None,
    };
    let Some(mut out) = shape else {
        return raw;
    };
    let mut refs = Vec::new();
    collect_refs(p, &mut refs);
    let imp_refs: Vec<i32> = refs.into_iter().filter(|v| *v < 0).collect();
    if imp_refs.is_empty() {
        return out;
    }
    out.push(' ');
    out.push_str(
        &imp_refs
            .iter()
            .map(|v| match import_target(ua, names, *v) {
                Some((obj, _)) => obj,
                None => format!("{v}"),
            })
            .collect::<Vec<_>>()
            .join(","),
    );
    out
}

/// The semantic identity of a prop value for conflict comparison: the human
/// repr, except that object refs are resolved to the (ObjectName, ClassName)
/// they name in their own file's table. Raw import indices shift whenever a
/// table carries appended imports, so comparing them directly reports phantom
/// conflicts between refs that name the same asset (`-497` vs `-485` both
/// being `F_15E_Cockpit/StaticMesh`). Unresolvable refs (out-of-range, export
/// indices) keep the raw repr — always a real difference, never silenced.
fn semantic_repr(ua: &[u8], names: &[String], p: &Prop) -> String {
    let raw = crate::resolver::prop_value_string(&p.value).unwrap_or_default();
    let mut refs = Vec::new();
    collect_refs(p, &mut refs);
    let imp_refs: Vec<i32> = refs.into_iter().filter(|v| *v < 0).collect();
    if imp_refs.is_empty() {
        return raw;
    }
    let mut targets: Vec<String> = Vec::with_capacity(imp_refs.len());
    for v in &imp_refs {
        match import_target(ua, names, *v) {
            Some((obj, cls)) => targets.push(format!("{obj}<{cls}>")),
            // Unresolvable refs (out-of-range, bad names) keep the raw
            // number — always a real difference, never silenced.
            None => targets.push(format!("{v}")),
        }
    }
    // A bare object ref's raw repr IS the import number — pure index noise,
    // so the resolved targets replace it entirely. Structs/arrays stringify
    // to "" (no scalar content), so appending there is equally exact; any
    // other shape keeps its raw content alongside.
    if matches!(p.value, PropValue::Object(_)) {
        targets.join(",")
    } else {
        let mut out = raw;
        out.push('|');
        out.push_str(&targets.join(","));
        out
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

/// Map the row-name FName's index into the merged name table, leaving the
/// FName's `number` field untouched.
///
/// The index is read from the stored bytes, never searched for by display
/// name: the walker renders a duplicate row name as `Name_N` (`CF-105_2`),
/// and `N` lives in the FName's number field, not in the name-table string —
/// so a positional search over `dt.names` silently misses those rows and
/// leaves the mod's index in place. In a table with a different order that
/// index then resolves to an unrelated string, corrupting the row name.
fn remap_name_bytes(bytes: &[u8], map: &[i32], warnings: &mut Vec<String>) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let old = i32::from_le_bytes(out[0..4].try_into().unwrap());
    match map.get(old.max(0) as usize) {
        Some(&new) if old >= 0 && new != old => out[0..4].copy_from_slice(&new.to_le_bytes()),
        Some(_) => {}
        None => warnings.push(format!("row name index {old} outside the name table")),
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
    // Child lists inside a struct/array are terminated by an FName naming
    // `None`. That terminator is part of the copied bytes and must be remapped
    // like any other index — otherwise it resolves to an unrelated string in
    // the merged table, the walker never sees the terminator, and it reads past
    // the struct into the next tag (the DB_MF Import_Create crash).
    let none_idx = dt.names.iter().position(|n| n == "None");
    remap_prop(
        &mut out, p.start, p, extent_end, dt, map, vimp, imp_map, none_idx, warnings,
    );
    out
}

/// Remap the `None` terminator that closes a child list, if the copied bytes
/// actually carry it at `term` (an absolute payload offset).
fn patch_terminator(
    out: &mut [u8],
    base: usize,
    term: usize,
    none_idx: Option<usize>,
    map: &[i32],
    warnings: &mut Vec<String>,
) {
    let Some(none_old) = none_idx else {
        return;
    };
    let off = match term.checked_sub(base) {
        Some(o) if o + 8 <= out.len() => o,
        _ => return,
    };
    let cur = i32::from_le_bytes(out[off..off + 4].try_into().unwrap());
    if cur == none_old as i32 {
        patch_index_raw(out, off, none_old as i32, map, warnings);
    }
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
    end: usize,
    dt: &DataTable,
    map: &[i32],
    vimp: usize,
    imp_map: &[i32],
    none_idx: Option<usize>,
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
            // Each child's bytes end where the next sibling's tag starts; the
            // last one ends at the struct's own `None` terminator.
            for (i, c) in children.iter().enumerate() {
                let cend = match children.get(i + 1) {
                    Some(next) => next.start,
                    None => end.saturating_sub(8),
                };
                remap_prop(
                    out, base, c, cend, dt, map, vimp, imp_map, none_idx, warnings,
                );
            }
            patch_terminator(out, base, end.saturating_sub(8), none_idx, map, warnings);
        }
        PropValue::Array { items, .. } => {
            for (i, it) in items.iter().enumerate() {
                match it {
                    modman_uasset::walk::ArrayElem::Struct(props) => {
                        // One `None` closes each item's prop list, so an item
                        // ends where the next item's first tag begins; the
                        // last item ends at the array payload's end.
                        let item_end = match items.get(i + 1) {
                            Some(modman_uasset::walk::ArrayElem::Struct(next)) => {
                                next.first().map(|q| q.start).unwrap_or(p.vend)
                            }
                            _ => p.vend,
                        };
                        for (j, c) in props.iter().enumerate() {
                            let cend = match props.get(j + 1) {
                                Some(next) => next.start,
                                None => item_end.saturating_sub(8),
                            };
                            remap_prop(
                                out, base, c, cend, dt, map, vimp, imp_map, none_idx, warnings,
                            );
                        }
                        patch_terminator(
                            out,
                            base,
                            item_end.saturating_sub(8),
                            none_idx,
                            map,
                            warnings,
                        );
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

    /// A mod that ships a SUBSET of the vanilla rows (a partial table — the
    /// FE15 shape) deletes the rest; a later mod that re-carries them must
    /// restore them COMPLETE.
    ///
    /// Regression guard for two distinct failures:
    ///  * a re-added row whose bytes are vanilla-identical hit the
    ///    `!changed` fast path, so it never re-entered `state` while
    ///    `order` (rebuilt from the mod's full row list) still named it —
    ///    assembly then indexed `state[name]` for a missing key and panicked;
    ///  * with the row seeded, unchanged props are skipped by design, so an
    ///    empty seed would emit the row with props missing mid-stream (the
    ///    DB_MF `Import_Create` corruption).
    #[test]
    fn deleted_then_readded_rows_are_restored_complete() {
        let vua = fixture("DB_LevelList.uasset");
        let vue = fixture("DB_LevelList.uexp");
        let vdt = DataTable::walk_bytes(&vua, &vue).unwrap();
        assert!(vdt.rows.len() >= 3, "fixture needs a few rows");
        let r0 = &vdt.rows[0];
        let r0_name = r0.name.clone();
        let r0_prop_names: Vec<String> = r0.props.iter().map(|p| p.name.clone()).collect();

        // "partial" mod: vanilla with row 0 dropped entirely. The name and
        // import tables stay vanilla, so only the row list shrinks — but the
        // uasset's export serial_size and the header's file-size field must
        // follow, or the walker's integrity check rejects the pair.
        let mut pue = Vec::with_capacity(vue.len());
        pue.extend_from_slice(&vue[..r0.start]);
        pue.extend_from_slice(&vue[r0.end..]);
        let n = (vdt.rows.len() - 1) as i32;
        pue[vdt.num_entries_offset..vdt.num_entries_offset + 4].copy_from_slice(&n.to_le_bytes());
        let mut pua = vua.clone();
        {
            let h = modman_uasset::header::PackageHeader::read(&mut std::io::Cursor::new(&vua[..]))
                .unwrap();
            let serial_size_off = h.export_offset as usize + 28;
            pua[serial_size_off..serial_size_off + 8]
                .copy_from_slice(&(pue.len() as i64).to_le_bytes());
            let file_size = vua.len() as i32 + pue.len() as i32;
            pua[169..173].copy_from_slice(&file_size.to_le_bytes());
        }

        // Case 1: the later mod re-carries the row BYTE-IDENTICAL to vanilla.
        // Net effect is no change — but it must not panic, and it must not
        // report a change that loses the row.
        let out = merge_datatable_overrides(
            (&vua, &vue),
            &[(&pua, &pue), (&vua, &vue)],
            &["partial".to_string(), "full".to_string()],
        )
        .expect("merge must not panic on a deleted-then-readded row");
        if let Some(c) = out {
            let odt = DataTable::walk_bytes(&c.uasset, &c.uexp).unwrap();
            assert!(
                odt.rows.iter().any(|r| r.name == r0_name),
                "row {r0_name} was re-carried and must be present"
            );
        }

        // Case 2: the later mod re-carries it with one edited int prop, so a
        // real change exists. The re-added row must come out COMPLETE: every
        // vanilla prop present, not just the edited one.
        let int_pi = r0
            .props
            .iter()
            .position(|p| matches!(p.value, PropValue::Int(_)))
            .expect("row 0 needs an int prop");
        let mut fue = vue.clone();
        let p = &r0.props[int_pi];
        fue[p.vstart..p.vstart + 4].copy_from_slice(&424_242i32.to_le_bytes());

        let out2 = merge_datatable_overrides(
            (&vua, &vue),
            &[(&pua, &pue), (&vua, &fue)],
            &["partial".to_string(), "full-edit".to_string()],
        )
        .unwrap()
        .expect("the edited prop is a real change");

        let odt2 = DataTable::walk_bytes(&out2.uasset, &out2.uexp).unwrap();
        let orow = odt2
            .rows
            .iter()
            .find(|r| r.name == r0_name)
            .expect("re-added row present");
        let got: Vec<String> = orow.props.iter().map(|p| p.name.clone()).collect();
        assert_eq!(
            got, r0_prop_names,
            "re-added row must carry every vanilla prop in order"
        );
        match &orow.props[int_pi].value {
            PropValue::Int(v) => assert_eq!(*v, 424_242, "the later mod's edit survived"),
            other => panic!("not int: {other:?}"),
        }
    }

    /// The name map must resolve by STRING at every index. A mod cooked
    /// against a different base carries the same strings in a different order
    /// (DB_MF: `A-10/A-10` @0 vs vanilla's `f15_icon` @0); positional mapping
    /// re-points prop tags and type names at the wrong strings, which is how
    /// the merged table became unwalkable and crashed the game on load.
    #[test]
    fn name_map_resolves_by_string_not_position() {
        let vanilla = [
            "f15_icon".to_string(),
            "AircraftModel".to_string(),
            "ObjectProperty".to_string(),
        ];
        let mut by_name: std::collections::HashMap<&str, i32> = std::collections::HashMap::new();
        for (k, n) in vanilla.iter().enumerate() {
            by_name.entry(n.as_str()).or_insert(k as i32);
        }

        // Same strings, permuted, plus one name vanilla has never seen.
        let mod_a = vec![
            "ObjectProperty".to_string(),
            "A10".to_string(),
            "AircraftModel".to_string(),
            "f15_icon".to_string(),
        ];
        let mut extras: Vec<String> = Vec::new();
        let map = map_names(&by_name, vanilla.len(), &mod_a, &mut extras);
        assert_eq!(
            map,
            vec![2, 3, 1, 0],
            "every vanilla name resolves to its vanilla slot; the unknown one appends"
        );
        assert_eq!(extras, vec!["A10".to_string()], "extra recorded first-seen");

        // A second mod re-using the same unknown name must share its slot,
        // not append a duplicate.
        let mod_b = vec!["A10".to_string(), "ObjectProperty".to_string()];
        let map_b = map_names(&by_name, vanilla.len(), &mod_b, &mut extras);
        assert_eq!(map_b, vec![3, 2]);
        assert_eq!(extras.len(), 1, "extra reused across mods, not duplicated");
    }

    /// Row-name FNames are an index plus a `number` field; the walker renders
    /// duplicates as `Name_N`, so the index must be read from the BYTES — a
    /// name-table search for the display string misses them. Regression:
    /// `CF-105_2` / `MiG-21_2` / `AV-8_2` in DB_MF kept the mod's index and
    /// resolved to unrelated strings in the merged table.
    #[test]
    fn duplicate_row_name_index_is_remapped_from_bytes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&273i32.to_le_bytes());
        bytes.extend_from_slice(&3i32.to_le_bytes());

        let mut map = vec![0i32; 400];
        map[273] = 42;
        let mut warnings = Vec::new();
        let out = remap_name_bytes(&bytes, &map, &mut warnings);
        assert_eq!(
            i32::from_le_bytes(out[0..4].try_into().unwrap()),
            42,
            "stored index must be remapped"
        );
        assert_eq!(
            i32::from_le_bytes(out[4..8].try_into().unwrap()),
            3,
            "the FName number must be preserved"
        );
        assert!(warnings.is_empty(), "{warnings:?}");

        // Identity mapping leaves the bytes alone.
        let ident: Vec<i32> = (0..400).collect();
        assert_eq!(remap_name_bytes(&bytes, &ident, &mut warnings), bytes);

        // An index outside the name table is reported rather than kept.
        let mut oor = Vec::new();
        oor.extend_from_slice(&900_000i32.to_le_bytes());
        oor.extend_from_slice(&0i32.to_le_bytes());
        let _ = remap_name_bytes(&oor, &map, &mut warnings);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    /// A struct's child list is closed by a `None` FName taken from the MOD's
    /// name table. Left unremapped it names an unrelated string in the merged
    /// table, so the walker reads past the struct into the next tag — the
    /// DB_MF `Import_Create` crash. Guards the remap of that terminator.
    #[test]
    fn struct_child_list_terminator_is_remapped() {
        let none_old = 475usize;
        let mut map = vec![0i32; 600];
        map[none_old] = 137; // "None" sits at 137 in the merged table
        let mut warnings = Vec::new();

        // [child tag 24 bytes][None terminator 8 bytes]
        let mut out = vec![0u8; 32];
        out[24..28].copy_from_slice(&(none_old as i32).to_le_bytes());
        patch_terminator(&mut out, 0, 24, Some(none_old), &map, &mut warnings);
        assert_eq!(
            i32::from_le_bytes(out[24..28].try_into().unwrap()),
            137,
            "the terminator must point at the merged table's None"
        );

        // A non-None value at that position must be left alone.
        let mut other = vec![0u8; 32];
        other[24..28].copy_from_slice(&7i32.to_le_bytes());
        patch_terminator(&mut other, 0, 24, Some(none_old), &map, &mut warnings);
        assert_eq!(i32::from_le_bytes(other[24..28].try_into().unwrap()), 7);
        assert!(warnings.is_empty(), "{warnings:?}");
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

    /// Two mods whose refs resolve to the SAME asset at DIFFERENT import
    /// indices are not a conflict: the raw `-497` vs `-485` shape in the
    /// wild (a merged table fed back in shifts every index) must compare by
    /// resolved (ObjectName, ClassName), not by raw number.
    #[test]
    fn same_asset_different_index_is_not_a_conflict() {
        let vua = fixture("DB_LevelList.uasset");
        let vue = fixture("DB_LevelList.uexp");
        let vdt = DataTable::walk_bytes(&vua, &vue).unwrap();
        // Anchor: first negative object ref, by row + prop name (positions
        // shift between input and output walks).
        let (anchor_row, anchor_prop, anchor_ref) = vdt
            .rows
            .iter()
            .find_map(|r| {
                r.props.iter().find_map(|p| match &p.value {
                    PropValue::Object(v) if *v < 0 => Some((r.name.clone(), p.name.clone(), *v)),
                    _ => None,
                })
            })
            .expect("fixture must contain an object ref");
        let anchor_idx = (-anchor_ref) as usize - 1;
        let mimp = rd_i32(&vua, 65) as usize;
        assert!(anchor_idx < mimp);
        let imp_off = rd_i32(&vua, 69) as usize;
        // Victim: an import entry NO uexp ref and NO outer-link points at —
        // computed, not hardcoded (elsewhere-verified: 0, 21, 98 here).
        let mut referenced: std::collections::BTreeSet<usize> = vdt
            .rows
            .iter()
            .flat_map(|r| r.props.iter())
            .flat_map(|p| {
                let mut v = Vec::new();
                collect_refs(p, &mut v);
                v
            })
            .filter(|v| *v < 0)
            .map(|v| (-v) as usize - 1)
            .collect();
        for k in 0..mimp {
            let outer = rd_i32(&vua, imp_off + k * 28 + 16);
            if outer < 0 {
                referenced.insert((-outer) as usize - 1);
            }
        }
        let mut victims = (0..mimp).filter(|k| !referenced.contains(k) && *k != anchor_idx);
        let (v1, v2) = (
            victims
                .next()
                .expect("fixture needs unreferenced import entries"),
            victims
                .next()
                .expect("fixture needs unreferenced import entries"),
        );
        let anchor_prop_walk = vdt
            .rows
            .iter()
            .find(|r| r.name == anchor_row)
            .and_then(|r| r.props.iter().find(|p| p.name == anchor_prop))
            .expect("anchor prop");
        // Each mod clones the anchor entry into its own victim slot — with
        // that slot's OuterIndex PATCHED to point at itself, so neither clone
        // byte-matches any vanilla entry and both survive content-based
        // mapping as appended extras (the wild merged-table shape). Each
        // mod's anchor row then points at its own victim index: same asset
        // (ObjectName+ClassName), different numbers on both sides, so the
        // later-wins path genuinely compares two keys.
        let mk_mod = |victim: usize| {
            let mut mua = vua.clone();
            let mut anchor_entry =
                vua[imp_off + anchor_idx * 28..imp_off + anchor_idx * 28 + 28].to_vec();
            anchor_entry[16..20].copy_from_slice(&(-((victim as i32) + 1)).to_le_bytes());
            mua[imp_off + victim * 28..imp_off + victim * 28 + 28].copy_from_slice(&anchor_entry);
            let mut mue = vue.clone();
            mue[anchor_prop_walk.vstart..anchor_prop_walk.vstart + 4]
                .copy_from_slice(&(-((victim as i32) + 1)).to_le_bytes());
            (mua, mue)
        };
        let (a_ua, a_ue) = mk_mod(v1);
        let (b_ua, b_ue) = mk_mod(v2);

        let out = merge_datatable_overrides(
            (&vua, &vue),
            &[(&a_ua, &a_ue), (&b_ua, &b_ue)],
            &["modA".to_string(), "modB".to_string()],
        )
        .unwrap()
        .expect("merge");
        assert!(
            out.conflicts.is_empty(),
            "same asset at a different index is not a conflict: {:?}",
            out.conflicts
        );
        // The later mod's edit still applied (its ref won the slot).
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
        match &oprop.value {
            PropValue::Object(v) => {
                let got = import_target(&out.uasset, &odt.names, *v).expect("resolves");
                // The victim entry is a clone of the anchor's, so the
                // winner's ref must name the anchor's (ObjectName, ClassName).
                let want = import_target(&vua, &vdt.names, anchor_ref).expect("anchor resolves");
                assert_eq!(got, want, "winner's ref names the same asset");
            }
            other => panic!("not an object ref: {other:?}"),
        }
    }

    /// A genuinely different target at the same field still conflicts: the
    /// semantic compare must not swallow real differences.
    #[test]
    fn different_asset_same_field_still_conflicts() {
        let vua = fixture("DB_LevelList.uasset");
        let vue = fixture("DB_LevelList.uexp");
        let vdt = DataTable::walk_bytes(&vua, &vue).unwrap();
        let (anchor_row, anchor_prop, anchor_ref) = vdt
            .rows
            .iter()
            .find_map(|r| {
                r.props.iter().find_map(|p| match &p.value {
                    PropValue::Object(v) if *v < 0 => Some((r.name.clone(), p.name.clone(), *v)),
                    _ => None,
                })
            })
            .expect("fixture must contain an object ref");
        let anchor_idx = (-anchor_ref) as usize - 1;
        let mimp = rd_i32(&vua, 65) as usize;
        let imp_off = rd_i32(&vua, 69) as usize;
        let anchor_prop_walk = vdt
            .rows
            .iter()
            .find(|r| r.name == anchor_row)
            .and_then(|r| r.props.iter().find(|p| p.name == anchor_prop))
            .expect("anchor prop");
        // Mod A repoints the field at a clone of the anchor entry (patched
        // OuterIndex so it survives as an appended extra — a genuine edit
        // with key = anchor target; a byte-identical mod A would be skipped
        // and record no key, leaving mod B with nothing to clash against).
        // Mod B repoints the same field at a DIFFERENT entry's slot: same
        // field, other asset → exactly one conflict.
        let other_idx = (anchor_idx + 1) % mimp;
        assert_ne!(other_idx, anchor_idx);
        let other_entry = vua[imp_off + other_idx * 28..imp_off + other_idx * 28 + 28].to_vec();
        let anchor_bytes: [u8; 28] = vua[imp_off + anchor_idx * 28..imp_off + anchor_idx * 28 + 28]
            .try_into()
            .unwrap();
        assert_ne!(
            other_entry, anchor_bytes,
            "test needs two distinct import entries"
        );
        // Anchor entry's ObjectName must differ from the other entry's, else
        // the keys coincide and no conflict is (correctly) reported.
        let oname = |idx: usize| rd_i32(&vua, imp_off + idx * 28 + 20);
        assert_ne!(
            oname(anchor_idx),
            oname(other_idx),
            "test needs two entries naming different objects"
        );
        let mut a_ua = vua.clone();
        let mut clone = anchor_bytes.to_vec();
        // Differ the clone's OuterIndex (point at the anchor slot) so it
        // survives content-mapping as an appended extra; then overwrite an
        // import slot no uexp ref or outer-link touches (computed below).
        clone[16..20].copy_from_slice(&anchor_ref.to_le_bytes());
        let mut referenced: std::collections::BTreeSet<usize> = vdt
            .rows
            .iter()
            .flat_map(|r| r.props.iter())
            .flat_map(|p| {
                let mut v = Vec::new();
                collect_refs(p, &mut v);
                v
            })
            .filter(|v| *v < 0)
            .map(|v| (-v) as usize - 1)
            .collect();
        for k in 0..mimp {
            let outer = rd_i32(&vua, imp_off + k * 28 + 16);
            if outer < 0 {
                referenced.insert((-outer) as usize - 1);
            }
        }
        let slot = (0..mimp)
            .find(|k| !referenced.contains(k))
            .expect("fixture needs an unreferenced import entry");
        a_ua[imp_off + slot * 28..imp_off + slot * 28 + 28].copy_from_slice(&clone);
        let mut a_ue = vue.clone();
        a_ue[anchor_prop_walk.vstart..anchor_prop_walk.vstart + 4]
            .copy_from_slice(&(-((slot as i32) + 1)).to_le_bytes());
        let mut b_ue = vue.clone();
        b_ue[anchor_prop_walk.vstart..anchor_prop_walk.vstart + 4]
            .copy_from_slice(&(-((other_idx as i32) + 1)).to_le_bytes());

        let out = merge_datatable_overrides(
            (&vua, &vue),
            &[(&a_ua, &a_ue), (&vua, &b_ue)],
            &["modA".to_string(), "modB".to_string()],
        )
        .unwrap()
        .expect("merge");
        assert_eq!(
            out.conflicts.len(),
            1,
            "different asset must conflict: {:?}",
            out.conflicts
        );
        assert!(out.conflicts[0].contains("modA"));
        assert!(out.conflicts[0].contains("modB"));
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
    /// path -> bytes: merged datatables and their fallbacks. Pass-through
    /// files are here too unless the caller asked to defer them.
    pub files: std::collections::BTreeMap<String, Vec<u8>>,
    /// How many datatables were merged.
    pub merged: usize,
    /// Field-level conflicts (later mod wins).
    pub field_conflicts: Vec<String>,
    /// Non-datatable files overridden by multiple mods (single-winner).
    pub pass_through_conflicts: Vec<String>,
    /// Merge warnings (divergences, unmergeable files, etc).
    pub warnings: Vec<String>,
    /// Input labels skipped because they are a previous combine output
    /// ([`COMBINE_OUTPUT_NAME`]), in input order. Merging a merged table
    /// back in shifts every import index and floods the report with phantom
    /// conflicts — so they are fixed-and-noted, never merged. Reported via
    /// this field (not warnings) so both frontends render it the same way.
    pub skipped: Vec<String>,
    /// Deferred pass-through files: (effective path, source index, source entry
    /// path). Populated only when `defer_passthrough` is set; the caller reads
    /// these from the source pak when writing the output.
    pub pass_through: Vec<(String, usize, String)>,
}

/// Combine conflicting override mods over a vanilla base: three-way
/// datatable merges (later mods win), single-winner pass-through for
/// everything else. This is the full orchestration both frontends share.
/// Combine behavior switches.
#[derive(Default, Clone, Copy, Debug)]
pub struct CombineOptions {
    /// Emit only the game's DataTable files (merged, or last-version
    /// fallback when unmergeable). Every pass-through file — textures,
    /// audio, meshes, loose files — is left out of the output.
    pub datatables_only: bool,
    /// Record pass-through files as (path, source index, source entry) instead
    /// of reading their bytes. Filesystem frontends set this and copy the bytes
    /// straight from the source pak while writing, so a 4.4 GB mod set does not
    /// have to sit in RAM. The browser leaves it off: it needs the bytes.
    pub defer_passthrough: bool,
}

pub fn combine_sources(
    base: &dyn PakSource,
    mods: &[&dyn PakSource],
    labels: &[String],
) -> Result<CombineOutcome, ApplyError> {
    combine_sources_with(base, mods, labels, &CombineOptions::default())
}

pub fn combine_sources_with(
    base: &dyn PakSource,
    mods: &[&dyn PakSource],
    labels: &[String],
    options: &CombineOptions,
) -> Result<CombineOutcome, ApplyError> {
    let base_mount = base.mount_point();
    let base_files: Vec<String> = base
        .files()
        .iter()
        .map(|r| effective_path(&base_mount, &r.replace('\\', "/")))
        .collect();
    // Exact hits are the common case; the suffix fallback exists for nested
    // mounts. Index both so a 50k-path pak table is not scanned per mod asset.
    let mut base_exact: std::collections::HashMap<&str, &String> =
        std::collections::HashMap::with_capacity(base_files.len());
    let mut base_by_name: std::collections::HashMap<&str, Vec<&String>> =
        std::collections::HashMap::new();
    for f in &base_files {
        base_exact.entry(f.as_str()).or_insert(f);
        if let Some(leaf) = f.rsplit('/').next() {
            base_by_name.entry(leaf).or_default().push(f);
        }
    }
    let base_find = |name: &str| -> Option<String> {
        if let Some(hit) = base_exact.get(name) {
            return Some((*hit).clone());
        }
        let leaf = name.rsplit('/').next().unwrap_or(name);
        base_by_name
            .get(leaf)
            .and_then(|candidates| candidates.iter().find(|f| f.ends_with(name)))
            .map(|f| (*f).clone())
    };

    struct Override {
        ua: Vec<u8>,
        ue: Vec<u8>,
        label: String,
    }
    let mut dt_overrides: std::collections::BTreeMap<String, Vec<Override>> =
        std::collections::BTreeMap::new();
    let mut passthrough: std::collections::BTreeMap<String, (usize, String)> =
        std::collections::BTreeMap::new();
    let mut pt_conflicts: Vec<String> = Vec::new();
    // Previous combine outputs are stale merges, not fresh mods: feeding one
    // back in shifts every import index, so each vanilla row reads as changed
    // and the report floods with phantom conflicts. Skip by label (filename),
    // case-insensitively — fixed-and-noted, never an error.
    let mut skipped: Vec<String> = Vec::new();
    let live: Vec<(usize, &&dyn PakSource)> = mods
        .iter()
        .enumerate()
        .filter(|(mi, _)| {
            let label = labels
                .get(*mi)
                .cloned()
                .unwrap_or_else(|| format!("override #{mi}"));
            if label.eq_ignore_ascii_case(COMBINE_OUTPUT_NAME) {
                skipped.push(label);
                false
            } else {
                true
            }
        })
        .collect();

    for (mi, m) in live {
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
                        for (k, src_entry) in [
                            (eff.clone(), raw.clone()),
                            (uexp_eff.clone(), uexp_raw.clone()),
                        ] {
                            if let Some((prev, _)) = passthrough.get(&k) {
                                if *prev != mi {
                                    pt_conflicts.push(k.clone());
                                }
                            }
                            passthrough.insert(k, (mi, src_entry));
                        }
                        continue;
                    };
                    let vkey_uexp = format!("{}.uexp", &vkey[..vkey.len() - 7]);
                    let Some(vue_key) = base_find(&vkey_uexp) else {
                        continue;
                    };
                    let ua = m.read(raw)?;
                    // Only real DataTables are field-mergeable. Opaque assets
                    // (textures, audio, meshes) that happen to ship a
                    // .uasset+.uexp pair pass through single-winner — their
                    // payloads (pixels, samples, vertices) cannot be merged.
                    if modman_uasset::asset_class(&ua).ok().as_deref() != Some("DataTable") {
                        drop(ua);
                        for (k, src_entry) in [
                            (eff.clone(), raw.clone()),
                            (uexp_eff.clone(), uexp_raw.clone()),
                        ] {
                            if let Some((prev, _)) = passthrough.get(&k) {
                                if *prev != mi {
                                    pt_conflicts.push(k.clone());
                                }
                            }
                            passthrough.insert(k, (mi, src_entry));
                        }
                        continue;
                    }
                    let ue = m.read(uraw)?;
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
                if let Some((prev, _)) = passthrough.get(eff) {
                    if *prev != mi {
                        pt_conflicts.push(eff.clone());
                    }
                }
                passthrough.insert(eff.clone(), (mi, raw.clone()));
            }
        }
    }

    let mut files = std::collections::BTreeMap::new();
    let mut pass_through: Vec<(String, usize, String)> = Vec::new();
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
    if options.datatables_only {
        let dropped = passthrough.len();
        if dropped > 0 {
            warnings.push(format!(
                "excluded {dropped} non-datatable file(s) (--datatables-only)"
            ));
        }
    } else if options.defer_passthrough {
        // Hand the caller source locations; it copies the bytes straight into
        // the output instead of materialising them here.
        pass_through = passthrough
            .into_iter()
            .map(|(path, (mi, src_entry))| (path, mi, src_entry))
            .collect();
    } else {
        // In-memory frontends get the bytes. Moved, never cloned: the clone
        // doubled peak memory for nothing.
        for (path, (mi, src_entry)) in passthrough {
            let bytes = mods[mi].read(&src_entry)?;
            files.insert(path, bytes);
        }
    }

    Ok(CombineOutcome {
        files,
        merged,
        field_conflicts,
        pass_through_conflicts: pt_conflicts,
        warnings,
        skipped,
        pass_through,
    })
}

// ── File-level orchestration ──────────────────────────────────────────────
//
// `modman combine`, the browser page and the GUI all run the same job. The
// pieces below are the parts that touch the filesystem, so every frontend
// shares one implementation instead of three that drift.

/// Where a combine run currently is, for a frontend's progress display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombineStage {
    Opening {
        index: usize,
        total: usize,
        label: String,
    },
    Merging,
    Writing {
        files: usize,
    },
    Packing,
}

/// What a completed run produced, without the file bytes.
#[derive(Debug, Clone)]
pub struct CombineRun {
    pub output: PathBuf,
    pub merged: usize,
    pub files: usize,
    pub order: Vec<String>,
    pub field_conflicts: Vec<String>,
    pub pass_through_conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
}

/// A merge's findings without an output path or file bytes. `probe` returns
/// this so a frontend can show conflicts before writing anything; `files` is
/// the total output-file count, matching [`CombineRun::files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombineReport {
    pub merged: usize,
    pub files: usize,
    pub order: Vec<String>,
    pub field_conflicts: Vec<String>,
    pub pass_through_conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
}

fn report_from_outcome(outcome: &CombineOutcome, order: Vec<String>) -> CombineReport {
    CombineReport {
        merged: outcome.merged,
        files: outcome.files.len() + outcome.pass_through.len(),
        order,
        field_conflicts: outcome.field_conflicts.clone(),
        pass_through_conflicts: outcome.pass_through_conflicts.clone(),
        warnings: outcome.warnings.clone(),
        skipped: outcome.skipped.clone(),
    }
}

/// A [`PakSource`] over a `.pak` file on disk. Public because every frontend
/// needs it: `combine` uses it internally, and the audio/locres readers take a
/// `PakSource` too.
pub struct FilePakSource<'a>(pub &'a modman_pak::PakArchive);

impl PakSource for FilePakSource<'_> {
    fn files(&self) -> Vec<String> {
        self.0.files()
    }
    fn read(&self, path: &str) -> Result<Vec<u8>, ApplyError> {
        self.0
            .read_entry(path)
            .map_err(|e| ApplyError::Asset(e.to_string()))
    }
    fn mount_point(&self) -> String {
        self.0.info().mount_point
    }
}

/// Run a combine over paks on disk and write `SicarioCombine_P.pak` into
/// `out_dir`. Returns the report; the caller decides how to show it.
pub fn run_to_dir(
    base_pak: &Path,
    mod_paks: &[PathBuf],
    out_dir: &Path,
    options: &CombineOptions,
    mut on_stage: impl FnMut(&CombineStage),
) -> Result<CombineRun, String> {
    if mod_paks.is_empty() {
        return Err("no mod paks given".to_string());
    }

    let base_archive = modman_pak::PakArchive::open(base_pak)
        .map_err(|e| format!("{}: {e}", base_pak.display()))?;
    let base_src = FilePakSource(&base_archive);

    let mut archives = Vec::with_capacity(mod_paks.len());
    let mut labels = Vec::with_capacity(mod_paks.len());
    for (i, path) in mod_paks.iter().enumerate() {
        let label = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        on_stage(&CombineStage::Opening {
            index: i + 1,
            total: mod_paks.len(),
            label: label.clone(),
        });
        let archive = modman_pak::PakArchive::open(path).map_err(|e| format!("{label}: {e}"))?;
        labels.push(label);
        archives.push(archive);
    }
    let srcs: Vec<FilePakSource> = archives.iter().map(FilePakSource).collect();
    let src_refs: Vec<&dyn PakSource> = srcs.iter().map(|s| s as &dyn PakSource).collect();

    on_stage(&CombineStage::Merging);
    // Filesystem frontends never need pass-through bytes in RAM: they are
    // copied from the source pak straight into the staging tree below.
    let opts = CombineOptions {
        datatables_only: options.datatables_only,
        defer_passthrough: true,
    };
    let outcome =
        combine_sources_with(&base_src, &src_refs, &labels, &opts).map_err(|e| e.to_string())?;

    let total_files = outcome.files.len() + outcome.pass_through.len();
    if total_files == 0 {
        return Err("nothing to combine (no conflicting overrides found)".to_string());
    }

    on_stage(&CombineStage::Writing { files: total_files });
    let staging = out_dir.join("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    for (key, bytes) in &outcome.files {
        let Some(rel) = modman_pak::safe_rel(key) else {
            return Err(format!("unsafe entry path escapes destination: {key}"));
        };
        let path = staging.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    // Pass-through content: streamed from the source pak to its staging file,
    // one file at a time. This is what keeps a 4.4 GB set from needing GBs of
    // RAM — the bytes are never held whole.
    for (path, source, entry) in &outcome.pass_through {
        let Some(rel) = modman_pak::safe_rel(path) else {
            return Err(format!("unsafe entry path escapes destination: {path}"));
        };
        let out_path = staging.join(rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        archives[*source]
            .extract_entry(entry, &out_path)
            .map_err(|e| format!("{path}: {e}"))?;
    }

    on_stage(&CombineStage::Packing);
    let output = out_dir.join(COMBINE_OUTPUT_NAME);
    modman_pak::pack(
        &staging,
        &output,
        modman_pak::Version::V3,
        "../../../".to_string(),
    )
    .map_err(|e| format!("pack: {e}"))?;
    // The staging copy is scratch; leaving multi-gigabyte duplicates behind is
    // how a user's disk fills up silently.
    let _ = std::fs::remove_dir_all(&staging);

    Ok(CombineRun {
        output,
        merged: outcome.merged,
        files: total_files,
        order: labels,
        field_conflicts: outcome.field_conflicts,
        pass_through_conflicts: outcome.pass_through_conflicts,
        warnings: outcome.warnings,
        skipped: outcome.skipped,
    })
}

#[cfg(test)]
mod combine_options_tests {
    use super::*;
    use std::collections::BTreeMap;

    struct MapPak {
        mount: String,
        files: BTreeMap<String, Vec<u8>>,
    }

    impl PakSource for MapPak {
        fn files(&self) -> Vec<String> {
            self.files.keys().cloned().collect()
        }
        fn read(&self, path: &str) -> Result<Vec<u8>, crate::apply::ApplyError> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| crate::apply::ApplyError::Asset(format!("missing {path}")))
        }
        fn mount_point(&self) -> String {
            self.mount.clone()
        }
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../modman-uasset/tests/fixtures/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        ))
        .unwrap()
    }

    #[test]
    fn report_counts_match_the_outcome() {
        let mut outcome = CombineOutcome::default();
        outcome.merged = 2;
        outcome.files.insert("a".to_string(), vec![1]);
        outcome
            .pass_through
            .push(("b".to_string(), 0, "b".to_string()));
        outcome.field_conflicts.push("x".to_string());
        let report = report_from_outcome(&outcome, vec!["m".to_string()]);
        assert_eq!(report.merged, 2);
        assert_eq!(report.files, 2);
        assert_eq!(report.order, vec!["m".to_string()]);
        assert_eq!(report.field_conflicts, vec!["x".to_string()]);
    }

    #[test]
    fn datatables_only_drops_pass_through_files() {
        let vua = fixture("DB_Aircraft.uasset");
        let vue = fixture("DB_Aircraft.uexp");
        let mua = fixture("DB_Aircraft.skin.merged.uasset");
        let mue = fixture("DB_Aircraft.skin.merged.uexp");

        let dt_ua = "../../../ProjectWingman/Content/Data/DB_Aircraft.uasset".to_string();
        let dt_ue = "../../../ProjectWingman/Content/Data/DB_Aircraft.uexp".to_string();
        let skin = "../../../ProjectWingman/Content/Assets/Skins/TF/TF_Grey.uasset".to_string();

        let base = MapPak {
            mount: "../../../".to_string(),
            files: [
                (dt_ua.clone(), vua),
                (dt_ue.clone(), vue),
                (skin.clone(), vec![0u8; 64]),
            ]
            .into(),
        };
        let modpak = MapPak {
            mount: "../../../".to_string(),
            files: [
                (dt_ua.clone(), mua),
                (dt_ue.clone(), mue),
                (skin.clone(), vec![9u8; 64]), // a texture-ish override
            ]
            .into(),
        };
        let labels = vec!["skinmod".to_string()];
        let src_refs: Vec<&dyn PakSource> = vec![&modpak];

        // Default: merged datatable pair AND the pass-through skin file.
        let full = combine_sources(&base, &src_refs, &labels).unwrap();
        assert!(full.files.contains_key(&dt_ua) && full.files.contains_key(&dt_ue));
        assert!(
            full.files.contains_key(&skin),
            "default keeps pass-throughs"
        );
        assert_eq!(full.files.len(), 3);

        // datatables_only: exactly the merged pair, skin excluded, reported.
        let dt = combine_sources_with(
            &base,
            &src_refs,
            &labels,
            &CombineOptions {
                datatables_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            dt.files.keys().cloned().collect::<Vec<_>>(),
            vec![dt_ua.clone(), dt_ue.clone()]
        );
        assert_eq!(dt.merged, 1);
        assert!(
            dt.warnings.iter().any(|w| w.contains("excluded 1")),
            "the exclusion must be reported: {:?}",
            dt.warnings
        );
    }

    #[test]
    fn previous_combine_output_is_skipped_not_merged() {
        let vua = fixture("DB_Aircraft.uasset");
        let vue = fixture("DB_Aircraft.uexp");
        let mua = fixture("DB_Aircraft.skin.merged.uasset");
        let mue = fixture("DB_Aircraft.skin.merged.uexp");

        let dt_ua = "../../../ProjectWingman/Content/Data/DB_Aircraft.uasset".to_string();
        let dt_ue = "../../../ProjectWingman/Content/Data/DB_Aircraft.uexp".to_string();

        let base = MapPak {
            mount: "../../../".to_string(),
            files: [(dt_ua.clone(), vua), (dt_ue.clone(), vue)].into(),
        };
        let stale = MapPak {
            mount: "../../../".to_string(),
            files: [(dt_ua.clone(), mua), (dt_ue.clone(), mue)].into(),
        };
        let labels = vec![COMBINE_OUTPUT_NAME.to_string()];
        let src_refs: Vec<&dyn PakSource> = vec![&stale];

        let out = combine_sources(&base, &src_refs, &labels).unwrap();
        assert_eq!(
            out.skipped,
            vec![COMBINE_OUTPUT_NAME.to_string()],
            "the stale output must be reported as skipped"
        );
        assert!(
            out.files.is_empty(),
            "nothing to merge from a skipped input: {:?}",
            out.files.keys().collect::<Vec<_>>()
        );
        assert_eq!(out.merged, 0);
        assert!(
            out.field_conflicts.is_empty() && out.pass_through_conflicts.is_empty(),
            "a skipped input contributes no conflicts: {:?} {:?}",
            out.field_conflicts,
            out.pass_through_conflicts
        );
    }

    #[test]
    fn forged_import_count_is_an_error_not_a_panic() {
        let vua = fixture("DB_Aircraft.uasset");
        let vue = fixture("DB_Aircraft.uexp");
        let mut ua = vua.clone();
        ua[65..69].copy_from_slice(&0x1000_0000i32.to_le_bytes());
        let err = match merge_datatable_overrides(
            (&vua, &vue),
            &[(&ua, &vue)],
            &["forged".to_string()],
        ) {
            Err(e) => e,
            Ok(_) => panic!("a forged import count must be rejected"),
        };
        assert!(
            err.to_string().contains("import table") || err.to_string().contains("import count"),
            "unexpected error: {err}"
        );
    }
}
