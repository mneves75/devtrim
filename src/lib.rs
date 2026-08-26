//! devtrim: developer-machine disk hygiene.
//!
//! Measure first, classify by risk, and apply only the reviewed plan.

pub mod app;
mod cli;
mod journal;
mod ops;
mod report;
mod safety;
mod tui;

#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzz_api {
    use std::path::{Path, PathBuf};

    pub fn validate_path_for_deletion(path: &Path, home: &Path) -> bool {
        crate::safety::validate_path_for_deletion(path, home, &[]).is_ok()
    }

    pub fn clean(path: &Path) -> PathBuf {
        crate::safety::clean(path)
    }

    pub fn parse_size(value: &str) -> anyhow::Result<u64> {
        crate::ops::docker::parse_size(value)
    }

    pub fn parse_pgrep_pids(output: &[u8], exit_code: Option<i32>) -> anyhow::Result<Vec<u32>> {
        crate::safety::parse_pgrep_pids(output, exit_code)
    }

    pub fn parse_lsof_cwds(output: &[u8], exit_code: Option<i32>) -> anyhow::Result<Vec<PathBuf>> {
        crate::safety::parse_lsof_cwds(output, exit_code)
    }

    pub fn parse_config_str(input: &str) -> anyhow::Result<()> {
        crate::safety::parse_config_str(input).map(|_| ())
    }
}
