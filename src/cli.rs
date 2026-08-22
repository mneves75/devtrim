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

    /// Skip confirmation prompts for danger ≤ Medium (5). Non-TTY requires this or --yolo.
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Skip ALL safety gates including typed confirmations for Critical ops
    #[arg(long, global = true)]
    pub yolo: bool,

    /// Permanently delete instead of moving to Trash where applicable
    #[arg(long, global = true)]
    pub shred: bool,

    /// Emit machine-readable JSON (agent-friendly)
    #[arg(long, global = true)]
    pub json: bool,

    /// Extra paths to scan beyond configured roots
    #[arg(long = "root", global = true)]
    pub roots: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Read-only report of every reclaimable category with sizes and risk
    Scan,
    /// Clean one category (see subcommand help)
    Clean {
        /// caches | node-modules | simulators | xcode | docker | toolchains | leftovers
        #[arg(value_enum)]
        target: Target,
    },
    /// Show iCloud Drive queued uploads and their local-materialization status
    Icloud,
    /// Purge the macOS Trash. Requires --confirm=<gb> matching current Trash size.
    TrashEmpty {
        /// Type the approximate Trash size in GB as acknowledgment (e.g. --confirm=14)
        #[arg(long)]
        confirm_gb: Option<u64>,
    },
}

impl Target {
    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Caches => "caches",
            Target::NodeModules => "node-modules",
            Target::Simulators => "simulators",
            Target::Xcode => "xcode",
            Target::Docker => "docker",
            Target::Toolchains => "toolchains",
            Target::Leftovers => "leftovers",
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
