---
name: verify-apply-stop
description: Check devtrim's apply stop protocol (journal::admit, begin, finish, ApplyMonitor) with its bounded model. Use when changing src/journal.rs's stop handling, adding an attempt that writes before journal::begin, or reviewing either.
---

# Verify the apply stop protocol

Run:

```bash
rustup run 1.98.1 cargo test --locked --lib journal::tests::stop_model -- --nocapture
python3 -B scripts/tests/planted-violations.py journal-stop/
```

The model in `src/journal.rs` (`mod stop_model`) drives the real `admit`,
`begin` and `finish` against a real journal. It covers every plan of one to three
attempts drawn from plain, uv (admit, lock write, begin) and uv failing before
begin. The stop lands before each step in turn: 345 runs, pinned by an assertion.

It fails when any of these happen:

- An attempt writes uv's lock and is then refused as stopped, leaving no journal
  record. This is incident D-07, fixed in cf7962b.
- An attempt begins after the stop without an admission granted before it.
- More than one attempt begins after the stop.
- The monitor's stopped or finished counts, or the journal's line count,
  disagree with the plan.

The negative control replays cf7962b's parent order (`refuse_if_stopped`, then
the lock, then `begin`). The model must find that counterexample and print it as
`D-07 replayed:`. If the replay passes, the model can no longer see the bug.

## When you add an attempt that writes before `begin`

Take `journal::admit` before the write, and hold the `Admission` until `begin`.
Then add the new shape to `Attempt` and `steps()`, recount the pinned total, and
rerun both commands.

## Limits

- The stop is placed only between protocol steps. Inside `admit` and `begin` the
  monitor is plain atomics, and a stop landing mid-step acts at the next check.
- One admission per `begin`. The model never runs two begins under one
  admission, because no caller does; `journal-stop/one-admission` covers that.
- A preparation that fails (uv's lock held) leaves its lock unrecorded and is
  reported as an error, not as stopped. The model allows exactly that.
- The model checks the protocol, not its callers: the uv steps are written into
  `steps()` by hand. That `caches.rs` takes the admission before writing the
  lock is guarded by `journal-stop/uv-lock`
  (`a_stopped_apply_creates_nothing_in_the_uv_cache`), not by the model.
