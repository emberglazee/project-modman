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
        /// Write a merge report (JSON) to this file (relative paths go next to the output)
        #[arg(long)]
        report: Option<String>,
        /// Do NOT embed detected skin-pak files (Assets/Skins) into the merged
        /// pak; with this flag the output matches the C# merger (skin paks must
        /// stay installed for the textures to resolve)
        #[arg(long)]
        no_embed_skins: bool,
    },
    /// Pack preset files into standalone merged mods (preset embedded at
    /// Content/sicario, like the C# `preset-pack` command)
    PresetPack {
        /// Paths to preset files (.dtp) or directories
        preset_paths: Vec<String>,
        /// Output file name root (default: SicarioPresetMerge)
        #[arg(short = 'n', long)]
        name: Option<String>,
        /// Path to the game install directory (auto-detected if omitted)
        #[arg(long)]
        install_path: Option<String>,
        /// Output directory (default: current directory)
        #[arg(long)]
        output: Option<String>,
    },
    /// Combine conflicting override mods (no Sicario metadata needed): merges
    /// datatable overrides structurally (later mods win conflicts) into one pak
    Combine {
        /// Pak files (or directories of paks) to combine, in load-priority order
        paks: Vec<String>,
        /// Path to the game install directory (auto-detected if omitted)
        #[arg(long)]
        install_path: Option<String>,
        /// Output directory for the combined pak (default: current directory)
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
            report,
            no_embed_skins,
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

                    use modman_core::components as comps;

                    // --- Assemble merge components (C# provider parity) ---
                    // Loose presets: *.dtp from given dirs + the install's
                    // default search paths; direct file arguments load as-is.
                    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();
                    let mut direct_mods: Vec<modman_core::manifest::WingmanMod> = Vec::new();
                    for pattern in &preset_paths {
                        let pb = std::path::PathBuf::from(pattern);
                        if pb.is_dir() {
                            search_dirs.push(pb);
                        } else {
                            match load_mods_from_file(&pb) {
                                Ok(mods) => direct_mods.extend(mods),
                                Err(e) => eprintln!("  Load error {}: {}", pattern, e),
                            }
                        }
                    }
                    search_dirs.push(
                        std::path::PathBuf::from(&path).join("ProjectWingman/Content/Presets"),
                    );
                    search_dirs.push(game_paks.join("~mods"));
                    search_dirs.push(game_paks.join("~presets"));
                    let loose_files = comps::collect_dtp_files(&search_dirs);
                    let (mut loose_comp, warnings) =
                        comps::loose_component(&loose_files, comps::ENGINE_VERSION);
                    for w in &warnings {
                        eprintln!("  Warning: {w}");
                    }
                    for m in direct_mods {
                        println!("  Loaded: {} (direct)", m.label());
                        loose_comp.mods.push(m);
                    }

                    // Embedded components from installed paks.
                    let scan = modman_core::discovery::scan_paks_dir(&game_paks);
                    let (mut embedded, warnings) =
                        comps::embedded_components(&scan, comps::ENGINE_VERSION);
                    for w in &warnings {
                        eprintln!("  Warning: {w}");
                    }

                    // Provider order (also the report order):
                    // embeddedPresets, sicarioRequests, loosePresets, customSkins.
                    let mut components: Vec<comps::MergeComponent> = Vec::new();
                    components.append(&mut embedded);
                    components.push(loose_comp);
                    let (skin_comp, _skin_warnings) = comps::skin_component(&game_paks);
                    if let Some(sc) = skin_comp {
                        components.push(sc);
                    }
                    for c in &components {
                        println!("  {}", c.message);
                    }

                    let inputs = comps::merged_params(&components);
                    println!("Final mod will be built with {} parameters", inputs.len());

                    let mut all_mods = comps::take_ordered_mods(&mut components);
                    if all_mods.is_empty() {
                        eprintln!("No mods or presets found to build!");
                        std::process::exit(1);
                    }
                    println!("Queuing mod build with {} mods", all_mods.len());

                    // Apply the template pipeline (vars + enableSteps + patch render).
                    for m in &mut all_mods {
                        modman_core::template::apply_variables_to_mod(m, &inputs);
                    }

                    if let Some(out_dir) = output.as_ref() {
                        // === WRITE MODE: merge + emit a real mod pak ===
                        let out_dir = std::path::PathBuf::from(out_dir);
                        let mod_refs: Vec<&modman_core::manifest::WingmanMod> =
                            all_mods.iter().collect();
                        // Self-contained mode (default): embed detected skin
                        // files so the merged pak works without the original
                        // skin paks installed (--no-embed-skins restores the
                        // strict C# behavior).
                        let extra_files: Vec<(String, Vec<u8>)> = if no_embed_skins {
                            Vec::new()
                        } else {
                            let files = comps::collect_skin_files(&game_paks);
                            if !files.is_empty() {
                                println!(
                                    "  Embedding {} skin file(s) from installed skin paks",
                                    files.len()
                                );
                            }
                            files
                        };
                        match build_pak_from_mods(
                            &game_paks,
                            &mod_refs,
                            &extra_files,
                            &out_dir,
                            "SicarioMerge_P.pak",
                            true,
                        ) {
                            Ok((ok, total)) => println!(
                                "\nWrote {} ({} asset target(s), {} file(s) total)",
                                out_dir.join("SicarioMerge_P.pak").display(),
                                ok,
                                total
                            ),
                            Err(e) => {
                                eprintln!("{e}");
                                std::process::exit(1);
                            }
                        }

                        if let Some(report_name) = report.as_ref() {
                            let report_path = {
                                let p = std::path::PathBuf::from(report_name);
                                if p.is_absolute() {
                                    p
                                } else {
                                    out_dir.join(p)
                                }
                            };
                            match write_report(&report_path, &inputs, &components) {
                                Ok(()) => {
                                    println!("Wrote merge report to '{}'.", report_path.display())
                                }
                                Err(e) => eprintln!("  Warning: error writing report file: {e}"),
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
        Some(Commands::PresetPack {
            preset_paths,
            name,
            install_path,
            output,
        }) => {
            let game_path = install_path.clone().or_else(|| {
                crate::game::detect_game().map(|g| g.path.to_string_lossy().to_string())
            });
            let Some(path) = game_path else {
                eprintln!("Error: Could not detect Project Wingman installation.");
                std::process::exit(1);
            };
            let game_paks = std::path::Path::new(&path).join("ProjectWingman/Content/Paks");
            println!("Game: {path}");
            println!("Paks: {}", game_paks.display());

            // Load preset files (dirs expand to *.dtp, like the C# loader).
            let mut presets: Vec<(std::path::PathBuf, modman_core::manifest::WingmanPreset)> =
                Vec::new();
            for p in &preset_paths {
                let pb = std::path::PathBuf::from(p);
                let files: Vec<std::path::PathBuf> = if pb.is_dir() {
                    modman_core::components::collect_dtp_files(std::slice::from_ref(&pb))
                } else {
                    vec![pb.clone()]
                };
                for f in files {
                    let parsed = std::fs::read_to_string(&f)
                        .map_err(|e| e.to_string())
                        .and_then(|t| {
                            modman_core::manifest::parse_preset_json(&t).map_err(|e| e.to_string())
                        });
                    match parsed {
                        Ok(preset) if !preset.mods.is_empty() => presets.push((f, preset)),
                        Ok(_) => {}
                        Err(e) => eprintln!("  Load error {}: {e}", f.display()),
                    }
                }
            }
            presets.retain(|(f, preset)| {
                let ok = modman_core::components::preset_supported(
                    preset.engine_version.as_deref(),
                    modman_core::components::ENGINE_VERSION,
                );
                if !ok {
                    eprintln!(
                        "  Warning: Incompatible embed! This preset is not supported by the current engine version and will not be loaded: {}",
                        f.display()
                    );
                }
                ok
            });
            if presets.is_empty() {
                eprintln!("No presets found to pack!");
                std::process::exit(1);
            }

            let name_root = name.unwrap_or_else(|| "SicarioPresetMerge".to_string());
            let multi = presets.len() > 1;
            let out_root = output
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            println!("Queuing mod build for {} presets", presets.len());

            for (idx, (file, preset)) in presets.iter().enumerate() {
                let base = if multi {
                    format!("{name_root}_{idx}")
                } else {
                    name_root.clone()
                };
                let mut mods = preset.mods.clone();
                let inputs = preset.mod_parameters.clone();
                for m in &mut mods {
                    modman_core::template::apply_variables_to_mod(m, &inputs);
                }
                let mod_refs: Vec<&modman_core::manifest::WingmanMod> = mods.iter().collect();
                // The preset file itself rides along at Content/sicario
                // (C# `AdditionalFiles`), making the output re-mergeable.
                let file_name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let extra = match std::fs::read(file) {
                    Ok(bytes) => {
                        vec![(format!("ProjectWingman/Content/sicario/{file_name}"), bytes)]
                    }
                    Err(e) => {
                        eprintln!("  Read error {}: {e}", file.display());
                        continue;
                    }
                };
                let pak_name = format!("{base}_P.pak");
                match build_pak_from_mods(
                    &game_paks, &mod_refs, &extra, &out_root, &pak_name, false,
                ) {
                    Ok(_) => println!(
                        "'{}' built to '{}'",
                        file.file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        out_root.join(&pak_name).display()
                    ),
                    Err(e) => eprintln!("  Error building {}: {e}", file.display()),
                }
            }
        }
        Some(Commands::Combine {
            paks,
            install_path,
            output,
        }) => {
            let game_path = install_path.clone().or_else(|| {
                crate::game::detect_game().map(|g| g.path.to_string_lossy().to_string())
            });
            let Some(path) = game_path else {
                eprintln!("Error: Could not detect Project Wingman installation.");
                std::process::exit(1);
            };
            let game_paks = std::path::Path::new(&path).join("ProjectWingman/Content/Paks");
            let out_dir = output
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            println!("Game: {path}");
            if let Err(e) = cmd_combine(&game_paks, &paks, &out_dir) {
                eprintln!("Error: {e}");
                std::process::exit(1);
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

/// Combine conflicting override mods: three-way datatable merges (vanilla +
/// each mod's delta, later mods win), single-winner pass-through for
/// non-datatable files, warnings on irreconcilable conflicts.
fn cmd_combine(
    game_paks: &std::path::Path,
    pak_args: &[String],
    out_dir: &std::path::Path,
) -> Result<(), String> {
    // Expand args (files or directories, recursive).
    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("pak"))
            {
                out.push(p);
            }
        }
    }
    let mut paks: Vec<std::path::PathBuf> = Vec::new();
    for a in pak_args {
        let p = std::path::PathBuf::from(a);
        if p.is_dir() {
            collect(&p, &mut paks);
        } else {
            paks.push(p);
        }
    }
    paks.sort();
    if paks.is_empty() {
        return Err("no paks given".to_string());
    }

    let base = modman_pak::PakArchive::open(game_paks.join("pakchunk0-WindowsNoEditor.pak"))
        .map_err(|e| format!("Pak error: {e}"))?;
    let base_files = base.files();
    let base_find = |name: &str| -> Option<String> {
        base_files
            .iter()
            .find(|f| f.as_str() == name)
            .or_else(|| base_files.iter().find(|f| f.ends_with(name)))
            .cloned()
    };

    struct Override {
        ua: Vec<u8>,
        ue: Vec<u8>,
    }
    let mut dt_overrides: std::collections::BTreeMap<String, Vec<Override>> =
        std::collections::BTreeMap::new();
    let mut passthrough: std::collections::BTreeMap<String, (usize, Vec<u8>)> =
        std::collections::BTreeMap::new();
    let mut conflicts: Vec<String> = Vec::new();

    for (mi, pak_path) in paks.iter().enumerate() {
        println!("Reading: {}", pak_path.display());
        let archive = modman_pak::PakArchive::open(pak_path)
            .map_err(|e| format!("{}: {e}", pak_path.display()))?;
        let records = archive.files();
        let norm = |r: &String| r.replace('\\', "/");
        let mut seen = std::collections::BTreeSet::new();
        for r in &records {
            let n = norm(r);
            if n.to_ascii_lowercase().ends_with(".uasset") {
                let uexp_name = format!("{}.uexp", &n[..n.len() - 7]);
                if let Some(uexp_rec) = records
                    .iter()
                    .find(|x| norm(x).eq_ignore_ascii_case(&uexp_name))
                {
                    if !seen.insert(n.clone()) {
                        continue;
                    }
                    // The .uexp record belongs to this datatable pair — mark
                    // it consumed so it never falls through to the
                    // pass-through (which would clobber the merged output).
                    seen.insert(norm(uexp_rec));
                    let Some(vkey) = base_find(&n) else {
                        // Not a game file: single-winner pass-through pair.
                        let ua = archive.read_entry(r).map_err(|e| e.to_string())?;
                        let ue = archive.read_entry(uexp_rec).map_err(|e| e.to_string())?;
                        for (k, b) in [(n.clone(), ua), (norm(uexp_rec), ue)] {
                            if let Some((prev, _)) = passthrough.get(&k) {
                                if *prev != mi {
                                    conflicts.push(k.clone());
                                }
                            }
                            passthrough.insert(k, (mi, b));
                        }
                        continue;
                    };
                    let vkey_uexp = format!("{}.uexp", &vkey[..vkey.len() - 7]);
                    let Some(vue_key) = base_find(&vkey_uexp) else {
                        continue;
                    };
                    let ua = archive.read_entry(r).map_err(|e| e.to_string())?;
                    let ue = archive.read_entry(uexp_rec).map_err(|e| e.to_string())?;
                    let vua = base.read_entry(&vkey).map_err(|e| e.to_string())?;
                    let vue = base.read_entry(&vue_key).map_err(|e| e.to_string())?;
                    if ua == vua && ue == vue {
                        continue; // not actually an override
                    }
                    dt_overrides
                        .entry(n.clone())
                        .or_default()
                        .push(Override { ua, ue });
                    continue;
                }
            }
            if seen.insert(n.clone()) {
                let bytes = archive.read_entry(r).map_err(|e| e.to_string())?;
                if let Some((prev, _)) = passthrough.get(&n) {
                    if *prev != mi {
                        conflicts.push(n.clone());
                    }
                }
                passthrough.insert(n.clone(), (mi, bytes));
            }
        }
    }

    let mut files: modman_core::merge::FileMap = modman_core::merge::FileMap::new();
    let mut merged = 0usize;
    for (target, ovs) in &dt_overrides {
        let Some(vkey) = base_find(target) else {
            continue;
        };
        let vkey_uexp = format!("{}.uexp", &vkey[..vkey.len() - 7]);
        let Some(vue_key) = base_find(&vkey_uexp) else {
            continue;
        };
        let vua = base.read_entry(&vkey).map_err(|e| e.to_string())?;
        let vue = base.read_entry(&vue_key).map_err(|e| e.to_string())?;
        let refs: Vec<(&[u8], &[u8])> = ovs
            .iter()
            .map(|o| (o.ua.as_slice(), o.ue.as_slice()))
            .collect();
        match modman_core::combine::merge_datatable_overrides((&vua, &vue), &refs) {
            Ok(Some(c)) => {
                for w in &c.warnings {
                    eprintln!("  Warning [{target}]: {w}");
                }
                println!(
                    "  Merged {} override(s) into {}",
                    ovs.len(),
                    target.split('/').next_back().unwrap_or(target)
                );
                files.insert(vkey, c.uasset);
                files.insert(vue_key, c.uexp);
                merged += 1;
            }
            Ok(None) => {}
            Err(e) => eprintln!("  Warning: could not merge {target}: {e}"),
        }
    }
    for (path, (_, bytes)) in &passthrough {
        files.insert(path.clone(), bytes.clone());
    }
    for c in &conflicts {
        eprintln!(
            "  Warning: '{c}' is overridden by multiple mods — only the last version is kept \
             (this file type cannot be combined)"
        );
    }

    if files.is_empty() {
        return Err("nothing to combine (no conflicting overrides found)".to_string());
    }
    let staging = out_dir.join("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    for (key, bytes) in &files {
        let out_path = staging.join(key);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&out_path, bytes).map_err(|e| e.to_string())?;
    }
    let pak_out = out_dir.join("SicarioCombine_P.pak");
    modman_pak::pack(
        &staging,
        &pak_out,
        modman_pak::Version::V3,
        "../../../".to_string(),
        None,
    )
    .map_err(|e| format!("Pack error: {e}"))?;
    println!(
        "\nWrote {} ({} datatable(s) merged, {} file(s) total, {} pass-through conflict warning(s))",
        pak_out.display(),
        merged,
        files.len(),
        conflicts.len()
    );
    Ok(())
}

/// Build a merged mod pak: load every target (plus `.uexp`/`.uasset`
/// sidecars) from the game pak into a virtual file map, run the hex phase
/// (all mods) then the asset phase (all mods), add any extra files, stage,
/// and pack (V3, `../../../` mount). Returns (asset targets, total files).
fn build_pak_from_mods(
    game_paks: &std::path::Path,
    mods: &[&modman_core::manifest::WingmanMod],
    extra_files: &[(String, Vec<u8>)],
    out_dir: &std::path::Path,
    pak_name: &str,
    verbose: bool,
) -> Result<(usize, usize), String> {
    let staging = out_dir.join("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    // Targets = union of every mod's asset + hex patch targets.
    let mut targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut asset_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for m in mods {
        targets.extend(m.asset_patches.keys().cloned());
        targets.extend(m.file_patches.keys().cloned());
        asset_targets.extend(m.asset_patches.keys().cloned());
    }

    let main_pak_path = game_paks.join("pakchunk0-WindowsNoEditor.pak");
    let pak =
        modman_pak::PakArchive::open(&main_pak_path).map_err(|e| format!("Pak error: {e}"))?;
    let all_files = pak.files();
    let extract_dir = std::env::temp_dir().join("modman-build");
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    // Load every target (plus .uexp/.uasset sidecars) into a virtual file map,
    // like the C# build context.
    let mut files: modman_core::merge::FileMap = modman_core::merge::FileMap::new();
    let find_entry = |name: &str| -> Option<String> {
        all_files
            .iter()
            .find(|f| f.as_str() == name)
            .or_else(|| all_files.iter().find(|f| f.ends_with(name)))
            .cloned()
    };
    for target in &targets {
        let needle = target.trim_start_matches("../../../").replace('\\', "/");
        let mut wanted = vec![needle.clone()];
        if needle.ends_with(".uexp") {
            wanted.push(needle.replace(".uexp", ".uasset"));
        } else if needle.ends_with(".uasset") {
            wanted.push(needle.replace(".uasset", ".uexp"));
        }
        for w in wanted {
            let Some(entry) = find_entry(&w) else {
                continue;
            };
            if files.contains_key(&entry) {
                continue;
            }
            let stem = std::path::Path::new(&entry)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "asset".to_string());
            let ext = std::path::Path::new(&entry)
                .extension()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let local = extract_dir.join(format!("{stem}.{ext}"));
            if let Err(e) = pak.extract_entry(&entry, &local) {
                eprintln!("  Extract error ({entry}): {e}");
                continue;
            }
            files.insert(entry, std::fs::read(&local).map_err(|e| e.to_string())?);
        }
    }

    // Phase 1 (engine-major): hex patches for all mods, including the .uexp
    // length auto-correct.
    modman_core::merge::apply_hex_phase(&mut files, mods)
        .map_err(|e| format!("Hex patch error: {e}"))?;

    // Phase 2: DataTable asset patches for all mods.
    let mut ok = 0usize;
    for target in &asset_targets {
        if verbose {
            println!("Merging: {target}");
        }
        let needle = target.trim_start_matches("../../../").replace('\\', "/");
        let uexp_needle = if needle.ends_with(".uexp") {
            needle.clone()
        } else if needle.ends_with(".uasset") {
            needle.replace(".uasset", ".uexp")
        } else {
            format!("{needle}.uexp")
        };
        let uasset_needle = uexp_needle.replace(".uexp", ".uasset");
        let (Some(uexp_key), Some(uasset_key)) = (
            modman_core::merge::resolve_target(&files, &uexp_needle).cloned(),
            modman_core::merge::resolve_target(&files, &uasset_needle).cloned(),
        ) else {
            eprintln!("  Warning: no pak entries for '{target}'; skipped");
            continue;
        };
        let uasset = files.get(&uasset_key).unwrap().clone();
        let uexp = files.get(&uexp_key).unwrap().clone();
        let merged = modman_core::merge::merge_mods(&uasset, &uexp, mods, target)
            .map_err(|e| format!("Merge error on {target}: {e}"))?;
        if verbose {
            println!(
                "  -> {} bytes uexp, {} bytes uasset",
                merged.uexp.len(),
                merged.uasset.len()
            );
        }
        files.insert(uasset_key, merged.uasset);
        files.insert(uexp_key, merged.uexp);
        ok += 1;
    }

    for (key, bytes) in extra_files {
        files.insert(key.clone(), bytes.clone());
    }

    // Write every file in the map into the staging dir.
    for (key, bytes) in &files {
        let out_path = staging.join(key);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&out_path, bytes).map_err(|e| e.to_string())?;
    }
    if files.is_empty() {
        return Err("No targets merged.".to_string());
    }

    let pak_out = out_dir.join(pak_name);
    modman_pak::pack(
        &staging,
        &pak_out,
        modman_pak::Version::V3,
        "../../../".to_string(),
        None,
    )
    .map_err(|e| format!("Pack error: {e}"))?;
    Ok((ok, files.len()))
}

/// Write the merge report (C# `JsonReportWriter` shape).
fn write_report(
    path: &std::path::Path,
    inputs: &modman_core::templating::Vars,
    components: &[modman_core::components::MergeComponent],
) -> Result<(), String> {
    fn indented(v: &impl serde::Serialize) -> Result<String, String> {
        let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
        Ok(s.lines()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    l.to_string()
                } else {
                    format!("  {l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
    let mut s = String::from("{\n  \"inputParameters\": ");
    s.push_str(&indented(inputs)?);
    for c in components {
        if !c.name.is_empty() && !c.resources.is_empty() {
            s.push_str(&format!(",\n  \"{}\": ", c.name));
            s.push_str(&indented(&c.resources)?);
        }
    }
    s.push_str("\n}");
    std::fs::write(path, s).map_err(|e| e.to_string())
}
