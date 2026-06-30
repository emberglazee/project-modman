use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "modman", version, about = "Project Modman — Project Wingman Modding Utility")]
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
    },
    /// Unpack a .pak file to a directory
    Unpack {
        /// Path to the .pak file
        input: String,
        /// Output directory
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Pack a directory into a .pak file
    Pack {
        /// Input directory
        input: String,
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
        Some(Commands::Info { input: _ }) => {
            eprintln!("info: not yet implemented — coming in 0.2.0");
        }
        Some(Commands::List { input: _ }) => {
            eprintln!("list: not yet implemented — coming in 0.2.0");
        }
        Some(Commands::Unpack { input: _, output: _ }) => {
            eprintln!("unpack: not yet implemented — coming in 0.3.0");
        }
        Some(Commands::Pack { input: _ }) => {
            eprintln!("pack: not yet implemented — coming in 0.4.0");
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
