use clap::{Parser, Subcommand};

mod game;

#[derive(Parser)]
#[command(
    name = "modman",
    version,
    about = "Project Modman — Project Wingman Modding Utility"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Print .pak file info
    Info {
        /// Path to the .pak file
        input: String,
    },
    /// List files in a .pak
    List {
        /// Path to the .pak file
        input: String,
        /// Prefix to strip from entry paths
        #[arg(short, long, default_value = "../../../")]
        strip_prefix: String,
    },
    /// Unpack a .pak file to a directory
    Unpack {
        /// Path to the .pak file
        input: String,
        /// Output directory. Defaults to next to input pak (without .pak extension)
        #[arg(short, long)]
        output: Option<String>,
        /// Prefix to strip from entry paths
        #[arg(short, long, default_value = "../../../")]
        strip_prefix: String,
        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
    },
    /// Pack a directory into a .pak file
    Pack {
        /// Input directory
        input: String,
        /// Output path. Defaults to input directory name + .pak
        #[arg(short, long)]
        output: Option<String>,
        /// Mount point
        #[arg(short, long, default_value = "../../../")]
        mount_point: String,
        /// PAK version (V3, V8B or V11, default: V11)
        #[arg(long, default_value = "V11")]
        version: String,
        /// Compression algorithm (zlib, gzip, zstd, lz4)
        #[arg(long)]
        compression: Option<String>,
    },
    /// Build a merged mod from Sicario patch files (drop-in replacement for Sicario CLI)
    Build {
        /// Paths to preset/mod files or directories
        preset_paths: Vec<String>,
        /// Path to the game install directory (auto-detected if omitted)
        #[arg(long)]
        install_path: Option<String>,
        /// Write the merged mod (SicarioMerge_P.pak) to this directory; omit for a preview
        #[arg(long)]
        output: Option<String>,
    },
    /// Inspect a Sicario patch file (.dtm, .dtp, or embedded _meta build request)
    Patch {
        /// Path to the patch file
        input: String,
    },
    /// Scan for installed Sicario mods in a Paks directory (or game install)
    Scan {
        /// Game install directory or Paks directory
        path: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Info { input }) => match modman_pak::PakArchive::open(&input) {
            Ok(pak) => {
                let info = pak.info();
                println!("PAK File:     {}", info.path);
                println!("Version:      {} (v{:?})", info.version, info.version_major);
                println!("Mount Point:  {}", info.mount_point);
                println!("File Count:   {}", info.file_count);
                println!("Encrypted:    {}", info.encrypted_index);
                if let Some(guid) = info.encryption_guid {
                    println!("Encryption GUID: {:032X}", guid);
                }
                if let Some(seed) = info.path_hash_seed {
                    println!("Path Hash Seed: 0x{:016X}", seed);
                }
                if info.compression.is_empty() {
                    println!("Compression:  None");
                } else {
                    println!(
                        "Compression:  {}",
                        info.compression
                            .iter()
                            .map(|c| c.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
        Some(Commands::List {
            input,
            strip_prefix,
        }) => match modman_pak::PakArchive::open(&input) {
            Ok(pak) => {
                let files = pak.files_stripped(&strip_prefix);
                for f in &files {
                    println!("{}", f);
                }
                if files.is_empty() {
                    println!("(no files matched)");
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
        Some(Commands::Unpack {
            input,
            output,
            strip_prefix,
            verbose,
        }) => {
            let output_dir = output.unwrap_or_else(|| {
                std::path::Path::new(&input)
                    .with_extension("")
                    .to_string_lossy()
                    .to_string()
            });
            match modman_pak::PakArchive::open(&input) {
                Ok(pak) => {
                    let file_count = pak.info().file_count;
                    println!(
                        "Unpacking {} files from {} to {} ...",
                        file_count, input, output_dir
                    );
                    match pak.unpack(&output_dir, &strip_prefix, verbose) {
                        Ok(()) => println!("Done! Unpacked {} files.", file_count),
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Pack {
            input,
            output,
            mount_point,
            version,
            compression,
        }) => {
            let output_path = output.unwrap_or_else(|| format!("{}.pak", input));
            let ver = match version.to_uppercase().as_str() {
                "V3" => modman_pak::Version::V3,
                "V8B" => modman_pak::Version::V8B,
                "V11" => modman_pak::Version::V11,
                _ => {
                    eprintln!(
                        "Error: unsupported version '{}'. Use V3, V8B or V11.",
                        version
                    );
                    std::process::exit(1);
                }
            };
            let comp: Option<modman_pak::Compression> =
                compression
                    .as_deref()
                    .map(|c| match c.to_lowercase().as_str() {
                        "zlib" => modman_pak::Compression::Zlib,
                        "gzip" => modman_pak::Compression::Gzip,
                        "zstd" => modman_pak::Compression::Zstd,
                        "lz4" => modman_pak::Compression::LZ4,
                        _ => {
                            eprintln!(
                            "Error: unsupported compression '{}'. Use zlib, gzip, zstd, or lz4.",
                            c
                        );
                            std::process::exit(1);
                        }
                    });
            println!(
                "Packing {} to {} (v{}, mount: {}) ...",
                input, output_path, ver, mount_point
            );
            match modman_pak::pack(&input, &output_path, ver, mount_point, comp) {
                Ok(()) => println!("Done!"),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Build {
            preset_paths,
            install_path,
            output,
        }) => {
            // Determine game path
            let game_path = install_path.clone().or_else(|| {
                crate::game::detect_game().map(|g| g.path.to_string_lossy().to_string())
            });

            match game_path {
                Some(path) => {
                    let game_paks = std::path::Path::new(&path).join("ProjectWingman/Content/Paks");
                    println!("Game: {}", path);
                    println!("Paks: {}", game_paks.display());

                    // Parse all .dtm/.dtp/.json patch files (kind-aware, lenient).
                    let mut all_mods: Vec<modman_core::manifest::WingmanMod> = Vec::new();

                    for pattern in &preset_paths {
                        match load_mods_from_path(std::path::Path::new(pattern)) {
                            Ok(mods) => {
                                for m in mods {
                                    println!("  Loaded: {} ({})", m.label(), pattern);
                                    all_mods.push(m);
                                }
                            }
                            Err(e) => eprintln!("  Load error {}: {}", pattern, e),
                        }
                    }

                    if all_mods.is_empty() {
                        eprintln!("No valid mod files found.");
                        std::process::exit(1);
                    }

                    // Apply template variables to each mod
                    for m in &mut all_mods {
                        modman_core::template::apply_variables_to_mod(m);
                    }

                    if let Some(out_dir) = output.as_ref() {
                        // === WRITE MODE: merge + emit a real mod pak ===
                        let out_dir = std::path::PathBuf::from(out_dir);
                        let staging = out_dir.join("staging");
                        let _ = std::fs::remove_dir_all(&staging);
                        std::fs::create_dir_all(&staging).unwrap();

                        for m in &all_mods {
                            if !m.file_patches.is_empty() {
                                eprintln!(
                                    "  Warning: {} has filePatches (not yet supported); skipped",
                                    m.label()
                                );
                            }
                        }

                        // Targets = union of every mod's asset patch targets.
                        let mut targets: std::collections::BTreeSet<String> =
                            std::collections::BTreeSet::new();
                        for m in &all_mods {
                            targets.extend(m.asset_patches.keys().cloned());
                        }

                        let main_pak_path = game_paks.join("pakchunk0-WindowsNoEditor.pak");
                        let pak = match modman_pak::PakArchive::open(&main_pak_path) {
                            Ok(p) => p,
                            Err(e) => {
                                eprintln!("Pak error: {}", e);
                                std::process::exit(1);
                            }
                        };
                        let all_files = pak.files();
                        let extract_dir = std::env::temp_dir().join("modman-build");
                        let _ = std::fs::remove_dir_all(&extract_dir);
                        std::fs::create_dir_all(&extract_dir).unwrap();

                        let mod_refs: Vec<&modman_core::manifest::WingmanMod> =
                            all_mods.iter().collect();
                        let mut ok = 0usize;
                        for target in &targets {
                            println!("Merging: {target}");
                            let needle = target.trim_start_matches("../../../").replace('\\', "/");
                            let uexp_needle = if needle.ends_with(".uexp") {
                                needle.clone()
                            } else if needle.ends_with(".uasset") {
                                needle.replace(".uasset", ".uexp")
                            } else {
                                format!("{needle}.uexp")
                            };
                            let uasset_needle = uexp_needle.replace(".uexp", ".uasset");
                            let find_entry = |name: &str| -> Option<String> {
                                all_files
                                    .iter()
                                    .find(|f| f.as_str() == name)
                                    .or_else(|| all_files.iter().find(|f| f.ends_with(name)))
                                    .cloned()
                            };
                            let (Some(uasset_entry), Some(uexp_entry)) =
                                (find_entry(&uasset_needle), find_entry(&uexp_needle))
                            else {
                                eprintln!("  Warning: no pak entries for '{target}'; skipped");
                                continue;
                            };
                            let stem = std::path::Path::new(&uexp_entry)
                                .file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_else(|| "asset".to_string());
                            let local_uasset = extract_dir.join(format!("{stem}.uasset"));
                            let local_uexp = extract_dir.join(format!("{stem}.uexp"));
                            if let Err(e) = pak.extract_entry(&uasset_entry, &local_uasset) {
                                eprintln!("  Extract error ({}): {}", uasset_entry, e);
                                continue;
                            }
                            if let Err(e) = pak.extract_entry(&uexp_entry, &local_uexp) {
                                eprintln!("  Extract error ({}): {}", uexp_entry, e);
                                continue;
                            }
                            let uasset = std::fs::read(&local_uasset).unwrap();
                            let uexp = std::fs::read(&local_uexp).unwrap();
                            let merged = match modman_core::merge::merge_mods(
                                &uasset, &uexp, &mod_refs, target,
                            ) {
                                Ok(m) => m,
                                Err(e) => {
                                    eprintln!("  Merge error on {target}: {e}");
                                    std::process::exit(1);
                                }
                            };
                            let out_uexp = staging.join(&needle);
                            let out_uasset =
                                staging.join(uasset_needle.replace(".uexp", ".uasset"));
                            if let Some(parent) = out_uexp.parent() {
                                std::fs::create_dir_all(parent).unwrap();
                            }
                            std::fs::write(&out_uexp, &merged.uexp).unwrap();
                            std::fs::write(&out_uasset, &merged.uasset).unwrap();
                            println!(
                                "  -> {} bytes uexp, {} bytes uasset",
                                merged.uexp.len(),
                                merged.uasset.len()
                            );
                            ok += 1;
                        }
                        if ok == 0 {
                            eprintln!("No targets merged.");
                            std::process::exit(1);
                        }
                        let pak_out = out_dir.join("SicarioMerge_P.pak");
                        match modman_pak::pack(
                            &staging,
                            &pak_out,
                            modman_pak::Version::V3,
                            "../../../".to_string(),
                            None,
                        ) {
                            Ok(()) => println!(
                                "\nWrote {} ({} target file(s) merged)",
                                pak_out.display(),
                                ok
                            ),
                            Err(e) => {
                                eprintln!("Pack error: {}", e);
                                std::process::exit(1);
                            }
                        }
                        return;
                    }

                    // Merge all mods into a single build plan
                    let first_mod = &all_mods[0];
                    let plan = match modman_core::engine::plan_build(first_mod) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Build plan error: {}", e);
                            std::process::exit(1);
                        }
                    };

                    if plan.file_patches.is_empty() {
                        println!("No asset patches to apply.");
                        return;
                    }

                    // Create temp working directory
                    let work_dir = std::env::temp_dir().join("modman-build");
                    let _ = std::fs::remove_dir_all(&work_dir);
                    std::fs::create_dir_all(&work_dir).unwrap();

                    // Find the main game pak
                    let main_pak_path = game_paks.join("pakchunk0-WindowsNoEditor.pak");
                    if !main_pak_path.exists() {
                        eprintln!("Game pak not found at: {}", main_pak_path.display());
                        std::process::exit(1);
                    }

                    // Engine preview: locate + parse each target asset from the game pak with
                    // the verified DataTable walker. Patch application and merge are the next
                    // milestone (see README, V1 scope) — nothing is written to disk until then,
                    // so no broken mod output can be produced.
                    let pak = match modman_pak::PakArchive::open(&main_pak_path) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Pak error: {}", e);
                            std::process::exit(1);
                        }
                    };
                    let all_files = pak.files();

                    let extract_dir = work_dir.join("extracted");
                    std::fs::create_dir_all(&extract_dir).unwrap();

                    let mut ok_targets = 0usize;
                    for (target_file, patches) in &plan.file_patches {
                        println!("\nProcessing: {} ({} patches)", target_file, patches.len());

                        let needle = target_file
                            .trim_start_matches("../../../")
                            .replace('\\', "/");
                        // Mods target the .uexp; the walker needs the .uasset companion too.
                        let uexp_needle = if needle.ends_with(".uexp") {
                            needle.clone()
                        } else if needle.ends_with(".uasset") {
                            needle.replace(".uasset", ".uexp")
                        } else {
                            format!("{needle}.uexp")
                        };
                        let uasset_needle = uexp_needle.replace(".uexp", ".uasset");

                        let find_entry = |name: &str| -> Option<String> {
                            all_files
                                .iter()
                                .find(|f| f.as_str() == name)
                                .or_else(|| all_files.iter().find(|f| f.ends_with(name)))
                                .cloned()
                        };

                        let (Some(uasset_entry), Some(uexp_entry)) =
                            (find_entry(&uasset_needle), find_entry(&uexp_needle))
                        else {
                            eprintln!("  Warning: no matching pak entries for '{}'", target_file);
                            continue;
                        };

                        let stem = std::path::Path::new(&uexp_entry)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "asset".to_string());
                        let local_uasset = extract_dir.join(format!("{stem}.uasset"));
                        let local_uexp = extract_dir.join(format!("{stem}.uexp"));

                        if let Err(e) = pak.extract_entry(&uasset_entry, &local_uasset) {
                            eprintln!("  Extract error ({}): {}", uasset_entry, e);
                            continue;
                        }
                        if let Err(e) = pak.extract_entry(&uexp_entry, &local_uexp) {
                            eprintln!("  Extract error ({}): {}", uexp_entry, e);
                            continue;
                        }

                        let ua = match std::fs::read(&local_uasset) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!("  Read error: {}", e);
                                continue;
                            }
                        };
                        let ue = match std::fs::read(&local_uexp) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!("  Read error: {}", e);
                                continue;
                            }
                        };
                        match modman_uasset::walk::DataTable::walk_bytes(&ua, &ue) {
                            Ok(dt) => {
                                ok_targets += 1;
                                println!(
                                    "  Parsed: {} rows, {} properties, leftover={}, size checks {}",
                                    dt.rows.len(),
                                    dt.count_props(),
                                    dt.leftover,
                                    if dt.size_mismatches.is_empty() {
                                        "OK"
                                    } else {
                                        "MISMATCH"
                                    }
                                );
                                for cp in patches.iter() {
                                    let nodes = modman_core::resolver::resolve(&dt, &cp.fragments);
                                    let spans: Vec<String> = nodes
                                        .iter()
                                        .take(4)
                                        .map(|n| {
                                            n.value_span()
                                                .map(|(a, b)| format!("{a}..{b}"))
                                                .unwrap_or_else(|| "n/a".to_string())
                                        })
                                        .collect();
                                    println!(
                                        "    [{}] -> {} target(s){}",
                                        cp.operation.label(),
                                        nodes.len(),
                                        if spans.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" @ {}", spans.join(", "))
                                        }
                                    );
                                }
                                println!(
                                    "  Patch application pending (length-changing + merge milestones); no output written."
                                );
                            }
                            Err(e) => eprintln!("  Walk error: {}", e),
                        }
                    }

                    println!(
                        "\nEngine preview complete: {}/{} target asset(s) parsed. Patch application + \
                         merge are the next milestone — no mod output was written.",
                        ok_targets,
                        plan.file_patches.len()
                    );
                }
                None => {
                    eprintln!(
                        "Error: Could not detect Project Wingman installation.\n\
                         Specify --install-path or set PW_INSTALL environment variable."
                    );
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Patch { input }) => {
            if let Err(e) = cmd_patch(&input) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Scan { path }) => {
            let paks_dir = resolve_paks_dir(&path);
            println!("Scanning: {}", paks_dir.display());
            let report = modman_core::discovery::scan_paks_dir(&paks_dir);
            for c in &report.components {
                let kind = match c.kind {
                    modman_core::discovery::ComponentKind::Preset => "preset",
                    modman_core::discovery::ComponentKind::BuildRequest => "request",
                };
                let pak_name = c
                    .pak_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                println!("- {pak_name} [{kind}] {}", c.record_path);
                for m in &c.mods {
                    print_mod_summary(m, "    ");
                }
            }
            println!(
                "\n{} pak(s) scanned, {} component(s), {} mod(s), {} error(s)",
                report.paks_scanned,
                report.components.len(),
                report.mod_count(),
                report.errors.len()
            );
            for (p, e) in &report.errors {
                eprintln!("  error {}: {}", p.display(), e);
            }
        }
        None => {
            println!(
                "Project Modman v{} — a Project Wingman modding utility",
                env!("CARGO_PKG_VERSION")
            );
        }
    }
}

/// Load all mods from a file or directory (kind-aware, lenient).
fn load_mods_from_path(
    path: &std::path::Path,
) -> Result<Vec<modman_core::manifest::WingmanMod>, String> {
    let mut out = Vec::new();
    if path.is_dir() {
        let entries = std::fs::read_dir(path).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "dtm" | "dtp" | "json") {
                        match load_mods_from_file(&p) {
                            Ok(mods) => out.extend(mods),
                            Err(e) => eprintln!("  Parse error {}: {}", p.display(), e),
                        }
                    }
                }
            }
        }
    } else {
        out.extend(load_mods_from_file(path)?);
    }
    Ok(out)
}

fn load_mods_from_file(
    path: &std::path::Path,
) -> Result<Vec<modman_core::manifest::WingmanMod>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut v: serde_json::Value =
        serde_json::from_str(&modman_core::manifest::sanitize_json(&raw))
            .map_err(|e| e.to_string())?;
    modman_core::manifest::normalize_json_keys(&mut v);
    let obj = v.as_object().ok_or("not a JSON object")?;
    if obj.contains_key("request") {
        Ok(modman_core::manifest::parse_meta_request_json(&raw)
            .map_err(|e| e.to_string())?
            .request
            .mods)
    } else if obj.contains_key("mods") {
        Ok(modman_core::manifest::parse_preset_json(&raw)
            .map_err(|e| e.to_string())?
            .mods)
    } else {
        Ok(vec![
            modman_core::manifest::parse_mod_json(&raw).map_err(|e| e.to_string())?
        ])
    }
}

fn print_mod_summary(m: &modman_core::manifest::WingmanMod, indent: &str) {
    let flags = if m.sicario.overwrites {
        " [overwrites]"
    } else {
        ""
    };
    println!("{indent}- {}{}", m.label(), flags);
    if let Some(meta) = &m.meta {
        if !meta.author.is_empty() {
            println!("{indent}    author: {}", meta.author);
        }
    }
    for (file, sets) in &m.asset_patches {
        let n: usize = sets.iter().map(|s| s.patches.len()).sum();
        println!("{indent}    asset: {file} ({n} patch(es))");
        for s in sets {
            for p in &s.patches {
                println!("{indent}      - {:<18} {}", p.patch_type, p.description);
            }
        }
    }
    for (file, sets) in &m.file_patches {
        let n: usize = sets.iter().map(|s| s.patches.len()).sum();
        println!("{indent}    file(hex): {file} ({n} patch(es))");
        for s in sets {
            for p in &s.patches {
                let t = p.patch_type.as_deref().unwrap_or("before");
                println!("{indent}      - {:<18} {}", t, p.description);
            }
        }
    }
    if !m.inputs.is_empty() {
        let ids: Vec<&str> = m.inputs.iter().map(|i| i.id.as_str()).collect();
        println!("{indent}    inputs: {}", ids.join(", "));
    }
    if !m.sicario.enable_steps.is_empty() {
        let keys: Vec<&str> = m.sicario.enable_steps.keys().map(|s| s.as_str()).collect();
        println!("{indent}    enableSteps: {}", keys.join(", "));
    }
}

fn cmd_patch(path: &str) -> Result<(), String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut v: serde_json::Value =
        serde_json::from_str(&modman_core::manifest::sanitize_json(&raw))
            .map_err(|e| e.to_string())?;
    modman_core::manifest::normalize_json_keys(&mut v);
    let obj = v.as_object().ok_or("not a JSON object")?;
    if obj.contains_key("request") {
        let req =
            modman_core::manifest::parse_meta_request_json(&raw).map_err(|e| e.to_string())?;
        println!("Format:      embedded build request (_meta/sicario)");
        println!(
            "App:         {} {} ({})",
            req.app.name, req.app.version, req.app.owner
        );
        println!("Request id:  {}", req.request.id);
        println!("Mods:        {}", req.request.mods.len());
        for m in &req.request.mods {
            print_mod_summary(m, "  ");
        }
    } else if obj.contains_key("mods") {
        let p = modman_core::manifest::parse_preset_json(&raw).map_err(|e| e.to_string())?;
        println!("Format:      preset (.dtp)");
        println!("Version:     {}", p.version);
        println!(
            "Engine:      {}",
            p.engine_version.as_deref().unwrap_or("-")
        );
        if !p.mod_parameters.is_empty() {
            println!("Parameters:  {}", p.mod_parameters.len());
            for (k, val) in &p.mod_parameters {
                println!("  {k} = {val}");
            }
        }
        println!("Mods:        {}", p.mods.len());
        for m in &p.mods {
            print_mod_summary(m, "  ");
        }
    } else {
        let m = modman_core::manifest::parse_mod_json(&raw).map_err(|e| e.to_string())?;
        println!("Format:      mod (.dtm)");
        print_mod_summary(&m, "");
    }
    Ok(())
}

/// Resolve a user-supplied path to the Paks directory: accepts either the
/// game install dir or the Paks dir itself.
fn resolve_paks_dir(path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    let nested = p.join("ProjectWingman/Content/Paks");
    if nested.is_dir() {
        nested
    } else {
        p.to_path_buf()
    }
}
