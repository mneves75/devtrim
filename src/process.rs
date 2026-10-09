//! Bounded external-command execution: every scan and apply-preflight
//! subprocess runs under a wall-clock limit. It must never return partial
//! output as if the command had finished, and a timeout is never an absent
//! program (`NotFound`), so a hung probe cannot read as "nothing running".

use std::io::{self, Read};
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Read-only queries: Git, `simctl`, `docker`, `npm`, `brew`.
///
/// A margin, not a measurement. On the development machine at load average 300
/// to 950 with about 1,700 processes (2026-10-08) only a system-wide `lsof` was
/// timed (16 s at load 300); no Git, `simctl`, `docker`, `npm` or `brew` query
/// was. A query that takes milliseconds when idle gets two minutes, which a
/// loaded machine should not exhaust, while a wedged tool still fails the
/// affected repository or category instead of hanging the whole scan.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(120);

/// Process-table probes: `pgrep` and `lsof`.
///
/// A system-wide `lsof` took 16 s at load average 300 on the same machine
/// (measured), and load reached about 950. Scaling that linearly gives about
/// 50 s (an extrapolation), so three minutes keeps a margin of more than three
/// times over it; beyond that the probe refuses.
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
    /// error, `NotFound` included.
    ///
    /// The child leads a process group of its own, and a timeout kills that
    /// whole group, so what a tool spawned (`docker`'s buildx plugin, a helper
    /// of `git`) does not outlive the failure that was reported. The cost: the
    /// terminal's SIGINT reaches devtrim only, so an interrupted devtrim does
    /// not signal a command in flight. A read or probe ends when it finishes or
    /// on SIGPIPE once devtrim's pipes close; one that is stuck stays running
    /// until it ends, and a typed mutation command runs on.
    /// Output still held open by a descendant after the child exited cleanly is
    /// abandoned, not awaited, and the descendant is left alone: it may be
    /// something the command meant to leave running.
    fn output_within(&mut self, limit: Duration) -> io::Result<Output>;
}

impl BoundedCommand for Command {
    fn output_within(&mut self, limit: Duration) -> io::Result<Output> {
        let limit = effective_limit(limit);
        let program = self.get_program().to_string_lossy().into_owned();
        let deadline = Instant::now() + limit;
        let mut child = self
            .process_group(0)
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

/// Kills the child's whole process group and waits for the child, for at most
/// [`REAP_GRACE`]. Whether it was reaped: an unreaped child stays a zombie
/// until devtrim exits.
///
/// The group is signalled before the child is reaped: while the leader is
/// unreaped its process id cannot be reused, so the group id still names the
/// group this call created and nothing else.
fn kill_and_reap(child: &mut Child) -> bool {
    let group = i32::try_from(child.id())
        .ok()
        .filter(|id| *id > 1)
        .and_then(rustix::process::Pid::from_raw);
    // Failing to kill means the group is already gone; `try_wait` reaps it.
    if let Some(group) = group {
        rustix::process::kill_process_group(group, rustix::process::Signal::KILL).ok();
    }
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

    /// A shell script, for the cases where the shell itself is what the test
    /// needs: capturing its own pid, writing to stderr with an exit code, or
    /// leaving a background process behind.
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
        Command::new("/bin/kill")
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
            &format!("echo $$ > '{}'\nexec sleep 60", pid_file.display()),
        );

        // The limit leaves a loaded machine seconds to start the shell, and the
        // guard and the sleep leave a regression that never times out room to
        // fail here rather than hang or be mistaken for a normal exit.
        let result = within(Duration::from_secs(25), move || {
            Command::new(hang).output_within(Duration::from_secs(5))
        })
        .expect("PV process/timeout: the command was never bounded");
        let error = result.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        let message = error.to_string();
        assert!(message.contains("hang"), "names the command: {message}");
        assert!(message.contains("5s"), "names the limit: {message}");
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        // `kill -0` still succeeds on a zombie, so this also proves the reap.
        assert!(!alive(pid.trim()), "the hung child {pid} was left behind");
        crate::ops::remove_test_path(directory);
    }

    /// `docker builder prune` runs through a buildx plugin child, and a Git
    /// helper is a child too: killing only the command would report the failure
    /// while the work went on. A shell that starts a background `sleep` and
    /// waits for it stands for such a tool.
    #[test]
    fn a_timed_out_command_takes_its_descendants_with_it() {
        let directory = temp("process-family");
        let pid_file = directory.join("descendant");
        let family = script(
            &directory,
            "family",
            &format!("sleep 60 &\necho $! > '{}'\nwait", pid_file.display()),
        );

        let result = within(Duration::from_secs(25), move || {
            Command::new(family).output_within(Duration::from_secs(5))
        })
        .expect("the command was never bounded");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);

        let pid = std::fs::read_to_string(&pid_file).unwrap();
        // The killed `sleep` is reparented and reaped by launchd, which can
        // take a moment; a surviving one is still there 60 seconds later.
        let mut gone = false;
        for _ in 0..50 {
            if !alive(pid.trim()) {
                gone = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            gone,
            "PV process/descendants: the timed-out command's child {pid} still runs"
        );
        crate::ops::remove_test_path(directory);
    }

    #[test]
    fn a_command_that_finishes_in_time_returns_everything_it_wrote() {
        let directory = temp("process-finish");
        let quick = script(&directory, "quick", "echo out; echo err >&2; exit 3");

        let output = Command::new(quick)
            .output_within(Duration::from_secs(60))
            .unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");

        // 588,895 bytes, far past a pipe's capacity: a reader that waited for
        // the exit before draining would stall the child and report a false
        // timeout.
        let output = Command::new("/usr/bin/seq")
            .args(["1", "100000"])
            .output_within(Duration::from_secs(60))
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 588_895);
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
        let output = Command::new("/bin/cat")
            .output_within(Duration::from_secs(60))
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn output_held_open_by_a_descendant_is_abandoned_at_the_deadline() {
        let directory = temp("process-descendant");
        // The shell exits at once and its background `sleep` keeps the pipes
        // open far past the limit, so the child is already gone when the
        // deadline passes.
        let leaves = script(&directory, "leaves", "sleep 40 &\nexit 0");

        let result = within(Duration::from_secs(30), move || {
            Command::new(leaves).output_within(Duration::from_secs(5))
        })
        .expect("the descendant held the call past its deadline");

        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(
            error.to_string().contains("still held open"),
            "the held-open branch was not reached: {error}"
        );
        crate::ops::remove_test_path(directory);
    }
}
