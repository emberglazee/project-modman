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
        /// PAK version (V8B or V11, default: V11)
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
    },
    /// Inspect a Sicario patch file (.dtm, .dtp, or embedded _meta build request)
    Patch {
        /// Path to the patch file
        input: String,
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
                "V8B" => modman_pak::Version::V8B,
                "V11" => modman_pak::Version::V11,
                _ => {
                    eprintln!("Error: unsupported version '{}'. Use V8B or V11.", version);
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
                        let entry = all_files
                            .iter()
                            .find(|f| f.as_str() == needle)
                            .or_else(|| all_files.iter().find(|f| f.ends_with(&needle)));

                        let entry = match entry {
                            Some(e) => e.clone(),
                            None => {
                                eprintln!("  Warning: no matching pak entry for '{}'", target_file);
                                continue;
                            }
                        };

                        let stem = std::path::Path::new(&entry)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "asset".to_string());
                        let local_uasset = extract_dir.join(format!("{stem}.uasset"));
                        let local_uexp = extract_dir.join(format!("{stem}.uexp"));

                        if let Err(e) = pak.extract_entry(&entry, &local_uasset) {
                            eprintln!("  Extract error ({}): {}", entry, e);
                            continue;
                        }
                        let uexp_entry = entry.replace(".uasset", ".uexp");
                        if let Err(e) = pak.extract_entry(&uexp_entry, &local_uexp) {
                            eprintln!("  Note: no .uexp companion for '{}' ({})", entry, e);
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
                                println!("  Patch application pending (A-layer milestone); no output written.");
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
