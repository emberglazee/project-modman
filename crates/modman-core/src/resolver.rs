//! Fragment resolution — execute parsed fragment chains against a walked
//! DataTable, producing the nodes a patch would apply to.
//!
//! Mirrors the C# fragment semantics (`SicarioPatch.Engine/Fragments/*` +
//! `CoreExtensions.Matches`/`ToValueString`): the chain starts from the rows
//! (the DataTable loader's initial set) and each fragment filters/descends.
//! Matching rules follow the C# exactly, including its quirks:
//!   - `StructName`/`StructProperty` partial matching via trailing `*`
//!   - `StructMatch` compares names/values **case-sensitively** with
//!     `*`-wildcards only where `allowWildcardMatch` is set (values, not names)
//!   - `PropertyValue` compares the raw value string **case-insensitively**;
//!     `ArrayProperty` compares the element COUNT; `ByteProperty` compares a
//!     name-table index (we compare resolved names, which is equivalent)
//!   - `<Type=N>` numeric constraints arrive as `NumberCollection` and use
//!     numeric equality on the raw value string

use crate::fragment::Fragment;
use modman_uasset::walk::{ArrayElem, ByteValue, DataTable, Prop, PropValue, Row};

/// A node in the current resolution set.
#[derive(Clone, Copy)]
pub enum Node<'a> {
    Row(&'a Row),
    Prop(&'a Prop),
    Item {
        elem: &'a ArrayElem,
        elem_type: &'a str,
    },
}

impl<'a> Node<'a> {
    /// Property type (`StructProperty` for rows; the element type for items).
    pub fn type_name(&self) -> &'a str {
        match self {
            Node::Row(_) => "StructProperty",
            Node::Prop(p) => &p.type_name,
            Node::Item { elem_type, .. } => elem_type,
        }
    }

    /// Node name (empty for array items — matching UAssetAPI's model where
    /// item names are not meaningful).
    pub fn name(&self) -> &'a str {
        match self {
            Node::Row(r) => &r.name,
            Node::Prop(p) => &p.name,
            Node::Item { .. } => "",
        }
    }

    /// Struct children (rows, struct properties, struct items).
    pub fn children(&self) -> Option<&'a [Prop]> {
        match self {
            Node::Row(r) => Some(&r.props),
            Node::Prop(p) => match &p.value {
                PropValue::Struct { children } => Some(children),
                _ => None,
            },
            Node::Item {
                elem: ArrayElem::Struct(props),
                ..
            } => Some(props),
            _ => None,
        }
    }

    /// Raw value string — UAssetAPI `ToString()`/`RawValue.ToString()` equivalent.
    pub fn value_string(&self) -> Option<String> {
        match self {
            Node::Prop(p) => prop_value_string(&p.value),
            Node::Item {
                elem: ArrayElem::Prim { value, .. },
                ..
            } => prop_value_string(value),
            _ => None,
        }
    }

    /// The byte span a same-size splice may overwrite (payload-relative).
    pub fn value_span(&self) -> Option<(usize, usize)> {
        match self {
            Node::Prop(p) => Some((p.vstart, p.vend)),
            Node::Item {
                elem: ArrayElem::Prim { start, end, .. },
                ..
            } => Some((*start, *end)),
            Node::Item {
                elem: ArrayElem::Custom { start, end, .. },
                ..
            } => Some((*start, *end)),
            _ => None,
        }
    }

    /// Short human-readable description (for reports).
    pub fn describe(&self) -> String {
        match self {
            Node::Row(r) => format!("row {}", r.name),
            Node::Prop(p) => format!("prop {} <{}>", p.name, p.type_name),
            Node::Item { elem_type, .. } => format!("item <{elem_type}>"),
        }
    }
}

/// Resolve a fragment chain against a DataTable. Starts from the rows
/// (the `datatable` loader's initial set, mirroring `DataTableTypeLoader`).
pub fn resolve<'a>(dt: &'a DataTable, fragments: &[Fragment]) -> Vec<Node<'a>> {
    let mut set: Vec<Node<'a>> = dt.rows.iter().map(Node::Row).collect();
    for f in fragments {
        set = apply(dt, &set, f);
        if set.is_empty() {
            break;
        }
    }
    set
}

/// Parse a template and resolve it in one step.
pub fn resolve_template<'a>(
    dt: &'a DataTable,
    template: &str,
) -> Result<Vec<Node<'a>>, crate::fragment::ParseError> {
    let ctx = crate::fragment::parse_template(template)?;
    Ok(resolve(dt, &ctx.fragments))
}

fn apply<'a>(dt: &'a DataTable, input: &[Node<'a>], fragment: &Fragment) -> Vec<Node<'a>> {
    match fragment {
        Fragment::Any => input.to_vec(),

        Fragment::Flatten => input
            .iter()
            .filter_map(|n| n.children())
            .flatten()
            .map(Node::Prop)
            .collect(),

        Fragment::StructName {
            name,
            invert,
            partial,
        } => input
            .iter()
            .copied()
            .filter(|n| {
                let m = if *partial {
                    n.name().starts_with(name.as_str())
                } else {
                    n.name() == name
                };
                m ^ invert
            })
            .collect(),

        Fragment::StructProperty { name, partial } => input
            .iter()
            .filter_map(|n| n.children())
            .flatten()
            .filter(|c| {
                if *partial {
                    c.name.starts_with(name.as_str())
                } else {
                    c.name == *name
                }
            })
            .map(Node::Prop)
            .collect(),

        Fragment::StructMatch {
            struct_type,
            prop_name,
            prop_value,
        } => input
            .iter()
            .copied()
            .filter(|n| {
                let type_ok = match struct_type.as_deref() {
                    None => true,
                    Some(t) => t.is_empty() || t == "*" || struct_type_of(n) == Some(t),
                };
                if !type_ok {
                    return false;
                }
                n.children().is_some_and(|children| {
                    children.iter().any(|c| {
                        matches_str(&c.name, prop_name, false)
                            && matches_str(
                                &prop_value_string(&c.value).unwrap_or_default(),
                                prop_value,
                                true,
                            )
                    })
                })
            })
            .collect(),

        Fragment::ArrayIndex(index) => input
            .get(*index)
            .copied()
            .map(|n| vec![n])
            .unwrap_or_default(),

        Fragment::ArrayPropertyIndex(index) => {
            // C#: if ALL inputs are ArrayProperties, take item N from each;
            // otherwise treat N as an index into the input set.
            let all_arrays = !input.is_empty()
                && input.iter().all(
                    |n| matches!(n, Node::Prop(p) if matches!(p.value, PropValue::Array { .. })),
                );
            if all_arrays {
                input
                    .iter()
                    .filter_map(|n| match n {
                        Node::Prop(p) => match &p.value {
                            PropValue::Array {
                                elem_type, items, ..
                            } => items.get(*index).map(|e| Node::Item { elem: e, elem_type }),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect()
            } else {
                input
                    .get(*index)
                    .copied()
                    .map(|n| vec![n])
                    .unwrap_or_default()
            }
        }

        Fragment::ArrayFlatten => input
            .iter()
            .filter_map(|n| match n {
                Node::Prop(p) => match &p.value {
                    PropValue::Array {
                        elem_type, items, ..
                    } => Some(items.iter().map(move |e| Node::Item { elem: e, elem_type })),
                    _ => None,
                },
                _ => None,
            })
            .flatten()
            .collect(),

        Fragment::PropertyType(t) => input
            .iter()
            .copied()
            .filter(|n| n.type_name() == t)
            .collect(),

        Fragment::PropertyValue { prop_type, value } => input
            .iter()
            .copied()
            .filter(|n| {
                if n.type_name() != prop_type {
                    return false;
                }
                let Some(query) = value else {
                    return true;
                };
                match n {
                    Node::Prop(p) => match &p.value {
                        // ArrayProperty: compare element count.
                        PropValue::Array { count, .. } => query.parse::<i32>().ok() == Some(*count),
                        // ByteProperty: C# compares a name-table index; we compare
                        // resolved names (equivalent through the same table).
                        PropValue::Byte { value: bv, .. } => byte_matches_query(dt, bv, query),
                        _ => default_value_match(n.value_string(), query),
                    },
                    _ => default_value_match(n.value_string(), query),
                }
            })
            .collect(),

        Fragment::NumberCollection { prop_type, values } => input
            .iter()
            .copied()
            .filter(|n| {
                if n.type_name() != prop_type {
                    return false;
                }
                n.value_string()
                    .and_then(|s| s.parse::<f64>().ok())
                    .is_some_and(|d| values.contains(&d))
            })
            .collect(),

        Fragment::EnumValue { enum_type, value } => input
            .iter()
            .copied()
            .filter(|n| {
                let Node::Prop(p) = n else {
                    return false;
                };
                let PropValue::Byte {
                    enum_name,
                    value: bv,
                } = &p.value
                else {
                    return false;
                };
                if enum_name != enum_type {
                    return false;
                }
                match value {
                    None => true,
                    Some(member) => {
                        let full = format!("{enum_type}::{member}");
                        match bv {
                            ByteValue::FName(nr) => nr.value == full,
                            ByteValue::Byte(b) => {
                                dt.names.iter().position(|x| x == &full) == Some(*b as usize)
                            }
                        }
                    }
                }
            })
            .collect(),
    }
}

fn struct_type_of<'a>(n: &Node<'a>) -> Option<&'a str> {
    match n {
        Node::Prop(p) => p.struct_type.as_deref(),
        Node::Item {
            elem: ArrayElem::Custom { kind, .. },
            ..
        } => Some(kind),
        // Row struct types are not resolved by the walker (no import table);
        // explicit type constraints on rows therefore never match, which is
        // fine for the corpus (no `{Type:{...}}` usage on rows).
        _ => None,
    }
}

/// Port of `CoreExtensions.Matches` (note: the C# ternary means
/// `(wildcard || partial) ? starts_with(trimmed) : exact`).
fn matches_str(value: &str, match_value: &str, allow_wildcard: bool) -> bool {
    let wildcard_match = allow_wildcard && match_value == "*" && value.trim().is_empty();
    let partial = match_value.ends_with('*');
    if wildcard_match || partial {
        value.starts_with(match_value.trim_end_matches('*'))
    } else {
        value == match_value
    }
}

/// Default `PropertyValueFragment` compare: case-insensitive equality, with
/// `*` meaning "non-empty and not zero".
fn default_value_match(value: Option<String>, query: &str) -> bool {
    match value {
        Some(s) => {
            if query == "*" {
                !s.trim().is_empty() && s != "0"
            } else {
                s.eq_ignore_ascii_case(query)
            }
        }
        None => false,
    }
}

/// ByteProperty query compare (name-table index semantics).
fn byte_matches_query(dt: &DataTable, bv: &ByteValue, query: &str) -> bool {
    let target = dt.names.iter().position(|n| n == query);
    match bv {
        ByteValue::Byte(b) => target == Some(*b as usize),
        ByteValue::FName(nr) => nr.index >= 0 && target == Some(nr.index as usize),
    }
}

/// Raw value string per UAssetAPI `ToString()` overrides.
fn prop_value_string(v: &PropValue) -> Option<String> {
    Some(match v {
        PropValue::Int(i) => i.to_string(),
        PropValue::Float(f) => format_f32(*f),
        PropValue::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        PropValue::Str(s) => s.clone(),
        PropValue::Name(n) => n.value.clone(),
        PropValue::Object(i) => i.to_string(),
        PropValue::Byte { value, .. } => match value {
            ByteValue::Byte(b) => b.to_string(),
            ByteValue::FName(n) => n.value.clone(),
        },
        // TextPropertyData.ToString joins the text parts; approximate with source/key.
        PropValue::Text(t) => t
            .source
            .clone()
            .or_else(|| t.key.clone())
            .unwrap_or_default(),
        PropValue::Struct { .. } | PropValue::Array { .. } | PropValue::Custom(_) => return None,
    })
}

/// .NET-style shortest float formatting (matches `float.ToString()` for the
/// value ranges in PW tables).
fn format_f32(f: f32) -> String {
    format!("{f}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragment::parse_template;

    fn dt() -> DataTable {
        let stem = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft"
        );
        DataTable::load(stem).unwrap()
    }

    fn resolve_t<'a>(dt: &'a DataTable, t: &str) -> Vec<Node<'a>> {
        let ctx = parse_template(t).unwrap();
        resolve(dt, &ctx.fragments)
    }

    #[test]
    fn spear_fixed_loadout_span() {
        let dt = dt();
        let nodes = resolve_t(&dt, "datatable:['SPEAR'].{'FixedLoadout*'}");
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].name(),
            "FixedLoadout_136_FE39646742CF06F6896D89A57BEF2430"
        );
        assert_eq!(nodes[0].value_span(), Some((83882, 83883)));
        assert_eq!(nodes[0].value_string().as_deref(), Some("True"));
    }

    #[test]
    fn spear_hardpoint_item() {
        let dt = dt();
        let nodes = resolve_t(
            &dt,
            "datatable:['SPEAR'].{'HardpointCompatibilityList*'}.[[3]].<StrProperty='rgps'>",
        );
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].value_string().as_deref(), Some("rgps"));
        assert_eq!(nodes[0].value_span(), Some((83714, 83719)));

        let raw = resolve_t(
            &dt,
            "datatable:['SPEAR'].{'HardpointCompatibilityList*'}.[[3]]",
        );
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].value_span(), Some((83714, 83719)));
    }

    #[test]
    fn f15c_max_speed_and_numeric_collection() {
        let dt = dt();
        let nodes = resolve_t(&dt, "datatable:['F-15C'].{'BaseStats*'}.{'MaxSpeed*'}");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].value_span(), Some((40356, 40360)));
        assert_eq!(nodes[0].value_string().as_deref(), Some("2500"));

        let hits = resolve_t(
            &dt,
            "datatable:['F-15C'].{'BaseStats*'}.{'MaxSpeed*'}.<FloatProperty='2500'>",
        );
        assert_eq!(hits.len(), 1);
        let miss = resolve_t(
            &dt,
            "datatable:['F-15C'].{'BaseStats*'}.{'MaxSpeed*'}.<FloatProperty='2501'>",
        );
        assert!(miss.is_empty());

        // Unquoted numbers go through NumberCollection (numeric equality).
        let numeric = resolve_t(
            &dt,
            "datatable:['F-15C'].{'BaseStats*'}.{'MaxSpeed*'}.<FloatProperty=2500>",
        );
        assert_eq!(numeric.len(), 1);
        let numeric_miss = resolve_t(
            &dt,
            "datatable:['F-15C'].{'BaseStats*'}.{'MaxSpeed*'}.<FloatProperty=2501>",
        );
        assert!(numeric_miss.is_empty());
    }

    #[test]
    fn any_flatten_and_type() {
        let dt = dt();
        assert_eq!(resolve_t(&dt, "datatable:[*]").len(), 39);

        let flat = resolve_t(&dt, "datatable:{'HardpointCompatibilityList*'}.[[*]]");
        assert!(!flat.is_empty());
        assert!(flat.iter().all(|n| n.type_name() == "StrProperty"));

        let structs = resolve_t(&dt, "datatable:['SPEAR'].{'BaseStats*'}.<StructProperty>");
        assert_eq!(structs.len(), 1);
    }

    #[test]
    fn invert_and_index() {
        let dt = dt();
        let inv = resolve_t(&dt, "datatable:[!'F-15C']");
        assert_eq!(inv.len(), 38);
        assert!(inv.iter().all(|n| n.name() != "F-15C"));

        let first = resolve_t(&dt, "datatable:['SPEAR'].[0]");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].name(), "SPEAR");
    }

    #[test]
    fn array_count_match_uses_quoted_value() {
        let dt = dt();
        // `<ArrayProperty='4'>` (quoted) → PropertyValueFragment count compare.
        let hit = resolve_t(
            &dt,
            "datatable:['SPEAR'].{'HardpointCompatibilityList*'}.<ArrayProperty='4'>",
        );
        assert_eq!(hit.len(), 1);
        let miss = resolve_t(
            &dt,
            "datatable:['SPEAR'].{'HardpointCompatibilityList*'}.<ArrayProperty='5'>",
        );
        assert!(miss.is_empty());
    }

    #[test]
    fn struct_match_by_child_value() {
        let dt = dt();
        // Rows whose direct child `FixedLoadout*` is `True` — Mig-15, SPEAR, PW-001.
        let rows = resolve_t(&dt, "datatable:{{'FixedLoadout*'='True'}}");
        let names: Vec<&str> = rows.iter().map(|n| n.name()).collect();
        assert_eq!(names.len(), 3, "got {names:?}");
        assert!(names.contains(&"SPEAR"));

        // Case-sensitive value compare (C# `Matches`): lowercase must NOT match.
        let lower = resolve_t(&dt, "datatable:{{'FixedLoadout*'='true'}}");
        assert!(lower.is_empty());
    }

    #[test]
    fn enum_member_search() {
        let dt = dt();
        // Find an enum byte property in the fixture (with its struct parent path),
        // then resolve it by a type-only enum chain.
        let mut found: Option<(String, Vec<String>, String, String)> = None;
        for r in &dt.rows {
            let mut parents = Vec::new();
            scan_for_enum(&r.name, &r.props, &mut parents, &mut found);
            if found.is_some() {
                break;
            }
        }
        let (row, parents, prop_base, enum_name) =
            found.expect("fixture has an enum byte property");
        let mut t = format!("datatable:['{row}']");
        for par in &parents {
            t.push_str(&format!(".{{'{par}*'}}"));
        }
        t.push_str(&format!(".{{'{prop_base}*'}}.<{enum_name}::>"));
        let nodes = resolve_t(&dt, &t);
        assert!(!nodes.is_empty(), "chain failed: {t}");
        assert!(nodes.iter().all(|n| n.type_name() == "ByteProperty"));
    }

    fn scan_for_enum(
        row: &str,
        props: &[Prop],
        parents: &mut Vec<String>,
        out: &mut Option<(String, Vec<String>, String, String)>,
    ) {
        for p in props {
            if let PropValue::Byte { enum_name, .. } = &p.value {
                if !enum_name.is_empty() {
                    let base = p.name.split('_').next().unwrap().to_string();
                    *out = Some((row.to_string(), parents.clone(), base, enum_name.clone()));
                    return;
                }
            }
            if let PropValue::Struct { children } = &p.value {
                parents.push(p.name.split('_').next().unwrap().to_string());
                scan_for_enum(row, children, parents, out);
                parents.pop();
                if out.is_some() {
                    return;
                }
            }
        }
    }
}
