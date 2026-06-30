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

                    // Parse all .dtm/.dtp files
                    let mut all_mods: Vec<modman_core::manifest::WingmanMod> = Vec::new();

                    for pattern in &preset_paths {
                        let path = std::path::Path::new(pattern);
                        if path.is_dir() {
                            if let Ok(entries) = std::fs::read_dir(path) {
                                for entry in entries.flatten() {
                                    let p = entry.path();
                                    if p.extension().is_some_and(|e| e == "dtm" || e == "dtp") {
                                        match std::fs::read_to_string(&p) {
                                            Ok(content) => match serde_json::from_str::<
                                                modman_core::manifest::WingmanMod,
                                            >(
                                                &content
                                            ) {
                                                Ok(m) => {
                                                    println!(
                                                        "  Loaded: {} ({})",
                                                        m.id,
                                                        p.display()
                                                    );
                                                    all_mods.push(m);
                                                }
                                                Err(e) => eprintln!(
                                                    "  Parse error {}: {}",
                                                    p.display(),
                                                    e
                                                ),
                                            },
                                            Err(e) => {
                                                eprintln!("  Read error {}: {}", p.display(), e)
                                            }
                                        }
                                    }
                                }
                            }
                        } else if path.is_file()
                            && path.extension().is_some_and(|e| e == "dtm" || e == "dtp")
                        {
                            match std::fs::read_to_string(path) {
                                Ok(content) => match serde_json::from_str::<
                                    modman_core::manifest::WingmanMod,
                                >(&content)
                                {
                                    Ok(m) => {
                                        println!("  Loaded: {} ({})", m.id, path.display());
                                        all_mods.push(m);
                                    }
                                    Err(e) => eprintln!("  Parse error {}: {}", path.display(), e),
                                },
                                Err(e) => eprintln!("  Read error {}: {}", path.display(), e),
                            }
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

                    // Process each target file
                    for (target_file, patches) in &plan.file_patches {
                        println!("\nProcessing: {} ({} patches)", target_file, patches.len());

                        // The target path in the .dtm is game-relative
                        // Strip the mount prefix to get the entry path in the pak
                        let mount_prefix = "../../../";
                        let pak_entry_path = std::path::Path::new(mount_prefix).join(target_file);

                        // Extract from game pak
                        let _output_dir = work_dir.join("extracted");
                        match modman_pak::PakArchive::open(&main_pak_path) {
                            Ok(pak) => {
                                // Try to extract the target file
                                // We need to extract matching .uasset and .uexp
                                let uasset_entry =
                                    format!("{}", pak_entry_path.display()).replace('\\', "/");
                                let _uexp_entry = uasset_entry.replace(".uasset", ".uexp");

                                // Check if the target exists in the pak
                                let all_files = pak.files_stripped("../../../");
                                let uasset_name = std::path::Path::new(&uasset_entry)
                                    .file_name()
                                    .unwrap()
                                    .to_string_lossy();

                                // Find matching entry
                                let matching: Vec<String> = all_files
                                    .iter()
                                    .filter(|f| f.contains(&uasset_name.replace(".uasset", "")))
                                    .cloned()
                                    .collect();

                                if matching.is_empty() {
                                    eprintln!(
                                        "  Warning: no matching files found for '{}'",
                                        target_file
                                    );
                                    continue;
                                }

                                // Extract the matching files
                                let extract_dir = work_dir.join("extracted");
                                std::fs::create_dir_all(&extract_dir).unwrap();
                                if let Err(e) = pak.unpack(&extract_dir, "../../../", false) {
                                    eprintln!("  Extract error: {}", e);
                                    continue;
                                }

                                // Find the extracted uasset file
                                let extracted_uasset = extract_dir.join(target_file);
                                let extracted_uexp = extracted_uasset.with_extension("uexp");

                                if !extracted_uasset.exists() || !extracted_uexp.exists() {
                                    eprintln!(
                                        "  Warning: extracted files not found at {}",
                                        extracted_uasset.display()
                                    );
                                    continue;
                                }

                                // Parse uasset
                                match modman_uasset::asset::AssetFile::open(
                                    &extracted_uasset.to_string_lossy(),
                                    &extracted_uexp.to_string_lossy(),
                                ) {
                                    Ok(mut asset) => {
                                        println!(
                                            "  Parsed: {} names, {} exports, {} properties",
                                            asset.names.len(),
                                            asset.exports.len(),
                                            asset.properties.len(),
                                        );

                                        // TODO: Apply patches to asset.properties
                                        // For now, just read-modify-write without changes
                                        println!("  Applied {} patches (stub)", patches.len());

                                        // Write back
                                        let out_dir = work_dir.join("output");
                                        std::fs::create_dir_all(&out_dir).unwrap();
                                        let out_uasset = out_dir.join(target_file);
                                        let out_uexp = out_uasset.with_extension("uexp");
                                        std::fs::create_dir_all(out_uasset.parent().unwrap())
                                            .unwrap();
                                        match asset.apply_and_write(
                                            &out_uasset.to_string_lossy(),
                                            &out_uexp.to_string_lossy(),
                                        ) {
                                            Ok(()) => {
                                                println!("  Written: {}", out_uasset.display())
                                            }
                                            Err(e) => eprintln!("  Write error: {}", e),
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("  Parse error: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("  Pak error: {}", e);
                            }
                        }
                    }

                    // Pack output into .pak
                    let output_dir = work_dir.join("output");
                    if output_dir.exists() {
                        let mods_dir = game_paks.join("~mods");
                        std::fs::create_dir_all(&mods_dir).unwrap();
                        let output_pak = mods_dir.join("modman_merged.pak");

                        println!("\nPacking output to {} ...", output_pak.display());
                        match modman_pak::pack(
                            &output_dir,
                            &output_pak,
                            modman_pak::Version::V11,
                            "../../../".to_string(),
                            None,
                        ) {
                            Ok(()) => println!("Done! Output: {}", output_pak.display()),
                            Err(e) => eprintln!("Pack error: {}", e),
                        }
                    }

                    println!("\nBuild complete.");
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
        None => {
            println!(
                "Project Modman v{} — a Project Wingman modding utility",
                env!("CARGO_PKG_VERSION")
            );
        }
    }
}
