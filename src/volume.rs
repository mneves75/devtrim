//! Free space on a volume, read with `statvfs`, and the report-only `disk`
//! command. A measurement never says devtrim freed it: other processes,
//! APFS snapshots and purgeable space move the same number.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::ExitCode;

use crate::ops::ApplyOutcome;
use crate::report::{self, HomeVolume};
use crate::safety::Ctx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FreeSpace {
    /// Bytes an unprivileged process may still allocate (`f_bavail`).
    pub(crate) available: u64,
    pub(crate) total: u64,
}

pub(crate) fn free_space(path: &Path) -> Result<FreeSpace> {
    let stat = rustix::fs::statvfs(path)
        .with_context(|| format!("cannot read free space: {}", path.display()))?;
    if stat.f_frsize == 0 || stat.f_blocks == 0 || stat.f_bavail > stat.f_blocks {
        bail!("invalid filesystem capacity metrics: {}", path.display());
    }
    let total = stat
        .f_blocks
        .checked_mul(stat.f_frsize)
        .with_context(|| format!("volume capacity overflow: {}", path.display()))?;
    let available = stat
        .f_bavail
        .checked_mul(stat.f_frsize)
        .with_context(|| format!("available-space overflow: {}", path.display()))?;
    Ok(FreeSpace { available, total })
}

/// Runs an apply between two reads of the home volume's free space. A read
/// that fails is recorded in the summary, never as an apply error: the apply
/// itself succeeded or failed on its own terms.
pub(crate) fn measure_apply(
    home: &Path,
    apply: impl FnOnce() -> Result<ApplyOutcome>,
) -> Result<ApplyOutcome> {
    measure_apply_with(home, free_space, apply)
}

fn measure_apply_with(
    home: &Path,
    read: impl Fn(&Path) -> Result<FreeSpace>,
    apply: impl FnOnce() -> Result<ApplyOutcome>,
) -> Result<ApplyOutcome> {
    let before = read(home);
    let mut outcome = apply()?;
    let after = read(home);
    outcome.summary.home_volume = Some(home_volume(before, after));
    Ok(outcome)
}

fn home_volume(before: Result<FreeSpace>, after: Result<FreeSpace>) -> HomeVolume {
    match (before, after) {
        (Ok(before), Ok(after)) => {
            let change = i128::from(after.available) - i128::from(before.available);
            match i64::try_from(change) {
                Ok(gained_bytes) => HomeVolume::Measured {
                    free_before: before.available,
                    free_after: after.available,
                    gained_bytes,
                },
                Err(_) => HomeVolume::Unmeasured {
                    error: "the free-space change does not fit a signed 64-bit byte count".into(),
                },
            }
        }
        (Err(error), _) => HomeVolume::Unmeasured {
            error: format!("before the apply: {error:#}"),
        },
        (_, Err(error)) => HomeVolume::Unmeasured {
            error: format!("after the apply: {error:#}"),
        },
    }
}

/// A byte count from the command line, with the spelling the user gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ByteSize {
    pub(crate) bytes: u64,
    pub(crate) spelled: String,
}

const MAX_SIZE_TEXT: usize = 32;
/// Fraction digits kept: the rest move a TiB count by under a millionth of a byte.
const FRACTION_DIGITS: usize = 18;
const FRACTION_SCALE: u128 = 1_000_000_000_000_000_000;

/// `20GiB`, `1.5G`, `500MB`, `4096`: a decimal number and an optional unit.
/// `K/M/G/T` and `KiB…TiB` are powers of 1024, `KB…TB` powers of 1000, a bare
/// number or `B` is bytes; units ignore case. A fraction of a byte is dropped.
pub(crate) fn parse_size(text: &str) -> Result<ByteSize, String> {
    if text.chars().count() > MAX_SIZE_TEXT {
        return Err(format!("a size is at most {MAX_SIZE_TEXT} characters"));
    }
    if text.is_empty() {
        return Err("a size cannot be blank".into());
    }
    if text.starts_with('-') {
        return Err("a size cannot be negative".into());
    }
    let number_end = text
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(number_end);
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() || (number.contains('.') && fraction.is_empty()) || fraction.contains('.') {
        return Err(format!(
            "'{text}' is not a size: expected a number such as 20, 1.5 or 500, then an optional unit"
        ));
    }
    let multiplier: u128 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kib" => 1 << 10,
        "m" | "mib" => 1 << 20,
        "g" | "gib" => 1 << 30,
        "t" | "tib" => 1 << 40,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        _ => {
            return Err(format!(
                "unknown size unit in '{text}': use B, K, M, G, T, KiB…TiB or KB…TB"
            ));
        }
    };
    let too_large = || format!("'{text}' is larger than {} bytes", u64::MAX);
    let whole: u128 = whole.parse().map_err(|_| too_large())?;
    let kept = &fraction[..fraction.len().min(FRACTION_DIGITS)];
    let padding = u32::try_from(FRACTION_DIGITS - kept.len()).map_err(|_| too_large())?;
    let fraction: u128 = if kept.is_empty() {
        0
    } else {
        kept.parse::<u128>().map_err(|_| too_large())? * 10u128.pow(padding)
    };
    let bytes = whole
        .checked_mul(FRACTION_SCALE)
        .and_then(|scaled| scaled.checked_add(fraction))
        .and_then(|scaled| scaled.checked_mul(multiplier))
        .map(|scaled| scaled / FRACTION_SCALE)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(too_large)?;
    Ok(ByteSize {
        bytes,
        spelled: text.to_owned(),
    })
}

#[derive(serde::Serialize)]
struct DiskDocument<'a> {
    operation: &'static str,
    available_bytes: u64,
    total_bytes: u64,
    warn_below_bytes: Option<u64>,
    below: bool,
    errors: &'a [String],
}

/// Free space on the home folder's volume. With a threshold it speaks only
/// when free space is below it, so a session hook stays quiet otherwise;
/// either way the answer is not a failure. An unreadable volume is.
pub(crate) fn run_disk(ctx: &Ctx, warn_below: Option<&ByteSize>) -> Result<ExitCode> {
    let space = free_space(&ctx.home).context("cannot read the home volume")?;
    let below = warn_below.is_some_and(|limit| space.available < limit.bytes);
    if ctx.json {
        let document = DiskDocument {
            operation: "disk",
            available_bytes: space.available,
            total_bytes: space.total,
            warn_below_bytes: warn_below.map(|limit| limit.bytes),
            below,
            errors: &[],
        };
        let mut output =
            serde_json::to_vec_pretty(&document).context("cannot serialize the disk report")?;
        output.push(b'\n');
        report::write_stdout(&output)?;
    } else if let Some(limit) = warn_below {
        if below {
            report::print_line(&report::terminal_safe(&format!(
                "warning: only {} free on the home volume (below {})",
                report::gb(space.available),
                limit.spelled
            )))?;
        }
    } else {
        let percent = space.available as f64 * 100.0 / space.total as f64;
        report::print_line(&format!(
            "home volume: {} free of {} ({percent:.0}% free)",
            report::gb(space.available),
            report::gb(space.total)
        ))?;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::{Config as ProptestConfig, RngSeed};

    fn bytes(text: &str) -> u64 {
        parse_size(text).unwrap().bytes
    }

    fn refusal(text: &str) -> String {
        parse_size(text).unwrap_err()
    }

    #[test]
    fn sizes_parse_binary_and_decimal_units_in_any_case() {
        assert_eq!(bytes("4096"), 4096);
        assert_eq!(bytes("100B"), 100);
        assert_eq!(bytes("1K"), 1024);
        assert_eq!(bytes("1kib"), 1024);
        assert_eq!(bytes("1KB"), 1000);
        assert_eq!(bytes("1kb"), 1000);
        assert_eq!(bytes("20GiB"), 20 << 30);
        assert_eq!(bytes("20g"), 20 << 30);
        assert_eq!(bytes("2TB"), 2_000_000_000_000);
        assert_eq!(bytes("1000TiB"), 1000 << 40);
        assert_eq!(bytes("1.5G"), 3 << 29);
        assert_eq!(bytes("0.5KB"), 500);
        // A fraction of a byte is dropped, never rounded up.
        assert_eq!(bytes("1.9"), 1);
        assert_eq!(bytes("1.0000000000000000000000000T"), 1 << 40);
        assert_eq!(bytes("18446744073709551615"), u64::MAX);
        assert_eq!(parse_size("20GiB").unwrap().spelled, "20GiB");
    }

    #[test]
    fn sizes_refuse_what_is_not_a_byte_count() {
        assert!(refusal("").contains("blank"));
        assert!(refusal("-5G").contains("negative"));
        assert!(refusal("12XB").contains("unknown size unit"));
        assert!(refusal("5 G").contains("unknown size unit"));
        assert!(refusal("1e3").contains("unknown size unit"));
        for text in ["nan", "inf", "NaN", "+5", " 5", ".5G", "5.G", "1.2.3", "G"] {
            assert!(refusal(text).contains("is not a size"), "{text}");
        }
        assert!(refusal("18446744073709551616").contains("larger than"));
        assert!(refusal("16777216TiB").contains("larger than"));
        assert!(refusal("99999999999999999999999999999999").contains("larger than"));
        let long = "1".repeat(MAX_SIZE_TEXT + 1);
        assert!(refusal(&long).contains("at most 32 characters"));
        // The boundary itself is accepted, so the length check is not wider.
        assert_eq!(bytes(&format!("{}K", "0".repeat(MAX_SIZE_TEXT - 2))), 0);
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            failure_persistence: None,
            rng_seed: RngSeed::Fixed(0xD15C),
            ..ProptestConfig::default()
        })]

        #[test]
        fn size_parsing_never_panics(text in "\\PC{0,40}") {
            let _ = parse_size(&text);
        }

        #[test]
        fn whole_numbers_with_units_scale_exactly(count in 0u64..1_000_000, unit in 0usize..4) {
            let (suffix, multiplier) = [("K", 1u64 << 10), ("M", 1 << 20), ("GB", 1_000_000_000), ("TiB", 1 << 40)][unit];
            prop_assert_eq!(bytes(&format!("{count}{suffix}")), count * multiplier);
        }
    }

    fn outcome(errors: Vec<String>) -> ApplyOutcome {
        let mut outcome = ApplyOutcome::new("caches");
        outcome.summary.items_touched = 1;
        outcome.errors = errors;
        outcome
    }

    fn space(available: u64) -> FreeSpace {
        FreeSpace {
            available,
            total: 1 << 40,
        }
    }

    #[test]
    fn a_failed_read_is_recorded_in_the_summary_and_never_as_an_apply_error() {
        let home = Path::new("/nonexistent/home");
        let failing = |_: &Path| -> Result<FreeSpace> { anyhow::bail!("injected statvfs failure") };

        let measured = measure_apply_with(home, failing, || Ok(outcome(Vec::new()))).unwrap();
        assert!(
            measured.errors.is_empty(),
            "a measurement failure must not change the apply's exit status: {:?}",
            measured.errors
        );
        let document = serde_json::to_value(&measured.summary).unwrap();
        let error = document["home_volume"]["error"].as_str().unwrap();
        assert!(error.contains("injected statvfs failure"), "{document}");
        assert!(document["home_volume"].get("free_before").is_none());

        // The apply's own errors pass through exactly, no more and no fewer.
        let partial =
            measure_apply_with(home, failing, || Ok(outcome(vec!["refused".into()]))).unwrap();
        assert_eq!(partial.errors, ["refused"]);

        // Control: a working reader gives the measured shape.
        let reads = std::cell::Cell::new(0u64);
        let working = |_: &Path| {
            reads.set(reads.get() + 1);
            Ok(space(if reads.get() == 1 { 5_000 } else { 3_000 }))
        };
        let measured = measure_apply_with(home, working, || Ok(outcome(Vec::new()))).unwrap();
        let document = serde_json::to_value(&measured.summary).unwrap();
        assert_eq!(document["home_volume"]["free_before"], 5_000);
        assert_eq!(document["home_volume"]["free_after"], 3_000);
        assert_eq!(document["home_volume"]["gained_bytes"], -2_000);
        assert!(document["home_volume"].get("error").is_none());
    }

    #[test]
    fn a_failed_apply_propagates_without_a_measurement() {
        let result = measure_apply_with(
            Path::new("/"),
            |_| Ok(space(1)),
            || anyhow::bail!("apply refused"),
        );
        assert_eq!(result.unwrap_err().to_string(), "apply refused");
    }

    #[test]
    fn free_space_reads_a_real_volume_and_names_an_unreadable_path() {
        let space = free_space(Path::new("/")).unwrap();
        assert!(space.total > 0 && space.available <= space.total);
        let missing = Path::new("/nonexistent/devtrim-volume");
        let error = format!("{:#}", free_space(missing).unwrap_err());
        assert!(error.contains("/nonexistent/devtrim-volume"), "{error}");
    }
}
