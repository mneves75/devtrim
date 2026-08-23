//! CLI definition. Preview-by-default: mutation requires `--apply`.

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "devtrim",
    author = "Marcus Neves",
    version,
    about = "Developer-machine disk hygiene: measure, classify, trim — safely.",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Apply changes (default is dry-run preview only)
    #[arg(long, global = true)]
    pub apply: bool,

    /// Skip normal y/N prompts. Non-TTY mutation requires this or --yolo.
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Skip confirmation only; never adds operations to the previewed plan
    #[arg(long, global = true)]
    pub yolo: bool,

    /// Permanently delete instead of moving filesystem targets to Trash
    #[arg(long, global = true)]
    pub shred: bool,

    /// Emit one machine-readable JSON document
    #[arg(long, global = true)]
    pub json: bool,

    /// Replace configured/default scan roots
    #[arg(long = "root", global = true)]
    pub roots: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Read-only report of every reclaimable category with sizes and risk
    Scan,
    /// Clean one category
    Clean {
        /// caches | node-modules | simulators | xcode | docker | toolchains | leftovers
        #[arg(value_enum)]
        target: Target,
    },
    /// Show iCloud Drive queued uploads and their local-materialization status
    Icloud,
    /// Purge macOS Trash permanently; requires --apply and --confirm=<gb>
    TrashEmpty {
        /// Approximate Trash size in GB as acknowledgment (e.g. --confirm=14)
        #[arg(long = "confirm")]
        confirm_gb: Option<u64>,
    },
}

impl Target {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Caches => "caches",
            Self::NodeModules => "node-modules",
            Self::Simulators => "simulators",
            Self::Xcode => "xcode",
            Self::Docker => "docker",
            Self::Toolchains => "toolchains",
            Self::Leftovers => "leftovers",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Target {
    Caches,
    NodeModules,
    Simulators,
    Xcode,
    Docker,
    Toolchains,
    Leftovers,
}
