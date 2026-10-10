//! Round-trip gate: parse → write must reproduce each locres byte-for-byte.
//! Usage: cargo run -p modman-core --example locres_roundtrip -- <file...>
fn main() {
    let mut all_ok = true;
    for f in std::env::args().skip(1) {
        match std::fs::read(&f) {
            Ok(data) => match modman_core::locres::LocresFile::parse(&data) {
                Ok(parsed) => {
                    let out = parsed.write();
                    let ok = out == data;
                    all_ok &= ok;
                    let name = f.rsplit('/').next().unwrap_or(&f);
                    println!(
                        "{name}: v{} entries={} ns={} strings={} roundtrip={}",
                        parsed.version.as_byte(),
                        parsed.entry_count(),
                        parsed.namespaces.len(),
                        parsed.strings.len(),
                        if ok { "BYTE-IDENTICAL" } else { "DIFFERS" }
                    );
                    if !ok {
                        let d = out
                            .iter()
                            .zip(data.iter())
                            .position(|(a, b)| a != b)
                            .unwrap_or(out.len().min(data.len()));
                        println!("  first diff at {d} (len {} vs {})", out.len(), data.len());
                    }
                }
                Err(e) => {
                    all_ok = false;
                    println!("{f}: PARSE ERROR: {e}");
                }
            },
            Err(e) => println!("{f}: read error {e}"),
        }
    }
    if !all_ok {
        std::process::exit(1);
    }
}
