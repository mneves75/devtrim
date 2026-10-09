//! Bounded external-command execution: every scan and apply-preflight
//! subprocess runs under a wall-clock limit. It must never return partial
//! output as if the command had finished, and a timeout is never an absent
//! program (`NotFound`), so a hung probe cannot read as "nothing running".

use std::io::{self, Read};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Read-only queries: Git, `simctl`, `docker`, `npm`, `brew`.
///
/// A query that normally takes milliseconds took tens of seconds on the
/// development machine at load average 300 to 950 with about 1,700 processes
/// (2026-10-08). Two minutes is far past that, yet a wedged tool still fails
/// the affected repository or category instead of hanging the whole scan.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(120);

/// Process-table probes: `pgrep` and `lsof`.
///
/// A system-wide `lsof` took 16 s at load average 300 on the same machine, and
/// load reached about 950. Scaling that linearly gives about 50 s, so three
/// minutes keeps a margin of more than three times over the worst observed
/// load; beyond it the probe refuses.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(180);

/// Typed maintenance and Docker/simulator commands run at apply, which do real
/// work (`docker builder prune`, `simctl delete unavailable`) rather than
/// answer a query. Killing one mid-run abandons the work, so the bound is only
/// a backstop against a wedged tool.
pub(crate) const MUTATION_TIMEOUT: Duration = Duration::from_secs(900);

/// How long a killed child gets to be reaped before it is reported unreaped.
/// A process stuck in uninterruptible I/O ignores SIGKILL until the I/O ends.
const REAP_GRACE: Duration = Duration::from_secs(5);

const FIRST_POLL: Duration = Duration::from_millis(1);
const LONGEST_POLL: Duration = Duration::from_millis(32);

pub(crate) trait BoundedCommand {
    /// What [`Command::output`] returns, within `limit`.
    ///
    /// Standard input is null and both output streams are captured, as with
    /// `output`. Both streams are drained while the child runs, so a large
    /// listing cannot stall it on a full pipe. Past `limit` the child is
    /// killed and reaped and the error is [`io::ErrorKind::TimedOut`], naming
    /// the program and the limit. A program that cannot start keeps its own
    /// error, `NotFound` included. Only the child itself is killed, never its
    /// descendants; output still held open by one is abandoned, not awaited.
    fn output_within(&mut self, limit: Duration) -> io::Result<Output>;
}

impl BoundedCommand for Command {
    fn output_within(&mut self, limit: Duration) -> io::Result<Output> {
        let limit = effective_limit(limit);
        let program = self.get_program().to_string_lossy().into_owned();
        let deadline = Instant::now() + limit;
        let mut child = self
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let timed_out = |detail: &str| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{program} timed out after {limit:?}{detail}"),
            )
        };

        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let (stdout, stderr) = match (stdout, stderr) {
            (Ok(stdout), Ok(stderr)) => (stdout, stderr),
            (Err(error), _) | (_, Err(error)) => {
                kill_and_reap(&mut child);
                return Err(error);
            }
        };

        let mut delay = FIRST_POLL;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    return Err(if kill_and_reap(&mut child) {
                        timed_out(" and was killed")
                    } else {
                        timed_out(" and could not be reaped after being killed")
                    });
                }
                Ok(None) => {
                    thread::sleep(delay);
                    delay = (delay * 2).min(LONGEST_POLL);
                }
                Err(error) => {
                    kill_and_reap(&mut child);
                    return Err(error);
                }
            }
        };

        // The child is gone, but a descendant it left behind can hold a pipe
        // open: wait for the captured output only until the deadline.
        let remaining = || deadline.saturating_duration_since(Instant::now());
        let collect = |receiver: mpsc::Receiver<io::Result<Vec<u8>>>| match receiver
            .recv_timeout(remaining())
        {
            Ok(bytes) => bytes,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(timed_out(" while its output was still held open"))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(format!(
                "{program} output reader stopped unexpectedly"
            ))),
        };
        Ok(Output {
            status,
            stdout: collect(stdout)?,
            stderr: collect(stderr)?,
        })
    }
}

/// Reads one pipe to the end on its own thread and hands the bytes over.
fn drain(
    pipe: Option<impl Read + Send + 'static>,
) -> io::Result<mpsc::Receiver<io::Result<Vec<u8>>>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("devtrim-command-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = match pipe {
                Some(mut pipe) => pipe.read_to_end(&mut bytes).map(|_| bytes),
                None => Ok(bytes),
            };
            // The receiver is gone once the deadline passed; nothing to tell.
            sender.send(result).ok();
        })?;
    Ok(receiver)
}

/// Kills the child and waits for it, for at most [`REAP_GRACE`]. Whether it
/// was reaped: an unreaped child stays a zombie until devtrim exits.
fn kill_and_reap(child: &mut Child) -> bool {
    // Failing to kill means the child already exited; `try_wait` reaps it.
    child.kill().ok();
    let deadline = Instant::now() + REAP_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(LONGEST_POLL),
            Ok(None) | Err(_) => return false,
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_CAP: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

/// Shortens every limit on this thread to `cap` until the guard drops, so a
/// test can make a hung program time out through the production call site
/// without waiting out the real limit. Other threads keep their limits.
#[cfg(test)]
pub(crate) struct LimitCap;

#[cfg(test)]
impl LimitCap {
    pub(crate) fn new(cap: Duration) -> Self {
        TEST_CAP.with(|cell| cell.set(Some(cap)));
        Self
    }
}

#[cfg(test)]
impl Drop for LimitCap {
    fn drop(&mut self) {
        TEST_CAP.with(|cell| cell.set(None));
    }
}

#[cfg(test)]
fn effective_limit(limit: Duration) -> Duration {
    TEST_CAP
        .with(std::cell::Cell::get)
        .map_or(limit, |cap| limit.min(cap))
}

#[cfg(not(test))]
fn effective_limit(limit: Duration) -> Duration {
    limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("devtrim-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn script(directory: &Path, name: &str, body: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Runs `body` on a thread and gives up after `wait`, so a regression that
    /// never times out fails the assertion instead of hanging the suite.
    fn within<T: Send + 'static>(
        wait: Duration,
        body: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            sender.send(body()).ok();
        });
        receiver.recv_timeout(wait).ok()
    }

    fn alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn a_hung_command_is_killed_reaped_and_reported_as_a_timeout() {
        let directory = temp("process-hang");
        let pid_file = directory.join("pid");
        let hang = script(
            &directory,
            "hang",
            &format!("echo $$ > '{}'\nexec sleep 30", pid_file.display()),
        );

        let result = within(Duration::from_secs(10), move || {
            Command::new(hang).output_within(Duration::from_millis(500))
        })
        .expect("PV process/timeout: the command was never bounded");
        let error = result.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        let message = error.to_string();
        assert!(message.contains("hang"), "names the command: {message}");
        assert!(message.contains("500ms"), "names the limit: {message}");
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        // `kill -0` still succeeds on a zombie, so this also proves the reap.
        assert!(!alive(pid.trim()), "the hung child {pid} was left behind");
        crate::ops::remove_test_path(directory);
    }

    #[test]
    fn a_command_that_finishes_in_time_returns_everything_it_wrote() {
        let directory = temp("process-finish");
        let quick = script(&directory, "quick", "echo out; echo err >&2; exit 3");
        // Well past a pipe's capacity, so a reader that waits for the exit
        // before draining would stall the child and report a false timeout.
        let flood = script(
            &directory,
            "flood",
            "i=0\nwhile [ $i -lt 4000 ]; do echo 0123456789012345678901234567890123456789012345678901234567890123456789; i=$((i+1)); done",
        );

        let output = Command::new(quick)
            .output_within(Duration::from_secs(60))
            .unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");

        let output = Command::new(flood)
            .output_within(Duration::from_secs(60))
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 4000 * 71);
        crate::ops::remove_test_path(directory);
    }

    #[test]
    fn a_missing_program_stays_not_found_and_is_not_a_timeout() {
        let error = Command::new("/nonexistent/devtrim-no-such-program")
            .output_within(Duration::from_secs(60))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn standard_input_is_closed_so_a_command_cannot_wait_on_the_terminal() {
        let directory = temp("process-stdin");
        let reader = script(&directory, "reader", "cat");
        let output = Command::new(reader)
            .output_within(Duration::from_secs(60))
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        crate::ops::remove_test_path(directory);
    }

    #[test]
    fn output_held_open_by_a_descendant_is_abandoned_at_the_deadline() {
        let directory = temp("process-descendant");
        let leaves = script(&directory, "leaves", "sleep 4 &\nexit 0");

        let started = Instant::now();
        let result = within(Duration::from_secs(20), move || {
            Command::new(leaves).output_within(Duration::from_millis(500))
        })
        .expect("the descendant held the call past its deadline");

        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        crate::ops::remove_test_path(directory);
    }
}
