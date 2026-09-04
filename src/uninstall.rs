//! Application leftovers, located by exact bundle identifier.
//!
//! This command reports; it never deletes, and that is a structural fact rather
//! than a policy choice. `safety::is_protected` refuses `/Applications` and
//! everything beneath it, and refuses everything under `~/Library` outside a
//! four-entry allowlist — a boundary the repository's own property tests assert
//! (`is_protected(home/"Library/Application Support")`). Deleting an app bundle
//! and its support files would require widening that list for every code path,
//! not just this one, so the useful half devtrim can honestly do is the half a
//! person cannot do by hand: find every file that actually belongs to an app.
//!
//! Matching is by EXACT bundle identifier, never by name. `com.example.thing`
//! must not select `com.example.thingy`, and a display name like "Notes" must
//! not select every path containing the word. The identifier is read from the
//! bundle's own `Info.plist`, which is the only structural evidence macOS
//! actually keys its support directories on.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};

use anyhow::{Context, Result, bail};
use colored::Colorize;

use crate::ops::dir_size;
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

/// Group containers are named `<team id>.<bundle identifier>` and are shared by
/// every app from that team, so a match here is evidence of association, never
/// of ownership.
const SHARED_DIRECTORIES: &[&str] = &["Library/Group Containers"];

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
/// `ps -Axo comm=` prints each process's full executable path, so the check
/// needs no dynamic argument at all — the comparison happens here rather than
/// in the command line. A probe that cannot complete blocks rather than
/// reporting "not running".
pub(crate) fn parse_running_bundle(output: &str, app: &Path) -> bool {
    let prefix = format!("{}/", app.display());
    output.lines().any(|line| line.starts_with(&prefix))
}

fn app_is_running(app: &Path) -> Result<bool> {
    let output = Command::new("ps")
        .args(["-Axo", "comm="])
        .output()
        .context("cannot run `ps -Axo comm=`")?;
    if !output.status.success() {
        bail!("`ps -Axo comm=` failed with {}", output.status);
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

/// True when `name` is `<team>.<identifier>` — association, not ownership.
pub(crate) fn matches_group_container(name: &str, identifier: &str) -> bool {
    name.strip_suffix(identifier)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .is_some_and(|team| !team.is_empty() && !team.contains('/'))
}

fn measure(path: &Path) -> Result<u64> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {}", path.display()))?;
    if metadata.file_type().is_dir() {
        dir_size(path)
    } else {
        Ok(metadata.len())
    }
}

fn info_finding(label: String, path: PathBuf, note: &str) -> Result<Finding> {
    let size = measure(&path)?;
    Ok(Finding::new(label, Some(path), size, note, 1, Action::Info))
}

/// Every path that belongs to `identifier`, plus the bundle itself.
pub(crate) fn locate(app: &Path, identifier: &str, home: &Path) -> Result<Vec<Finding>> {
    let mut findings = vec![info_finding(
        "application bundle".to_string(),
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

    for directory in SHARED_DIRECTORIES {
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
            if matches_group_container(name, identifier) {
                findings.push(info_finding(
                    format!("{directory} entry"),
                    entry.path(),
                    "SHARED: a group container serves every app from this team; association, not ownership",
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
        "\nbundle identifier: {}\n\n{} devtrim reports these and does not remove them. \
Deleting an application bundle or its support files would require widening the \
protected-path boundary that refuses /Applications and all of ~/Library outside \
a four-entry allowlist, for every command rather than only this one. Review the \
list, then remove what you recognize.",
        report::terminal_safe(&identifier),
        "report-only:".yellow().bold(),
    ))?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("devtrim-uninstall")
            .tempdir()
            .unwrap()
    }

    fn make_app(root: &Path, name: &str, identifier: &str) -> PathBuf {
        let app = root.join(format!("{name}.app"));
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
<key>CFBundleIdentifier</key><string>{identifier}</string></dict></plist>"
            ),
        )
        .unwrap();
        app
    }

    /// The whole safety argument rests on this: a prefix must never match.
    #[test]
    fn identifiers_match_exactly_and_never_by_prefix() {
        assert!(matches_identifier("com.example.thing", "com.example.thing"));
        assert!(!matches_identifier(
            "com.example.thingy",
            "com.example.thing"
        ));
        assert!(!matches_identifier("com.example.thin", "com.example.thing"));

        assert!(matches_identifier_file(
            "com.example.thing.plist",
            "com.example.thing",
            "plist"
        ));
        assert!(!matches_identifier_file(
            "com.example.thingy.plist",
            "com.example.thing",
            "plist"
        ));
        // A different extension in the same directory is not this app's file.
        assert!(!matches_identifier_file(
            "com.example.thing.savedState",
            "com.example.thing",
            "plist"
        ));
    }

    #[test]
    fn group_containers_need_a_team_prefix_and_an_exact_suffix() {
        assert!(matches_group_container(
            "ABCDE12345.com.example.thing",
            "com.example.thing"
        ));
        // No team prefix: not a group container name.
        assert!(!matches_group_container(
            "com.example.thing",
            "com.example.thing"
        ));
        assert!(!matches_group_container(
            "ABCDE12345.com.example.thingy",
            "com.example.thing"
        ));
    }

    #[test]
    fn bundle_identifiers_are_validated_as_filenames() {
        assert!(validate_bundle_identifier("com.example.thing").is_ok());
        for bad in [
            "",
            "noreversedns",
            ".com.example",
            "com.example.",
            "com..example",
            "com/example.thing",
            "com.example.thing\n../../etc",
            "com.example.thing;rm",
        ] {
            assert!(validate_bundle_identifier(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn a_running_bundle_is_detected_by_executable_path_prefix() {
        let listing = "/usr/libexec/somed\n\
/Applications/AltTab.app/Contents/MacOS/AltTab\n\
/Applications/Other.app/Contents/MacOS/Other\n";
        assert!(parse_running_bundle(
            listing,
            Path::new("/Applications/AltTab.app")
        ));
        assert!(!parse_running_bundle(
            listing,
            Path::new("/Applications/Missing.app")
        ));
        // A sibling whose name merely starts the same must not count.
        assert!(!parse_running_bundle(
            listing,
            Path::new("/Applications/AltTab")
        ));
    }

    #[test]
    fn resolution_refuses_ambiguity_and_paths_outside_the_roots() {
        let home = home();
        let user_apps = home.path().join(USER_APPLICATION_ROOT);
        std::fs::create_dir_all(&user_apps).unwrap();
        make_app(&user_apps, "Widget", "com.example.widget");

        assert_eq!(
            resolve_app("Widget", home.path()).unwrap(),
            user_apps.join("Widget.app")
        );
        assert_eq!(
            resolve_app("widget.app", home.path()).unwrap(),
            user_apps.join("Widget.app"),
            "the name is matched case-insensitively, with or without the suffix"
        );
        assert!(resolve_app("Nonexistent", home.path()).is_err());
        assert!(
            resolve_app(
                &home
                    .path()
                    .join("elsewhere/Widget.app")
                    .display()
                    .to_string(),
                home.path()
            )
            .is_err(),
            "a bundle outside the Applications roots must be refused"
        );
    }

    #[test]
    fn a_symlinked_bundle_is_refused() {
        let home = home();
        let user_apps = home.path().join(USER_APPLICATION_ROOT);
        std::fs::create_dir_all(&user_apps).unwrap();
        let real = make_app(home.path(), "Real", "com.example.real");
        let link = user_apps.join("Linked.app");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(!is_approved_app_bundle(&link, home.path()).unwrap());
    }

    #[test]
    fn located_paths_are_reported_and_never_actionable() {
        let home = home();
        let user_apps = home.path().join(USER_APPLICATION_ROOT);
        std::fs::create_dir_all(&user_apps).unwrap();
        let app = make_app(&user_apps, "Widget", "com.example.widget");

        let identifier = "com.example.widget";
        std::fs::create_dir_all(home.path().join("Library/Application Support")).unwrap();
        std::fs::create_dir_all(
            home.path()
                .join("Library/Application Support")
                .join(identifier),
        )
        .unwrap();
        // A sibling whose identifier merely extends this one must stay out.
        std::fs::create_dir_all(
            home.path()
                .join("Library/Application Support")
                .join(format!("{identifier}y")),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("Library/Preferences")).unwrap();
        std::fs::write(
            home.path()
                .join("Library/Preferences")
                .join(format!("{identifier}.plist")),
            b"x",
        )
        .unwrap();

        let findings = locate(&app, identifier, home.path()).unwrap();
        let paths: Vec<String> = findings
            .iter()
            .filter_map(|finding| finding.path.clone())
            .collect();

        assert!(paths.iter().any(|path| path.ends_with("Widget.app")));
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with(&format!("Application Support/{identifier}")))
        );
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with(&format!("Preferences/{identifier}.plist")))
        );
        assert!(
            !paths
                .iter()
                .any(|path| path.ends_with(&format!("{identifier}y"))),
            "a longer identifier must never be swept in"
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.action == Action::Info),
            "this command must never produce an actionable finding"
        );
    }
}
