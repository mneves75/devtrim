# 016 — Gaps against diskpilot that fit this charter

Date: 2026-11-06. Base: devtrim `798adf97997c89dca41ad81dff2eac77a04a4b59`
(v0.12.6). Comparison, read only: diskpilot
`7dfb588d5570fd3877f7cc62717e4599c4c04421` (HEAD when this plan was written).
No diskpilot source is vendored.

## What was compared

diskpilot modules under `src-tauri/src/`: `duplicates.rs`, `secrets.rs`,
`apps.rs`, `cleanup/`, `scanner/`, `devtools/`, `docker/`, `trash.rs`,
`explorer.rs`, `assistant/`, `tray.rs`, and ADRs `docs/adr/0001`–`0007`.

devtrim owners: `src/ops/` (one `Op` per cleanup category), `src/safety.rs`,
`src/uninstall.rs`, `src/largest.rs`, `src/analyze.rs`, `src/app_storage.rs`,
`src/journal.rs`, `src/tui.rs`.

## Matrix

| diskpilot | Where | devtrim before | Decision |
| --- | --- | --- | --- |
| Content-hash duplicates, keep-one, Trash, byte recheck, hard-link collapse, APFS clone flag | `src-tauri/src/duplicates.rs` | No content-hash grouping. `largest` is report-only. Hard links are already counted once in `safety::DiskUsage`. | **Ship** `devtrim duplicates`, outside `scan` and the TUI. Clone IDs deferred (below). |
| Credential shapes in Claude / Codex / Gemini transcripts, masked | `src-tauri/src/secrets.rs`, `src/features/developer/TranscriptSecrets.tsx` | Agents history can be deleted by age. Nothing scans transcript text, and nothing should mutate it for this. | **Ship** `devtrim secrets`, read-only. |
| App footprint and leftover Containers / Group Containers for bundle-like names with no installed app | `src-tauri/src/apps.rs` `app_storage` | `uninstall` reports paths named for one installed app's exact bundle id and refuses group containers. `app-storage` is four WhatsApp containers plus explicit Screen Studio roots. | **Defer.** Group-container suffix rules mis-attribute (the reason `uninstall` omits them). No mutation authority covers `~/Library`. |
| Trash-first cleanup, planner, history | `cleanup/`, `trash.rs`, ADR 0005 | Already the deletion sink, journal, and `trash-empty`. | No gap to copy. |
| Docker via CLI | `docker/`, ADR 0006 | `clean docker`. Volumes stay. | No gap. |
| Large files, explorer | large-files, `explorer.rs` | `largest`, `analyze`. | No gap. |
| Developer tool inventory | `devtools/` | `caches`, `toolchains`, `xcode`, `agents`. | No new authority. |
| Menu bar | `tray.rs` | Out of charter. | **Defer.** |
| Optional Anthropic assistant | `assistant/`, ADR 0007 | Would send local paths off the machine. | **Defer.** Offline wins. |
| Full Disk Access coaching | GUI | Not a CLI surface. | **Defer.** |

## Clone IDs

diskpilot reads `ATTR_CMNEXT_CLONEID` with `getattrlist` (`duplicates.rs`,
unsafe `libc`). devtrim forbids `unsafe_code`. `stat` does not expose a clone
id, and allocated blocks still count a clone in full (already documented for
`DiskUsage`). `duplicates` collapses hard links by device and inode, and every
finding says the size can overstate what Trash frees. Adding unsafe code, or a
new crate whose only job is that syscall, was rejected.

## What shipped

- `src/ops/duplicates.rs`, command `duplicates`, not in `ops::all()`.
- `src/secrets.rs`, command `secrets`.
- Keeper path and preview SHA-256 live on `Finding` (`DuplicateProof`), not in
  parsed display text.
- Tests: unit tests in both modules, and `tests/duplicates.rs` /
  `tests/secrets.rs` through the real binary. The flag eval in
  `tests/eval_system.rs` rejects `duplicates --shred` and `secrets --apply`.

## Deferred, and why

- Orphan `~/Library` containers. Informational only if built, and a wrong
  bundle id in a report is worse than a missing row. Needs its own spec.
- APFS clone IDs, until there is a safe API.
- Putting duplicates inside `scan` or the TUI. Hashing every project tree on
  a routine preview is the wrong default.
- `--shred` for duplicates. Permanent deletion of a path that still matters
  is `trash-empty` after the Trash plan, which already has its own
  confirmation.
