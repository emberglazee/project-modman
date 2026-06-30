use std::fs;
use std::path::PathBuf;

#[test]
fn parse_sample_dtm() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/sample.dtm");
    let json = fs::read_to_string(path).unwrap();
    let m: modman_core::manifest::WingmanMod = serde_json::from_str(&json).unwrap();
    assert_eq!(m.id, "sample-mod");
    assert_eq!(m.sicario.group, "weapons");
    assert_eq!(m.variables.get("mult").unwrap(), "2.0");
    assert_eq!(m.asset_patches.len(), 1);

    let (path, sets) = m.asset_patches.into_iter().next().unwrap();
    assert_eq!(path, "Game/Data/Weapons.uasset");
    assert_eq!(sets[0].patches[0].patch_type, "modifyPropertyValue");
}
