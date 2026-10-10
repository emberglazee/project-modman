//! Extract ProjectWingman locres files for format probing.
//! Usage: cargo run -p modman-pak --example extract_locres -- <pak> <outdir>
fn main() {
    let pak_path = std::env::args().nth(1).expect("pak path");
    let out_dir = std::env::args().nth(2).expect("out dir");
    let pak = modman_pak::PakArchive::open(&pak_path).expect("open pak");
    for e in pak.files() {
        if e.ends_with(".locres") && e.contains("Localization/ProjectWingman/") {
            let short = e.rsplit('/').next().unwrap();
            let lang = e.split('/').rev().nth(1).unwrap();
            let out = format!("{out_dir}/{lang}_{short}");
            pak.extract_entry(&e, &out).expect("extract");
            println!("{}", out);
        }
    }
}
