//! objectRef patch application (array-append form) — ports the C#
//! `ObjectRefPatchType` (via UAssetAPI's link machinery). Oracle-verified
//! against the skin-merge run (`DB_Aircraft.skin.merged.*` fixtures).
//!
//! Semantics per patch (matching `ObjectRefPatchType.RunPatch`):
//! * The target must resolve to an `ArrayProperty<ObjectProperty>` with at
//!   least one existing element (else the C# silently does nothing).
//! * The first element's link supplies the reference name link; that link's
//!   `Linkage` points at the outer (package) link.
//! * Two links are appended: a path link (`{outer Base/Class, 0, path}`) and
//!   a name link (`{ref Base/Class, path link index, name}`), unless an
//!   identical-property link already exists (then it is re-appended as-is).
//! * The element appended to the array is the new name link's index (i32).

use crate::apply::ApplyError;
use crate::manifest::WingmanMod;
use crate::resolver::{resolve, Node};
use modman_uasset::rewrite::{NewLink, RewritePlan};
use modman_uasset::walk::{ArrayElem, DataTable, PropValue};
use std::collections::BTreeMap;

pub struct ObjectRefEdits {
    pub uasset: Vec<u8>,
    pub uexp: Vec<u8>,
}

fn rd_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Apply the mod's objectRef patches for `target`. Returns `None` when no
/// patch applied (no matches / empty arrays).
pub fn apply_object_refs(
    dt: &DataTable,
    uasset: &[u8],
    uexp: &[u8],
    modm: &WingmanMod,
    target: &str,
) -> Result<Option<ObjectRefEdits>, ApplyError> {
    let Some(sets) = modm.asset_patches.get(target) else {
        return Ok(None);
    };

    // Import (link) table from the uasset header.
    let import_count = rd_i32(uasset, 65) as usize;
    let import_off = rd_i32(uasset, 69) as usize;
    let link_at = |i: usize| -> (u64, u64, i32, i32, i32) {
        let o = import_off + i * 28;
        (
            u64::from_le_bytes(uasset[o..o + 8].try_into().unwrap()),
            u64::from_le_bytes(uasset[o + 8..o + 16].try_into().unwrap()),
            rd_i32(uasset, o + 16),
            rd_i32(uasset, o + 20),
            rd_i32(uasset, o + 24),
        )
    };

    let mut names: Vec<String> = dt.names.clone();
    let mut name_additions: Vec<String> = Vec::new();
    let mut links: Vec<NewLink> = Vec::new();
    // insert offset -> concatenated element bytes
    let mut inserts: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut count_bumps: Vec<usize> = Vec::new();
    let mut size_bumps: Vec<usize> = Vec::new();
    let mut delta: i64 = 0;

    let intern = |names: &mut Vec<String>, additions: &mut Vec<String>, s: &str| -> i32 {
        if let Some(i) = names.iter().position(|n| n == s) {
            return i as i32;
        }
        names.push(s.to_string());
        additions.push(s.to_string());
        (names.len() - 1) as i32
    };

    // SearchForLink(int property): find an existing (or just-appended) link
    // whose Property matches.
    let find_link = |links: &[NewLink], property: i32| -> Option<NewLink> {
        links.iter().find(|l| l.property == property).copied()
    };

    for set in sets {
        for patch in &set.patches {
            if !patch.patch_type.eq_ignore_ascii_case("objectRef") {
                continue;
            }
            let op = crate::patch::parse_patch_value(&patch.patch_type, &patch.value)
                .map_err(|e| ApplyError::Value(e.to_string()))?;
            let crate::patch::PatchOp::ObjectRef {
                object_name,
                object_path,
            } = op
            else {
                continue;
            };
            let ctx = crate::fragment::parse_template(&patch.template)
                .map_err(|e| ApplyError::Parse(e.to_string()))?;
            for node in resolve(dt, &ctx.fragments) {
                let Node::Prop(p) = node else { continue };
                let PropValue::Array { items, .. } = &p.value else {
                    continue;
                };
                // C#: needs a first existing element to borrow links from.
                let Some(ArrayElem::Prim {
                    value: PropValue::Object(first_link),
                    ..
                }) = items.first()
                else {
                    continue;
                };
                let Some(ArrayElem::Prim { end: last_end, .. }) = items.last() else {
                    continue;
                };

                let ref_pos = (first_link.unsigned_abs() as usize) - 1;
                let (ref_base, ref_class, ref_linkage, _, _) = link_at(ref_pos);
                let outer_pos = (ref_linkage.unsigned_abs() as usize) - 1;
                let (outer_base, outer_class, _, _, _) = link_at(outer_pos);

                let path_idx = intern(&mut names, &mut name_additions, &object_path);
                let name_idx = intern(&mut names, &mut name_additions, &object_name);

                // Path link: reuse an existing same-property link if present,
                // else copy Base/Class from the outer reference link.
                let path_link = find_link(&links, path_idx).unwrap_or(NewLink {
                    base: outer_base,
                    class: outer_class,
                    linkage: 0,
                    property: path_idx,
                    target: 0,
                });
                let next_pos = import_count + links.len();
                let path_link_index = -(next_pos as i32) - 1;
                links.push(path_link);

                // Name link: reuse if present, else copy from the reference
                // name link and point at the new path link.
                let name_link = find_link(&links, name_idx).unwrap_or(NewLink {
                    base: ref_base,
                    class: ref_class,
                    linkage: path_link_index,
                    property: name_idx,
                    target: 0,
                });
                let next_pos = import_count + links.len();
                let name_link_index = -(next_pos as i32) - 1;
                links.push(name_link);

                inserts
                    .entry(*last_end)
                    .or_default()
                    .extend_from_slice(&name_link_index.to_le_bytes());
                count_bumps.push(p.vstart);
                size_bumps.push(p.start + 16);
                delta += 4;
            }
        }
    }

    if delta == 0 {
        return Ok(None);
    }

    // Apply uexp edits: inserts descending by offset (equal offsets merged
    // above, preserving patch order), then the count/size bumps (their
    // offsets precede the inserts, so they are unaffected).
    let mut new_uexp = uexp.to_vec();
    for (off, bytes) in inserts.iter().rev() {
        new_uexp.splice(*off..*off, bytes.iter().copied());
    }
    for off in &count_bumps {
        let v = rd_i32(&new_uexp, *off) + 1;
        new_uexp[*off..*off + 4].copy_from_slice(&v.to_le_bytes());
    }
    for off in &size_bumps {
        let v = rd_i32(&new_uexp, *off) + 4;
        new_uexp[*off..*off + 4].copy_from_slice(&v.to_le_bytes());
    }

    let new_uasset = modman_uasset::rewrite::rewrite_uasset(
        uasset,
        &RewritePlan {
            name_append: name_additions,
            link_append: links,
            uexp_delta: delta,
        },
    )
    .map_err(|e| ApplyError::Asset(e.to_string()))?;

    Ok(Some(ObjectRefEdits {
        uasset: new_uasset,
        uexp: new_uexp,
    }))
}

/// True when the mod carries any objectRef patches for `target`.
pub fn has_object_refs(modm: &WingmanMod, target: &str) -> bool {
    modm.asset_patches.get(target).is_some_and(|sets| {
        sets.iter().any(|s| {
            s.patches
                .iter()
                .any(|p| p.patch_type.eq_ignore_ascii_case("objectRef"))
        })
    })
}
