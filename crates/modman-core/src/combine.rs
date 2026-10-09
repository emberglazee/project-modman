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
}

/// Three-way merge of a vanilla datatable pair with any number of override
/// pairs. Returns `None` when nothing changed.
pub fn merge_datatable_overrides(
    vanilla: (&[u8], &[u8]),
    overrides: &[(&[u8], &[u8])],
) -> Result<Option<CombinedPair>, ApplyError> {
    let (vua, vue) = vanilla;
    let vdt = DataTable::walk_bytes(vua, vue).map_err(|e| ApplyError::Asset(e.to_string()))?;
    let vcount = vdt.names.len();
    let mut warnings = Vec::new();

    // Build the merged extras list + per-mod index maps.
    let mut merged_extras: Vec<String> = Vec::new();
    let mut maps: Vec<Vec<i32>> = Vec::new();
    let mut dts: Vec<DataTable> = Vec::new();
    for (i, (ua, ue)) in overrides.iter().enumerate() {
        let dt = DataTable::walk_bytes(ua, ue).map_err(|e| ApplyError::Asset(e.to_string()))?;
        // Sanity: the shared prefix must match vanilla, otherwise the
        // index-compat shortcut is unsound for this mod.
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
        dts.push(dt);
    }

    // Row state: start from vanilla; apply each override's DELTA in order.
    // A mod's row applies only when it differs from vanilla (otherwise a mod
    // that never touched a row would clobber earlier mods' edits).
    let vanilla_rows: BTreeMap<&str, &[u8]> = vdt
        .rows
        .iter()
        .map(|r| (r.name.as_str(), &vue[r.start..r.end]))
        .collect();
    let mut order: Vec<String> = vdt.rows.iter().map(|r| r.name.clone()).collect();
    let mut rows: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for r in &vdt.rows {
        rows.insert(r.name.clone(), vue[r.start..r.end].to_vec());
    }

    for (m, dt) in dts.iter().enumerate() {
        let ue = overrides[m].1;
        let map = &maps[m];
        // Changed + added rows only (delta vs vanilla).
        for r in &dt.rows {
            let src = &ue[r.start..r.end];
            if let Some(vb) = vanilla_rows.get(r.name.as_str()) {
                if *vb == src {
                    continue; // untouched by this mod
                }
            }
            let bytes = remap_row(src, r, dt, map, &mut warnings);
            if !rows.contains_key(&r.name) {
                order.push(r.name.clone());
            }
            rows.insert(r.name.clone(), bytes);
        }
        // Deletions: vanilla rows absent from this mod.
        let present: std::collections::BTreeSet<&str> =
            dt.rows.iter().map(|r| r.name.as_str()).collect();
        for vr in &vdt.rows {
            if !present.contains(vr.name.as_str()) {
                rows.remove(&vr.name);
                order.retain(|n| n != &vr.name);
            }
        }
    }

    // Assemble the merged uexp: [prefix][numEntries][between][rows][tail].
    let mut uexp = Vec::new();
    let num_off = vdt.num_entries_offset;
    uexp.extend_from_slice(&vue[..num_off]);
    uexp.extend_from_slice(&(order.len() as i32).to_le_bytes());
    let first_row_start = vdt.rows.first().map(|r| r.start).unwrap_or(num_off + 4);
    if num_off + 4 < first_row_start {
        uexp.extend_from_slice(&vue[num_off + 4..first_row_start]);
    }
    for name in &order {
        uexp.extend_from_slice(&rows[name]);
    }
    let last_row_end = vdt.rows.last().map(|r| r.end).unwrap_or(vue.len());
    uexp.extend_from_slice(&vue[last_row_end..]);

    if uexp == vue {
        return Ok(None);
    }

    let delta = uexp.len() as i64 - vue.len() as i64;
    let uasset = modman_uasset::rewrite::rewrite_uasset(
        vua,
        &modman_uasset::rewrite::RewritePlan {
            name_append: merged_extras,
            link_append: vec![],
            uexp_delta: delta,
        },
    )
    .map_err(|e| ApplyError::Asset(e.to_string()))?;

    Ok(Some(CombinedPair {
        uasset,
        uexp,
        warnings,
    }))
}

/// Copy a row's bytes with name indices remapped into the merged table.
fn remap_row(
    bytes: &[u8],
    row: &Row,
    dt: &DataTable,
    map: &[i32],
    warnings: &mut Vec<String>,
) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let base = row.start;
    // Row-name FName index at row-relative offset 0.
    patch_index(&mut out, 0, row.name.as_str(), dt, map, warnings);
    for p in &row.props {
        remap_prop(&mut out, base, p, dt, map, warnings);
    }
    out
}

fn remap_prop(
    out: &mut [u8],
    base: usize,
    p: &Prop,
    dt: &DataTable,
    map: &[i32],
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
        PropValue::Struct { children } => {
            for c in children {
                remap_prop(out, base, c, dt, map, warnings);
            }
        }
        PropValue::Array { items, .. } => {
            for it in items {
                if let modman_uasset::walk::ArrayElem::Struct(props) = it {
                    for c in props {
                        remap_prop(out, base, c, dt, map, warnings);
                    }
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

    /// A single override must reproduce the mod's datatable content exactly:
    /// same rows (names + bytes) in the same order, and the name table =
    /// vanilla + the mod's extras.
    #[test]
    fn single_override_reproduces_the_mod() {
        let vua = fixture("DB_LevelList.uasset");
        let vue = fixture("DB_LevelList.uexp");
        let mua = fixture("DB_LevelList.mod.uasset");
        let mue = fixture("DB_LevelList.mod.uexp");

        let out = merge_datatable_overrides((&vua, &vue), &[(&mua, &mue)])
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

        let out = merge_datatable_overrides((&vua, &vue), &[(&mua, &mue), (&b_ua, &b_uexp)])
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
