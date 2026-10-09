//! Temporary inspection helpers for skin-system debugging.
use modman_uasset::walk::{ArrayElem, DataTable, PropValue};

#[test]
fn dump_ajs37_row() {
    let stem = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/DB_Aircraft");
    let dt = DataTable::load(stem).unwrap();
    let row = dt.rows.iter().find(|r| r.name == "AJS-37").unwrap();
    println!("=== AJS-37 row props ===");
    for p in &row.props {
        let detail = match &p.value {
            PropValue::Array {
                elem_type,
                count,
                items,
            } => {
                let vals: Vec<String> = items
                    .iter()
                    .take(8)
                    .map(|it| match it {
                        ArrayElem::Prim {
                            value: PropValue::Object(v),
                            start,
                            end,
                        } => format!("Object({v}) @{start}..{end}"),
                        other => format!("{other:?}").chars().take(60).collect(),
                    })
                    .collect();
                format!("Array<{elem_type}> count={count} [{vals:?}]")
            }
            PropValue::Name(n) => format!("Name({})", n.value),
            PropValue::Object(v) => format!("Object({v})"),
            PropValue::Str(s) => format!("Str({s:?})"),
            PropValue::Int(v) => format!("Int({v})"),
            PropValue::Bool(b) => format!("Bool({b})"),
            other => format!("{other:?}").chars().take(100).collect(),
        };
        println!("  {} <{}> size={} :: {}", p.name, p.type_name, p.size, detail);
    }
}
