# Video tooling

Use the repository's existing npm/package-lock workflow here: `npm ci --strict-allow-scripts`.
`npm run lint` keeps Remotion's ESLint rules and checks the same three source files with
exact native TypeScript7.0.2 through `typescript-native`. `tsconfig.json` declares
ES2018 library types to match its existing ES2018 target and the source's use of
`String.padStart`. It keeps the existing exclusion of `remotion.config.ts`.

Classic TypeScript5.9.3 remains under `typescript` for Remotion's typed ESLint rules
and its SVGR/cosmiconfig TypeScript config loader. Remove that boundary only after
those consumers support the native API and valid/invalid lint and synchronous/
asynchronous config-loader controls pass. `npm run format:check` and `npm run build`
remain the other documented video gates; a source check does not render or publish a film.

## The demo film

`film/` holds the pipeline for the landing page's demo film; `src/film/` holds
its scenes. `film/brief.txt` records every decision and why. Build and check it
from this folder with:

```sh
python3 -B film/scripts/build.py          # full size; --draft renders at half size
```

It writes `out/film/film.mp4` (H.264 + AAC), `film-muted.mp4`, `music.wav`,
contact sheets every 0.2 s and a 15-frame thumbnail sheet, and stops at the
first failing check. The scripts use only the Python standard library and
ffmpeg:

| Script | Does |
|---|---|
| `facts.py` | regenerates `facts.json` from `../index.html` and `../MANUAL.html` |
| `check_facts.py` | every `copy.json` line is a verbatim substring of its cited fact |
| `check_contrast.py` | at most six colours; every text pair in `theme.json` at least 4.5:1 |
| `music.py` | synthesizes music and effects from `timing.json` (fixed seed) |
| `loudness.py` | two-pass loudnorm to -14 LUFS, true peak at most -1 dBTP, then measures |
| `check_media.py` | size, frame rate, duration, H.264 yuv420p, AAC 48 kHz, +faststart |
| `check_sync.py` | cuts on the beat grid with a swish; every kick, click and hit within one frame |
| `contact.py` | tiles frames into timecoded contact sheets |

`film/ledger.txt` keeps the before/after numbers of every review round.
