# DESIGN.md — devtrim site visual world (v4, replaces v3)

v4 (2026-11-11) keeps v3's one world and fixes what the rendered page got
wrong: monochrome terminal frames, a small hero frame beside a dead column, no
install line above the fold, broken box lines, commands as prose walls, and a
manual that looked like another product.

## Direction
- **The proof is the product.** The pictures are devtrim's own screens,
  rendered in their real colours from the real binary
  (`scripts/site/frames.py`, which maps the TUI's `38;5;0-15` and `30-37/90-97`
  colours to the frame classes); no mock-ups, stock imagery or illustration.
- **One world.** Dark near-black ground, one lime accent, the platform sans
  for reading and the monospace face only for code, commands, keys and
  terminal frames. No gradients, glows, glass, decorative shadows, pill
  buttons, bento grids, numbered or monospace eyebrow labels, cream or
  off-white grounds, italic accent words, logo walls, counters or testimonials.
- **One set of tokens.** `styles.css` holds the tokens, the top bar, the type
  scale, code and the terminal frames. `MANUAL.html` keeps a single inline
  style admitted by hash (its CSP and `scripts/tests/csp-controls.py` expect
  one): its first block is a verbatim copy of the shared top of `styles.css`
  and must change with it. Tokens are `light-dark()` pairs; the landing page
  is dark only, and the manual adds light mode by switching `color-scheme`.
- **Surfaces** separate by a step in tone, never a line or a shadow; real
  tables keep their row lines. Near-square corners (`--radius: 0.25rem`).
- **Order is shown by position and words**, never by `01/02/03` numbering; a
  real sequence is an `<ol>`.
- **Commands are grouped by what happens**, in the TUI's own words:
  READ-ONLY (see before anything moves), TRASH (moves to the Trash first),
  PERMANENT (a typed command, `trash-empty`, or `--shred`).

## Landing page
1. **First viewport** (1440×900 and 390×844): the headline "Clean your Mac
   like you deploy code.", the two-sentence pitch, the install command
   `brew install mneves75/devtrim/devtrim` with a Copy button, and a text link
   to the manual. The walkthrough starts below them at full container width;
   no column is left empty.
2. **Walkthrough** of the real frames, `tui:results`, `tui:select` and
   `tui:confirm`: Preview, Leave one out, Confirm. It is a tablist (arrow keys,
   Home and End, roving focus) only once its script runs; without script every
   step shows in order with its caption. There is no outcome step: an apply
   moves files to the developer's own Trash whatever HOME says, so
   `frames.py` never applies a plan.
3. **Demo video** below it, secondary, with its transcript; no autoplay.
4. **Commands**: one `<table>` (command, what it does in one line, what
   happens) grouped by the written label, coloured and spelled out; every
   command links to its card in the manual (`MANUAL.html#cmd-<name>`). Below
   48rem it stacks into two-line rows. The `devtrim scan` frame follows it.
5. **Safety**: a short "never touched" list beside the real refusals frame,
   then the release verification commands and a link to SECURITY.md.
6. **Install**: Homebrew, the Apple-silicon download and the source build.

## Manual
Same top bar, tokens, type scale and code styling as the landing page; a
sticky contents rail with search and the theme toggle; command cards carry
`id="cmd-<name>"` and open when a link lands on them. Light mode is neutral
white, never cream. Every existing id stays: other pages and the README link
to them. The version beside the title (`    <span class="chip g">v…</span>`)
and in the footer (`  <span>devtrim <b>v…</b></span>`) are whole lines that
`scripts/release.sh` matches exactly; keep their markup and indentation.

## Palette
Dark: bg #0a0a0a · surface #121212 · raised #1a1a1a · high #262626 · ink
#efefec · muted #b4b4ae · accent #c8ff38 on ink #101400 · amber #ffc53d ·
red #ff6b57. Light (manual): bg #ffffff · surface #f3f3f3 · ink #141414 ·
muted #4d4d4d · accent #3a6300 · amber #7a4d00 · red #ad2a19. All text pairs
measure at least 4.5:1 (lowest: light accent on #e9e9e9, 5.83:1).

Terminal frames stay dark in both themes on #050505: ink #d9d9d4 14.4:1, red
7.3:1, amber 12.9:1, green 17.3:1, cyan 11.8:1, blue 8.8:1, magenta 9.2:1,
dim grey `k` #8a8a84 5.9:1.

## Rules
1. Danger semantics: green = safe, amber = think, red = irreversible, and each
   meaning is also written (READ-ONLY, TRASH, PERMANENT).
2. Terminal frames: unitless line-height 1.18 so box drawing joins, the
   monospace stack led by `ui-monospace` and `SF Mono`, no letter-spacing, one
   font size per frame (walkthrough frames share one size), an 11 px floor,
   and a frame wider than its box scrolls inside it, focusable and named.
3. All content reflows at 320 px with no sideways page scroll; headlines take
   two lines or fewer.
4. Copy buttons only on single safe commands; multi-step recipes copy nothing.
5. Inline scripts and styles are admitted by CSP hash only; no `style`
   attributes; elements limited to `ELEMENTS` in `scripts/site/csp.py`.
6. Motion is transform and opacity, short, inside
   `prefers-reduced-motion: no-preference`; nothing reveals on scroll; print
   shows every step and card.
7. Targets at least 24×24 px (controls 44 px tall), visible focus rings, AA
   contrast, hover only where hover exists.

## Offline disk dashboard

The generated dashboard is an Operate surface with an explicitly requested
light disk-map design. Use white panels, cool neutral ground and system sans
text. Compact summary metrics lead directly to the map, with category preview
rows alongside on desktop and below on narrow screens. The map owns the first
viewport; avoid marketing headlines, hero whitespace and decorative animation.

Parent headers and one nested map level show hierarchy; drilldown, breadcrumbs,
search and the paginated list expose retained detail. Layout uses actual CSS
pixels so visible labels stay legible at narrow widths. Small tiles keep
equivalent list controls. Category color never implies an ancestor is removable.
The output is an immutable offline snapshot with text-only path rendering,
keyboard focus and useful empty/partial/error states. Logical sizes and actual
filesystem available space stay separate. There is no apply control.

## Terminal interface
- Preserve the same semantic palette: green = preview/read-only, amber = review,
  red = permanent. Every meaning also appears as text (`READ-ONLY`, `TRASH`,
  `SHRED`, `PERMANENT`) so color is never the only signal.
- Minimum viewport is 64×18. Smaller terminals show a resize instruction rather
  than clipping controls or confirmation copy, and accept only quit input.
- Arrow keys and `j`/`k` are equivalent; direct number keys open operations;
  `Esc` always cancels a confirmation.
- The exact preview comes first. The following confirmation state repeats the
  data-loss warning and exact requested acknowledgment before apply.
- Results, errors, and outcomes use the same arrow/Vim scrolling; scanner
  diagnostics remain in the alternate-screen state instead of leaking to stderr.
