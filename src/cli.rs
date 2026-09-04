//! CLI definition. Preview-by-default: mutation requires `--apply`.

use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

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
    pub command: Option<Command>,

    /// Apply the exact previewed actions; cleanup can delete data
    #[arg(long, global = true)]
    pub apply: bool,

    /// Accept data-loss risk and skip y/N prompts; required for non-TTY mutation
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Accept data-loss risk and skip prompts; operation acknowledgments still apply
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
    /// Open the interactive terminal interface
    Tui,
    /// Read-only report of every reclaimable category with sizes and risk
    Scan,
    /// Read-only machine vitals: load, memory, disk, battery, thermals, processes
    Status,
    /// Locate every file belonging to an app by its exact bundle identifier
    Uninstall {
        /// Application name or bundle path (e.g. AltTab, "Visual Studio Code")
        app: String,
    },
    /// Explore disk usage interactively, largest first; never deletes anything
    Analyze {
        /// Directory to start from (default: your home directory)
        path: Option<String>,
    },
    /// Show the largest directories one and two levels below scan roots
    Largest {
        /// Number of entries to show (clamped 1..=100)
        #[arg(long)]
        top: Option<usize>,
    },
    /// Clean one category
    Clean {
        /// caches | node-modules | artifacts | simulators | xcode | docker | toolchains | installers | leftovers
        #[arg(value_enum)]
        target: Target,
    },
    /// Show recent apply results from the local write-ahead journal
    History {
        /// Maximum entries to show (clamped to 1..=1000)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Print a shell completion script
    Completions {
        #[arg(value_parser = completion_shell)]
        shell: Shell,
    },
    /// Print the devtrim man page in roff format
    Manpage,
    /// Show large iCloud Drive files and their locally allocated storage
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
            Self::Artifacts => "artifacts",
            Self::Simulators => "simulators",
            Self::Xcode => "xcode",
            Self::Docker => "docker",
            Self::Toolchains => "toolchains",
            Self::Installers => "installers",
            Self::Leftovers => "leftovers",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Caches,
    NodeModules,
    Artifacts,
    Simulators,
    Xcode,
    Docker,
    Toolchains,
    Installers,
    Leftovers,
}

fn completion_shell(value: &str) -> Result<Shell, String> {
    match value {
        "bash" => Ok(Shell::Bash),
        "zsh" => Ok(Shell::Zsh),
        "fish" => Ok(Shell::Fish),
        _ => Err("supported shells: bash, zsh, fish".into()),
    }
}
