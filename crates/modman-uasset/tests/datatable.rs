//! Fixture-based gates for the cooked DataTable walker + splice editor.
//!
//! Fixtures are real PW assets (`DB_Aircraft`, `DB_ProjectWingmanLevelList`);
//! goldens were generated from the independently-verified Python walker
//! (see `~/modding/project-wingman/datatable-arch-backup/`).

use std::path::PathBuf;

use modman_uasset::edit;
use modman_uasset::walk::{resolve, ByteValue, CustomValue, DataTable, Prop, PropValue};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load(name: &str) -> DataTable {
    DataTable::load(fixture(name)).unwrap_or_else(|e| panic!("load {name}: {e}"))
}

fn read_json(name: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(fixture(name)).unwrap()).unwrap()
}

#[test]
fn aircraft_walk_invariants() {
    let dt = load("DB_Aircraft");
    assert_eq!(dt.rows.len(), 39);
    assert_eq!(dt.leftover, 0, "leftover = {}", dt.leftover);
    assert!(dt.size_mismatches.is_empty(), "{:?}", dt.size_mismatches);
    assert_eq!(dt.count_props(), 2809);
    assert_eq!(dt.top_props.len(), 1);
    assert_eq!(dt.top_props[0].name, "RowStruct");
    assert_eq!(dt.rows[0].name, "Mig-15");
    assert_eq!(dt.rows[38].name, "AV-8_2");
    assert_eq!(dt.counters.get("customstruct:Vector"), Some(&39));
}

#[test]
fn levellist_walk_invariants() {
    let dt = load("DB_ProjectWingmanLevelList");
    assert_eq!(dt.rows.len(), 43);
    assert_eq!(dt.leftover, 0, "leftover = {}", dt.leftover);
    assert!(dt.size_mismatches.is_empty(), "{:?}", dt.size_mismatches);
    assert_eq!(dt.count_props(), 1678);
    assert_eq!(dt.top_props.len(), 1);
    assert_eq!(dt.top_props[0].name, "RowStruct");
    assert_eq!(dt.rows[0].name, "PGF_2017");
}

fn structure(rows: &[modman_uasset::walk::Row]) -> serde_json::Value {
    use serde_json::json;
    let rs: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            json!([
                r.name.as_str(),
                r.props.iter().map(prop_json).collect::<Vec<_>>()
            ])
        })
        .collect();
    json!({ "rows": rs })
}

fn prop_json(p: &Prop) -> serde_json::Value {
    use serde_json::json;
    match &p.value {
        PropValue::Custom(_) => {
            json!([
                p.name.as_str(),
                format!("CustomStruct:{}", p.custom.clone().unwrap_or_default())
            ])
        }
        PropValue::Struct { children } => {
            json!([
                p.name.as_str(),
                format!(
                    "StructProperty:{}",
                    p.struct_type.clone().unwrap_or_default()
                ),
                children.iter().map(prop_json).collect::<Vec<_>>()
            ])
        }
        PropValue::Array {
            elem_type, count, ..
        } => {
            json!([p.name.as_str(), format!("ArrayProperty:{elem_type}"), count])
        }
        _ => json!([p.name.as_str(), p.type_name.as_str()]),
    }
}

fn first_diff(a: &serde_json::Value, b: &serde_json::Value, path: String) -> Option<String> {
    use serde_json::Value;
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: array len {} vs {}", x.len(), y.len()));
            }
            for (i, (xx, yy)) in x.iter().zip(y).enumerate() {
                if let Some(d) = first_diff(xx, yy, format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            None
        }
        (Value::Object(x), Value::Object(y)) => {
            if x.len() != y.len() {
                return Some(format!(
                    "{path}: object key count {} vs {}",
                    x.len(),
                    y.len()
                ));
            }
            for (k, xx) in x {
                match y.get(k) {
                    Some(yy) => {
                        if let Some(d) = first_diff(xx, yy, format!("{path}.{k}")) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}: missing key {k}")),
                }
            }
            None
        }
        _ => {
            if a != b {
                Some(format!("{path}: {a} vs {b}"))
            } else {
                None
            }
        }
    }
}

fn check_structure(golden_name: &str, fixture_name: &str) {
    let dt = load(fixture_name);
    let built = structure(&dt.rows);
    let golden = read_json(golden_name);
    if built != golden {
        let d = first_diff(&built, &golden, "root".to_string())
            .unwrap_or_else(|| "(unknown)".to_string());
        panic!("{fixture_name} structure mismatch: {d}");
    }
}

#[test]
fn structure_matches_golden() {
    check_structure("db_aircraft.structure.json", "DB_Aircraft");
    check_structure("levellist.structure.json", "DB_ProjectWingmanLevelList");
}

#[test]
fn golden_values_match() {
    let vals = read_json("golden_values.json");
    let entries = vals.as_array().unwrap();
    for entry in entries {
        let table = entry["table"].as_str().unwrap();
        let row = entry["row"].as_str().unwrap();
        let kind = entry["kind"].as_str().unwrap();
        let path: Vec<&str> = entry["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let expected = &entry["value"];
        let dt = load(table);
        let prop = resolve(&dt, row, path.as_slice())
            .unwrap_or_else(|| panic!("resolve {table} {row} {path:?}"));
        let c = format!("{table} {row} {}", path.join("/"));
        match kind {
            "f" => match &prop.value {
                PropValue::Float(v) => {
                    let e = expected.as_f64().unwrap();
                    assert!((*v as f64 - e).abs() < 1e-4, "{c}: {v} vs {e}");
                }
                other => panic!("{c}: expected float, got {other:?}"),
            },
            "i" => match &prop.value {
                PropValue::Int(v) => assert_eq!(*v as i64, expected.as_i64().unwrap(), "{c}"),
                other => panic!("{c}: expected int, got {other:?}"),
            },
            "b" => match &prop.value {
                PropValue::Bool(v) => assert_eq!(*v, expected.as_bool().unwrap(), "{c}"),
                other => panic!("{c}: expected bool, got {other:?}"),
            },
            "s" => match &prop.value {
                PropValue::Str(v) => assert_eq!(v, expected.as_str().unwrap(), "{c}"),
                other => panic!("{c}: expected str, got {other:?}"),
            },
            "nm" => match &prop.value {
                PropValue::Name(v) => assert_eq!(v.value, expected.as_str().unwrap(), "{c}"),
                other => panic!("{c}: expected name, got {other:?}"),
            },
            "ob" => match &prop.value {
                PropValue::Object(v) => assert_eq!(*v as i64, expected.as_i64().unwrap(), "{c}"),
                other => panic!("{c}: expected object, got {other:?}"),
            },
            "ben" => match &prop.value {
                PropValue::Byte {
                    value: ByteValue::FName(v),
                    ..
                } => {
                    assert_eq!(v.value, expected.as_str().unwrap(), "{c}");
                }
                other => panic!("{c}: expected byte-fname, got {other:?}"),
            },
            "by" => match &prop.value {
                PropValue::Byte {
                    value: ByteValue::Byte(v),
                    ..
                } => {
                    assert_eq!(*v as i64, expected.as_i64().unwrap(), "{c}");
                }
                other => panic!("{c}: expected byte, got {other:?}"),
            },
            "tx" => match &prop.value {
                PropValue::Text(t) => {
                    let exp = expected.as_array().unwrap();
                    assert_eq!(t.key.as_deref(), exp[0].as_str(), "{c} (key)");
                    assert_eq!(t.source.as_deref(), exp[1].as_str(), "{c} (source)");
                }
                other => panic!("{c}: expected text, got {other:?}"),
            },
            "cf" => match &prop.value {
                PropValue::Custom(CustomValue::Floats(vals)) => {
                    let exp: Vec<f64> = expected
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_f64().unwrap())
                        .collect();
                    assert_eq!(vals.len(), exp.len(), "{c}");
                    for (a, b) in vals.iter().zip(&exp) {
                        assert!((*a as f64 - b).abs() < 1e-4, "{c}: {a} vs {b}");
                    }
                }
                other => panic!("{c}: expected custom floats, got {other:?}"),
            },
            "ac" => match &prop.value {
                PropValue::Array { count, .. } => {
                    assert_eq!(*count as i64, expected.as_i64().unwrap(), "{c}");
                }
                other => panic!("{c}: expected array, got {other:?}"),
            },
            other => panic!("{c}: unknown kind {other}"),
        }
    }
    assert!(entries.len() >= 28, "golden entries: {}", entries.len());
}

#[test]
fn splice_float_edit_is_minimal_and_verified() {
    let ua = std::fs::read(fixture("DB_Aircraft.uasset")).unwrap();
    let mut ue = std::fs::read(fixture("DB_Aircraft.uexp")).unwrap();
    let dt = DataTable::walk_bytes(&ua, &ue).unwrap();
    let prop = resolve(&dt, "F-15C", &["BaseStats", "MaxSpeed"]).unwrap();
    match &prop.value {
        PropValue::Float(v) => assert_eq!(*v, 2500.0),
        other => panic!("expected float, got {other:?}"),
    }
    let span = edit::edit_span(prop).unwrap();
    assert_eq!(span, (40356, 40360));
    let orig = ue.clone();
    edit::set_f32(&mut ue, prop, 3000.0).unwrap();
    let diff: Vec<usize> = orig
        .iter()
        .zip(ue.iter())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        diff,
        vec![40357, 40358],
        "exactly the 2 differing value bytes"
    );
    assert_eq!(&ue[40356..40360], [0x00u8, 0x80, 0x3b, 0x45].as_slice());

    let dt2 = DataTable::walk_bytes(&ua, &ue).unwrap();
    assert_eq!(dt2.leftover, 0);
    assert!(dt2.size_mismatches.is_empty(), "{:?}", dt2.size_mismatches);
    match &resolve(&dt2, "F-15C", &["BaseStats", "MaxSpeed"])
        .unwrap()
        .value
    {
        PropValue::Float(v) => assert_eq!(*v, 3000.0),
        other => panic!("expected float, got {other:?}"),
    }
}

#[test]
fn splice_int_edit_is_minimal_and_verified() {
    let ua = std::fs::read(fixture("DB_Aircraft.uasset")).unwrap();
    let mut ue = std::fs::read(fixture("DB_Aircraft.uexp")).unwrap();
    let dt = DataTable::walk_bytes(&ua, &ue).unwrap();
    let prop = resolve(&dt, "Mig-15", &["Price"]).unwrap();
    match &prop.value {
        PropValue::Int(v) => assert_eq!(*v, 17500),
        other => panic!("expected int, got {other:?}"),
    }
    let span = edit::edit_span(prop).unwrap();
    assert_eq!(span, (618, 622));
    let orig = ue.clone();
    edit::set_i32(&mut ue, prop, 22222).unwrap();
    let diff: Vec<usize> = orig
        .iter()
        .zip(ue.iter())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(diff, vec![618, 619]);
    assert_eq!(&ue[618..622], [0xceu8, 0x56, 0x00, 0x00].as_slice());

    let dt2 = DataTable::walk_bytes(&ua, &ue).unwrap();
    match &resolve(&dt2, "Mig-15", &["Price"]).unwrap().value {
        PropValue::Int(v) => assert_eq!(*v, 22222),
        other => panic!("expected int, got {other:?}"),
    }
}

#[test]
fn splice_bool_edit_flips_single_byte() {
    let ua = std::fs::read(fixture("DB_Aircraft.uasset")).unwrap();
    let mut ue = std::fs::read(fixture("DB_Aircraft.uexp")).unwrap();
    let dt = DataTable::walk_bytes(&ua, &ue).unwrap();
    let prop = resolve(&dt, "Mig-15", &["FixedLoadout"]).unwrap();
    match &prop.value {
        PropValue::Bool(v) => assert!(*v),
        other => panic!("expected bool, got {other:?}"),
    }
    let (a, b) = edit::edit_span(prop).unwrap();
    assert_eq!(b - a, 1);
    let orig = ue.clone();
    edit::set_bool(&mut ue, prop, false).unwrap();
    let diff: Vec<usize> = orig
        .iter()
        .zip(ue.iter())
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(diff, vec![a]);

    let dt2 = DataTable::walk_bytes(&ua, &ue).unwrap();
    match &resolve(&dt2, "Mig-15", &["FixedLoadout"]).unwrap().value {
        PropValue::Bool(v) => assert!(!*v),
        other => panic!("expected bool, got {other:?}"),
    }
}
