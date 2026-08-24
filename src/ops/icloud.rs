//! iCloud Drive upload status: how much of each queued file is still
//! materialized locally (evictable only after upload completes).

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::report::{Action, Finding};
use crate::safety::Ctx;

pub fn icloud_status(ctx: &Ctx) -> Result<Vec<Finding>> {
    let docs = ctx
        .home
        .join("Library/Mobile Documents/com~apple~CloudDocs/Documents");
    if !docs.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for e in WalkDir::new(&docs).max_depth(1).into_iter().flatten() {
        let p = e.path();
        if p == docs.as_path() || !p.is_file() {
            continue;
        }
        let logical = e.metadata().map(|m| m.len()).unwrap_or(0);
        if logical < 100 * 1024 * 1024 {
            continue; // only interesting for big queued files
        }
        let on_disk = blocks_bytes(p)?;
        let pct = on_disk
            .saturating_mul(100)
            .checked_div(logical)
            .unwrap_or(100)
            .min(100);
        let label = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push(Finding::new(
            format!("{label} — {pct}% still local"),
            Some(p.to_path_buf()),
            logical,
            if pct >= 99 {
                "fully local; `brctl evict` will succeed once iCloud marks it uploaded"
            } else {
                "upload in progress; keep Mac awake and online; evict only after upload"
            },
            1,
            Action::Info,
        ));
    }
    Ok(out)
}

/// Real bytes on disk via st_blocks (detects sparse/dataless files).
fn blocks_bytes(p: &std::path::Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    let blocks = std::fs::metadata(p)
        .with_context(|| format!("cannot inspect iCloud file {}", p.display()))?
        .blocks();
    blocks
        .checked_mul(512)
        .ok_or_else(|| anyhow::anyhow!("allocated size overflow for {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_measurement_fails_closed() {
        let missing =
            std::env::temp_dir().join(format!("devtrim-icloud-missing-{}", std::process::id()));
        crate::ops::remove_test_path(&missing);

        let error = blocks_bytes(&missing).unwrap_err();

        assert!(error.to_string().contains("cannot inspect iCloud file"));
    }
}
