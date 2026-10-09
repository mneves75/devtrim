//! Bounded external-command execution: every scan and apply-preflight
//! subprocess runs under a wall-clock limit. It must never return partial
//! output as if the command had finished, and a timeout is never an absent
//! program (`NotFound`), so a hung probe cannot read as "nothing running".
//! It also owns the one registry of live command groups, so an interrupted
//! devtrim kills every command it started before it exits.

use std::collections::BTreeSet;
use std::io::{self, Read};
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
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
    /// of `git`) does not outlive the failure that was reported. The terminal's
    /// SIGINT reaches devtrim only, so the group is also registered for
    /// [`exit_on_interrupt`], which kills every live group before devtrim
    /// exits.
    ///
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
        let mut child = spawn_registered(
            self.process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )?;
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
            match poll_exit(&mut child) {
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

/// The process groups of commands that are running now, and whether devtrim
/// has been interrupted. Every command devtrim runs is registered here, and
/// only [`poll_exit`] reaps one.
///
/// Reaping and removal happen under the registry's lock, as does the kill on
/// interrupt, so a group is never signalled after its leader was reaped: once
/// the leader is gone its process id can name an unrelated process.
struct Registry {
    groups: BTreeSet<i32>,
    /// Commands being spawned right now, not yet registered.
    spawning: usize,
    interrupted: bool,
}

static LIVE: Mutex<Registry> = Mutex::new(Registry {
    groups: BTreeSet::new(),
    spawning: 0,
    interrupted: false,
});

/// Signalled when a spawn finishes, for [`exit_on_interrupt`] to wait on.
static SPAWNED: Condvar = Condvar::new();

fn live() -> MutexGuard<'static, Registry> {
    LIVE.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Registry {
    /// Kills every registered group. Their leaders are unreaped, so each id
    /// still names the group it was registered for.
    fn kill_all(&self) {
        for group in &self.groups {
            if let Some(group) = rustix::process::Pid::from_raw(*group) {
                rustix::process::kill_process_group(group, rustix::process::Signal::KILL).ok();
            }
        }
    }
}

/// Spawns `command` and registers its group before anything else can run, so
/// a descendant born at once is already covered. Refuses after an interrupt:
/// devtrim is about to exit and must not start another command.
///
/// The lock is not held across the spawn, so commands still start in parallel.
/// A spawn in flight is counted instead, and [`exit_on_interrupt`] waits for
/// the count to reach zero before it kills, so no group is missed.
fn spawn_registered(command: &mut Command) -> io::Result<Child> {
    {
        let mut registry = live();
        if registry.interrupted {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "devtrim was interrupted",
            ));
        }
        registry.spawning += 1;
    }
    let spawned = command.spawn();
    let mut registry = live();
    registry.spawning = registry.spawning.saturating_sub(1);
    let registered = spawned.and_then(|mut child| match i32::try_from(child.id()) {
        Ok(group) => {
            registry.groups.insert(group);
            Ok(child)
        }
        // A group id that does not fit could never be killed on interrupt.
        Err(_) => {
            child.kill().ok();
            child.wait().ok();
            Err(io::Error::other("child process id out of range"))
        }
    });
    drop(registry);
    SPAWNED.notify_all();
    registered
}

/// `try_wait` that unregisters the group in the same critical section that
/// reaps its leader.
fn poll_exit(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    let mut registry = live();
    let status = child.try_wait()?;
    if status.is_some()
        && let Ok(group) = i32::try_from(child.id())
    {
        registry.groups.remove(&group);
    }
    Ok(status)
}

/// Kills every command devtrim started when it receives SIGINT or SIGTERM,
/// then exits with the conventional status: 130 or 143.
///
/// Each command leads a process group of its own, so the terminal's SIGINT
/// never reaches it; without this an interrupted run would leave a `docker`
/// prune or a stuck query running. The kill happens under the registry's lock
/// before the exit, and leaders are left for the kernel to reap. A journal
/// attempt in flight stays unmatched, which `history` reports as interrupted.
///
/// Nothing between the signal and the exit can block on the outside world: no
/// message is written, since a full or unread stderr would stall the write
/// while the run kept deleting. A second signal exits at once, without
/// waiting for the first kill to finish.
///
/// Installing the handler replaces a SIGINT that was ignored at startup, as in
/// a background job of a non-interactive shell: such a run now stops on SIGINT.
/// std and rustix offer no safe way to read the earlier disposition.
///
/// Not installed for the TUI, which runs in raw mode where Ctrl-C is a key and
/// owns its terminal restoration.
pub(crate) fn exit_on_interrupt() -> io::Result<()> {
    use signal_hook::consts::{SIGINT, SIGTERM};

    let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM])?;
    thread::Builder::new()
        .name("devtrim-interrupt".into())
        .spawn(move || {
            let mut stopping = false;
            for signal in signals.forever() {
                if stopping {
                    std::process::exit(128 + signal);
                }
                stopping = true;
                // On its own thread, so a second signal is still heard while
                // the kill waits for the lock.
                let stop = thread::Builder::new()
                    .name("devtrim-interrupt-stop".into())
                    .spawn(move || kill_and_exit(signal));
                if stop.is_err() {
                    kill_and_exit(signal);
                }
            }
        })?;
    Ok(())
}

/// How long an interrupt waits for a spawn in flight to register its group.
/// A spawn takes milliseconds, so the bound only keeps a wedged one from
/// delaying the exit.
const SPAWN_GRACE: Duration = Duration::from_secs(2);

fn kill_and_exit(signal: i32) -> ! {
    let mut registry = live();
    registry.interrupted = true;
    registry = SPAWNED
        .wait_timeout_while(registry, SPAWN_GRACE, |registry| registry.spawning > 0)
        .unwrap_or_else(PoisonError::into_inner)
        .0;
    registry.kill_all();
    // The lock stays held: no command can be reaped, registered or started
    // between the kill and the exit.
    std::process::exit(128 + signal)
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
        match poll_exit(child) {
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

    /// Starts a shell script as a registered group leader, as `output_within`
    /// does, with its output discarded.
    fn start_family(directory: &Path, body: &str) -> Child {
        let family = script(directory, "family", body);
        spawn_registered(
            Command::new(family)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .unwrap()
    }

    /// Waits for a pid file the script writes. The script's own progress is the
    /// readiness signal: how long a loaded machine takes to start a shell is
    /// not something a test can guess.
    fn pid_from(path: &Path) -> String {
        for _ in 0..3000 {
            if let Ok(pid) = std::fs::read_to_string(path)
                && pid.ends_with('\n')
            {
                return pid.trim().to_owned();
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("the script never wrote {}", path.display());
    }

    fn registered(pid: &str) -> bool {
        let group = pid.parse::<i32>().unwrap();
        live().groups.contains(&group)
    }

    #[test]
    fn a_hung_command_is_killed_reaped_and_reported_as_a_timeout() {
        // The guard and the sleep leave a regression that never times out room
        // to fail here rather than hang. `sleep` starts at once, so the limit
        // depends on no script making progress.
        let result = within(Duration::from_secs(25), || {
            Command::new("/bin/sleep")
                .arg("60")
                .output_within(Duration::from_secs(3))
        })
        .expect("PV process/timeout: the command was never bounded");
        let error = result.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        let message = error.to_string();
        assert!(
            message.contains("/bin/sleep"),
            "names the command: {message}"
        );
        assert!(message.contains("3s"), "names the limit: {message}");
        // That wording is only used once the leader was reaped.
        assert!(message.contains("and was killed"), "{message}");
    }

    /// `docker builder prune` runs through a buildx plugin child, and a Git
    /// helper is a child too: killing only the command would report the failure
    /// while the work went on. A shell that starts a background `sleep` and
    /// waits for it stands for such a tool, and `kill_and_reap` is what a
    /// timeout runs.
    #[test]
    fn a_timed_out_command_takes_its_descendants_with_it() {
        let directory = temp("process-family");
        let leader_file = directory.join("leader");
        let descendant_file = directory.join("descendant");
        let mut child = start_family(
            &directory,
            &format!(
                "echo $$ > '{}'\nsleep 600 &\necho $! > '{}'\nwait",
                leader_file.display(),
                descendant_file.display()
            ),
        );
        let leader = pid_from(&leader_file);
        let descendant = pid_from(&descendant_file);
        assert!(
            alive(&descendant),
            "control: the child runs before the kill"
        );

        assert!(kill_and_reap(&mut child), "the leader was not reaped");

        // `kill -0` still succeeds on a zombie, so this proves the reap.
        assert!(!alive(&leader), "the leader {leader} was left behind");
        // The killed `sleep` is reparented and reaped by launchd, which can
        // take a moment. The limit is wall-clock, because on a loaded machine
        // each `kill` takes seconds to start, and the `sleep` outlives it.
        let started = Instant::now();
        let mut gone = false;
        while started.elapsed() < Duration::from_secs(60) {
            if !alive(&descendant) {
                gone = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        Command::new("/bin/kill")
            .args(["-9", &descendant])
            .output()
            .ok();
        assert!(
            gone,
            "PV process/descendants: the timed-out command's child {descendant} still runs"
        );
        crate::ops::remove_test_path(directory);
    }

    /// A group is registered from the spawn until its leader is reaped, and no
    /// later: an interrupt that found a reaped leader's id could signal an
    /// unrelated process group that reused it.
    #[test]
    fn a_command_is_registered_while_it_runs_and_unregistered_once_reaped() {
        let directory = temp("process-registry");
        let leader_file = directory.join("leader");
        let mut child = start_family(
            &directory,
            &format!("echo $$ > '{}'\nexec sleep 60", leader_file.display()),
        );
        let leader = pid_from(&leader_file);
        assert!(
            registered(&leader),
            "control: a running command is registered"
        );

        assert!(kill_and_reap(&mut child));
        assert!(
            !registered(&leader),
            "PV process/unregister: a reaped leader's group is still registered"
        );

        let finished_file = directory.join("finished");
        let quick = script(
            &directory,
            "quick",
            &format!("echo $$ > '{}'", finished_file.display()),
        );
        Command::new(quick)
            .output_within(Duration::from_secs(120))
            .unwrap();
        let finished = pid_from(&finished_file);
        assert!(
            !registered(&finished),
            "PV process/unregister: a finished command's group is still registered"
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
        let descendant_file = directory.join("descendant");
        // The shell exits at once and its background `sleep` keeps the pipes
        // open far past the limit, so the child is already gone when the
        // deadline passes. The limit is long enough for a loaded machine to
        // start and finish the shell.
        let leaves = script(
            &directory,
            "leaves",
            &format!(
                "sleep 120 &\necho $! > '{}'\nexit 0",
                descendant_file.display()
            ),
        );

        let result = within(Duration::from_secs(90), move || {
            Command::new(leaves).output_within(Duration::from_secs(20))
        });
        let descendant = pid_from(&descendant_file);
        Command::new("/bin/kill")
            .args(["-9", &descendant])
            .output()
            .ok();
        let error = result
            .expect("the descendant held the call past its deadline")
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(
            error.to_string().contains("still held open"),
            "the held-open branch was not reached: {error}"
        );
        crate::ops::remove_test_path(directory);
    }
}
