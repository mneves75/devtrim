//! Read-only inventory of large iCloud Drive files and local allocation.

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

use crate::report::{Action, Finding};
use crate::safety::Ctx;

pub fn icloud_status(ctx: &Ctx) -> Result<Vec<Finding>> {
    let cloud_docs = ctx
        .home
        .join("Library/Mobile Documents/com~apple~CloudDocs");
    match std::fs::symlink_metadata(&cloud_docs) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => bail!(
            "iCloud Drive root is not a directory: {}",
            cloud_docs.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("cannot inspect iCloud Drive root {}", cloud_docs.display())
            });
        }
    }

    let mut findings = Vec::new();
    // A folder the walk cannot read hides only what is inside it: it is named
    // in one error entry and the files that could be read are still listed.
    let mut unread = Vec::new();
    for result in WalkDir::new(&cloud_docs)
        .follow_links(false)
        .follow_root_links(false)
    {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) if error.depth() > 0 => {
                unread.push(super::project::UnreadFolder::from_walk(&error));
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "cannot inventory iCloud Drive under {}",
                        cloud_docs.display()
                    )
                });
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let logical = entry
            .metadata()
            .with_context(|| format!("cannot inspect iCloud file {}", path.display()))?
            .len();
        if logical < 100 * 1024 * 1024 {
            continue;
        }
        let allocated = blocks_bytes(path)?;
        let relative = path.strip_prefix(&cloud_docs).unwrap_or(path);
        findings.push(Finding::new(
            format!("large iCloud Drive file: {}", relative.display()),
            Some(path.to_path_buf()),
            logical,
            format!(
                "{allocated} bytes allocated locally for {logical} logical bytes; allocation is an estimate and does not indicate iCloud upload status"
            ),
            1,
            Action::Info,
        ));
    }
    findings.sort_by(|left, right| left.path.cmp(&right.path));
    if !unread.is_empty() {
        findings.push(super::project::unread_folders_finding(
            "iCloud Drive files",
            &unread,
        ));
    }
    Ok(findings)
}

/// Real bytes on disk via st_blocks (detects sparse/dataless files).
fn blocks_bytes(path: &std::path::Path) -> Result<u64> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("cannot inspect iCloud file {}", path.display()))?;
    super::blocks_bytes(&metadata, path)
}
