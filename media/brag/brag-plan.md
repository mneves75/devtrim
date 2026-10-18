# devtrim — brag plan

**What:** A Rust CLI/TUI that finds what's eating a developer Mac's disk and trims it safely: every action previewed, danger-scored, Trash-first.
**For:** Developers whose Macs fill up with DerivedData, `node_modules`, venvs, Pods, model caches.
**Different:** It won't guess. Closed, corroborated categories; exact previewed plans; Trash first; things like Docker volumes and Xcode Archives are never touched.
**Most impressive real claims (CHANGELOG 0.10.0):** built after a Mac hit 97% full; the scan report went from about 1,660 lines to 53; `purge` puts stale `node_modules` and build artifacts into one plan; Space leaves anything out.
**Visual hook:** a disk meter filling to 97%, snapping into the wordmark.
**Real UI shown:** the 0.10.0 TUI purge screen, selection and confirmation, captured from the real binary in a PTY against a disposable HOME with planted stale repos (`source/capture.py`). The only change: the scratch HOME prefix is displayed as `/Users/you`.
**Tone:** `default` leaning `polished` — precise instrument, confident, no jokes forced.
**Share line:** "devtrim purge: every stale node_modules and build folder in one plan, previewed, danger-scored, Trash first."

## Identity
Landing palette: bg `#0a0a0a`, ink `#f4f1e9`, accent `#c8ff38`, amber `#ffc53d`, red `#ff6b57`, cyan `#7fd7d0`, dim `#7c7a74`, 1px technical grid. System grotesk + SF Mono / Menlo.

## Storyboard (30 fps, 120 BPM — 1 beat = 15 frames)
| # | Frames | Scene | On screen | Sound |
|---|---|---|---|---|
| 1 Hook | 0–75 (2.5s) | Disk meter | Meter fills 61%→97%, number counts, bar goes amber→red. "Disk almost full. Again." | Filtered riser, tick on 97 |
| 2 Reveal | 75–165 (3s) | Wordmark | Bar collapses into `devtrim▍`. "Measure. Classify. Trim. Safely." | Sub drop, kick enters |
| 3 Purge | 165–345 (6s) | Real TUI | Terminal rises; 6 findings stagger in with sizes; caption "Every stale node_modules and build folder. One plan." Then `j`, `Space` keycaps: row 2 → `[ ]`, title 6/6·18.6 GB → 5/6·17.4 GB, amber "Left out of this plan" — caption "Space leaves anything out." | Soft in-key blips per row, key clicks |
| 4 Confirm | 345–450 (3.5s) | Real modal | "Confirm exact plan" pops over: DATA-LOSS WARNING, 5 of 6 selected · 1 left out, Danger-7. Caption "Nothing moves until you approve the exact plan." | Two-note chime |
| 5 Stat | 450–540 (3s) | Scan summary | "1,660 lines" rolls down to "53" — "The scan report on a 97%-full Mac." | Counter ticks, whoosh |
| 6 Outro | 540–645 (3.5s) | CTA | `devtrim` + `brew install mneves75/devtrim/devtrim` + "Free and open source · Apache-2.0 · macOS" | Final chord rings out |

Total 645 frames = 21.5s.

## Caveat
`media/brag.mp4` and `media/brag.jpg` are historical footage: rendered on 2026-09-26 from the 0.10.0 TUI, and not re-rendered for any later release, so a newer TUI can differ from what they show. `purge` has shipped since 0.10.0, so `brew install` gives a build that has it.

## Rebuild
Run every command from the repository root. Each tool set installs into its own folder from its own lockfile, never `video/`'s. `uv run --project media/brag/source --locked` builds `media/brag/source/.venv` from `media/brag/source/uv.lock`. `--locked` refuses to run when the lock no longer matches `pyproject.toml`. The lock pins `pyte`, `numpy` and `scipy` with hashes and needs Python 3.12 or later. Real screen capture and audio generation were checked with Python 3.12; audio was also checked with 3.15. npm follows the existing `video/` package-lock convention: `npm --prefix media/brag/source/remotion ci --ignore-scripts` installs exactly that folder's `package-lock.json`, using the same Remotion, React and TypeScript versions. esbuild's install script is the only one, and it is not needed.
1. Screens: build `target/debug/devtrim` (`rustup run 1.98.1 cargo build --locked`), then run `uv run --project media/brag/source --locked python media/brag/source/capture.py target/debug/devtrim`. It runs the real TUI in a PTY against a disposable HOME with planted stale repos, and applies nothing. Output lands in `media/brag/source/screens/`; the composition carries the captured text.
2. Audio: create the output folder with `mkdir -p media/brag/source/remotion/public`, then run `uv run --project media/brag/source --locked python media/brag/source/audio.py raw.wav`, then `ffmpeg -i raw.wav -af loudnorm=I=-16:TP=-1.5:LRA=9 -ar 48000 media/brag/source/remotion/public/mix.wav`.
3. Video: `npm --prefix media/brag/source/remotion ci --ignore-scripts`, check with `npm --prefix media/brag/source/remotion run typecheck`, then render from that folder: `cd media/brag/source/remotion && npx remotion render src/index.ts Brag out.mp4 --crf=16`.
4. Poster: take frame 330 as `brag.jpg` and overlay it on frame 0 only, so the duration and audio sync stay unchanged.
