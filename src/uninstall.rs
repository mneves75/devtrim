//! Application files whose name IS the app's bundle identifier.
//!
//! The contract is narrow on purpose, and stating it exactly matters more than
//! sounding complete: this finds paths macOS keys by the identifier, and only
//! those. An app that stores data under a product name is invisible to it —
//! Visual Studio Code keeps `~/Library/Application Support/Code` and `~/.vscode`,
//! neither of which contains `com.microsoft.VSCode`. System-wide helpers under
//! `/Library` are outside the catalog too. This is a conservative report, not an
//! inventory, and the output says so.
//!
//! This command reports; it never deletes, and that is a structural fact rather
//! than a policy choice. `safety::is_protected` refuses `/Applications` and
//! everything beneath it, and refuses everything under `~/Library` outside a
//! closed allowlist of developer-managed paths — a boundary the repository's own
//! property tests assert
//! (`is_protected(home/"Library/Application Support")`). Deleting an app bundle
//! and its support files would require widening that list for every code path,
//! not just this one, so the useful half devtrim can honestly do is the half a
//! person cannot do by hand: resolve the identifier and find what is keyed to it.
//!
//! Matching is by EXACT bundle identifier, never by name. `com.example.thing`
//! must not select `com.example.thingy`, and a display name like "Notes" must
//! not select every path containing the word. The identifier is read from the
//! bundle's own `Info.plist`, which is the only structural evidence macOS
//! actually keys its support directories on.
//!
//! Group containers are deliberately NOT reported. Their names come from the
//! `com.apple.security.application-groups` entitlement and are arbitrary: on a
//! real machine `2BUA8C4S2C.com.1password` belongs to an app whose identifier is
//! `com.1password.1password`, and 49 of 171 containers here begin with `group.`
//! and contain no app identifier at all. A filename-suffix rule would both miss
//! real containers and attribute another app's data to this one, and a wrong
//! entry in a list someone acts on is worse than an absent one.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};

use anyhow::{Context, Result, bail};
use colored::Colorize;

use crate::report::{self, Action, Finding};
use crate::safety::Ctx;

/// Directories whose immediate children are named for a bundle identifier.
const ID_DIRECTORIES: &[&str] = &[
    "Library/Application Support",
    "Library/Caches",
    "Library/Containers",
    "Library/HTTPStorages",
    "Library/WebKit",
];

/// Files named `<bundle identifier>.<extension>`.
const ID_FILES: &[(&str, &str)] = &[
    ("Library/Preferences", "plist"),
    ("Library/Saved Application State", "savedState"),
    ("Library/LaunchAgents", "plist"),
];

/// Where an app may be found. `/Applications` is system-wide; the per-user one
/// is where Homebrew casks and hand-installed apps land.
const APPLICATION_ROOTS: &[&str] = &["/Applications"];
const USER_APPLICATION_ROOT: &str = "Applications";

/// Closed authority for the one dynamic argument this module hands to a
/// process.
///
/// `CODING_STANDARDS.md` S12 permits a dynamic argument only when a closed
/// typed authority validates and carries it. Constructing this requires a path
/// already resolved to a real `.app` directory that is a direct child of an
/// approved Applications root, so `plutil` can never be aimed at an arbitrary
/// file.
struct BundlePlist(PathBuf);

impl BundlePlist {
    fn for_app(app: &Path, home: &Path) -> Result<Self> {
        if !is_approved_app_bundle(app, home)? {
            bail!("refusing to inspect a path outside the Applications directories");
        }
        let plist = app.join("Contents/Info.plist");
        let metadata = std::fs::symlink_metadata(&plist)
            .with_context(|| format!("cannot inspect {}", plist.display()))?;
        if !metadata.file_type().is_file() {
            bail!("{} is not a regular file", plist.display());
        }
        Ok(Self(plist))
    }

    fn bundle_identifier(&self) -> Result<String> {
        let output: Output = Command::new("plutil")
            .arg("-extract")
            .arg("CFBundleIdentifier")
            .arg("raw")
            .arg("-o")
            .arg("-")
            .arg(&self.0)
            .output()
            .with_context(|| format!("cannot run `plutil` on {}", self.0.display()))?;
        if !output.status.success() {
            bail!("{} declares no CFBundleIdentifier", self.0.display());
        }
        let identifier = String::from_utf8(output.stdout)
            .with_context(|| format!("{} returned non-UTF-8 output", self.0.display()))?
            .trim()
            .to_string();
        validate_bundle_identifier(&identifier)?;
        Ok(identifier)
    }
}

/// The identifier becomes a filename this module compares against, so it is
/// validated as one: no separators, no traversal, no surprises.
pub(crate) fn validate_bundle_identifier(identifier: &str) -> Result<()> {
    if identifier.is_empty() || identifier.len() > 255 {
        bail!("implausible bundle identifier length");
    }
    if !identifier.contains('.') {
        bail!("bundle identifier `{identifier}` is not in reverse-DNS form");
    }
    if identifier.starts_with('.') || identifier.ends_with('.') || identifier.contains("..") {
        bail!("bundle identifier `{identifier}` has an empty component");
    }
    if !identifier
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        bail!("bundle identifier `{identifier}` contains unexpected characters");
    }
    Ok(())
}

fn application_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = APPLICATION_ROOTS.iter().map(PathBuf::from).collect();
    roots.push(home.join(USER_APPLICATION_ROOT));
    roots
}

fn is_approved_app_bundle(app: &Path, home: &Path) -> Result<bool> {
    let Some(parent) = app.parent() else {
        return Ok(false);
    };
    if !application_roots(home).iter().any(|root| parent == root) {
        return Ok(false);
    }
    if !app
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        return Ok(false);
    }
    let metadata = std::fs::symlink_metadata(app)
        .with_context(|| format!("cannot inspect {}", app.display()))?;
    // A symlinked bundle is refused: its target may live anywhere, and every
    // check after this one would be describing a different directory.
    Ok(metadata.file_type().is_dir())
}

/// Resolves a user-supplied name to exactly one installed bundle.
///
/// Ambiguity is refused rather than guessed: naming the wrong app here would
/// send the operator to delete another program's files.
pub(crate) fn resolve_app(name: &str, home: &Path) -> Result<PathBuf> {
    if name.is_empty() {
        bail!("name an application, for example `devtrim uninstall AltTab`");
    }
    // An explicit path is accepted as-is, still subject to the approval check.
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        if is_approved_app_bundle(candidate, home)? {
            return Ok(candidate.to_path_buf());
        }
        bail!(
            "{} is not an application bundle in {}",
            candidate.display(),
            describe_roots(home)
        );
    }

    let wanted = name.strip_suffix(".app").unwrap_or(name);
    let mut matches = Vec::new();
    for root in application_roots(home) {
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", root.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot enumerate {}", root.display()))?;
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if stem.eq_ignore_ascii_case(wanted) && is_approved_app_bundle(&path, home)? {
                matches.push(path);
            }
        }
    }
    matches.sort();
    match matches.as_slice() {
        [] => bail!("no application named `{name}` in {}", describe_roots(home)),
        [single] => Ok(single.clone()),
        several => {
            let listed = several
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("`{name}` is ambiguous; name the exact path: {listed}")
        }
    }
}

fn describe_roots(home: &Path) -> String {
    application_roots(home)
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Whether any running process executes from inside this bundle.
///
/// `ps -Axww -o comm=` prints each process's command path, so the check needs no
/// dynamic argument at all — the comparison happens here rather than in the
/// command line, and `ww` keeps a long path from being width-truncated into a
/// false negative. A probe that cannot complete blocks rather than reporting
/// "not running".
///
/// What this does NOT guarantee: `comm` is the command value, not an
/// authenticated executable path, so this is a courtesy check against reviewing
/// a live app's files, not a security boundary. Nothing here deletes anything.
pub(crate) fn parse_running_bundle(output: &str, app: &Path) -> bool {
    let prefix = format!("{}/", app.display());
    output.lines().any(|line| line.starts_with(&prefix))
}

fn app_is_running(app: &Path) -> Result<bool> {
    let output = Command::new("ps")
        .args(["-Axww", "-o", "comm="])
        .output()
        .context("cannot run `ps -Axww -o comm=`")?;
    if !output.status.success() {
        bail!("`ps -Axww -o comm=` failed with {}", output.status);
    }
    let listing = String::from_utf8(output.stdout).context("`ps` returned non-UTF-8 output")?;
    Ok(parse_running_bundle(&listing, app))
}

/// True when `name` is exactly `identifier`, never a prefix of a longer one.
pub(crate) fn matches_identifier(name: &str, identifier: &str) -> bool {
    name == identifier
}

/// True when `name` is exactly `<identifier>.<extension>`.
pub(crate) fn matches_identifier_file(name: &str, identifier: &str, extension: &str) -> bool {
    name.strip_suffix(extension)
        .and_then(|rest| rest.strip_suffix('.'))
        .is_some_and(|stem| stem == identifier)
}

/// Logical bytes under `path` and how many entries could not be read. A
/// protected folder inside a container lowers that one size to a disclosed
/// lower bound instead of hiding every path the report found.
fn measure(path: &Path) -> Result<(u64, usize)> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {}", path.display()))?;
    if metadata.file_type().is_dir() {
        let measured = crate::worktrees::measure_lenient(path, &Default::default());
        Ok((measured.bytes, measured.unread))
    } else {
        Ok((metadata.len(), 0))
    }
}

fn info_finding(label: impl Into<String>, path: PathBuf, note: &str) -> Result<Finding> {
    let (size, unread) = measure(&path)?;
    let note = if unread == 0 {
        note.to_owned()
    } else {
        format!("{note}; size is a lower bound: {unread} entries could not be read")
    };
    Ok(Finding::new(
        label,
        Some(path),
        size,
        &note,
        1,
        Action::Info,
    ))
}

/// Every path that belongs to `identifier`, plus the bundle itself.
pub(crate) fn locate(app: &Path, identifier: &str, home: &Path) -> Result<Vec<Finding>> {
    let mut findings = vec![info_finding(
        "application bundle",
        app.to_path_buf(),
        "report-only; devtrim refuses every path under /Applications",
    )?];

    for directory in ID_DIRECTORIES {
        let root = home.join(directory);
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", root.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot enumerate {}", root.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if matches_identifier(name, identifier) {
                findings.push(info_finding(
                    format!("{directory} entry"),
                    entry.path(),
                    "report-only; belongs to this bundle identifier",
                )?);
            }
        }
    }

    for (directory, extension) in ID_FILES {
        let root = home.join(directory);
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", root.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot enumerate {}", root.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if matches_identifier_file(name, identifier, extension) {
                findings.push(info_finding(
                    format!("{directory} entry"),
                    entry.path(),
                    "report-only; belongs to this bundle identifier",
                )?);
            }
        }
    }

    Ok(findings)
}

pub fn run(ctx: &Ctx, app: &str) -> Result<ExitCode> {
    let bundle = resolve_app(app, &ctx.home)?;
    let identifier = BundlePlist::for_app(&bundle, &ctx.home)?.bundle_identifier()?;
    // A running app's files are in use; reporting them as removable would be
    // advice that corrupts state if followed.
    if app_is_running(&bundle)? {
        bail!(
            "{} is running; quit it before reviewing its files",
            bundle.display()
        );
    }
    let findings = locate(&bundle, &identifier, &ctx.home)?;

    if ctx.json {
        report::print_json("uninstall", false, &findings, None, &[])?;
        return Ok(ExitCode::SUCCESS);
    }

    report::print_human(&findings)?;
    report::print_line(&format!(
        "\nbundle identifier: {}\n\n{} these are the paths macOS keys by that identifier, \
and only those. An app that stores data under a product name is not listed — Visual \
Studio Code keeps ~/Library/Application Support/Code, which contains no identifier — \
so treat this as a conservative report, not an inventory. devtrim does not remove \
them: /Applications and all of ~/Library outside a closed allowlist of developer-managed \
paths are refused \
by the protected-path boundary, for every command rather than only this one.",
        report::terminal_safe(&identifier),
        "report-only:".yellow().bold(),
    ))?;
    Ok(ExitCode::SUCCESS)
}
