use clap::{Parser, Subcommand};

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
            preset_paths: _,
            install_path: _,
        }) => {
            eprintln!("build: not yet implemented — coming in 0.11.0");
        }
        None => {
            println!(
                "Project Modman v{} — a Project Wingman modding utility",
                env!("CARGO_PKG_VERSION")
            );
        }
    }
}
