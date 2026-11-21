# DESIGN.md — devtrim site visual world (v6, replaces v5)

v6 (2026-11-21) keeps v5's visual world (dark ground, one lime accent, real
binary frames, zero external fonts/JS deps, hash-only CSP) and applies 2026+
web platform craft for 0.16.0 (plan 020): single-row horizontal tab scrolling
with `@property` scroll-driven edge fades and forced-colors-safe border
underlines (with progressive `anchor-name` sliding underline), `hidden="until-found"`
+ `beforematch` so inactive walkthrough steps stay searchable, a container-queried
`.table-wrapper` for the commands table, `color-scheme: only dark` on `.term`,
sticky translucent top bar with architectural hairlines, a `subgrid`-aligned
pillars strip, and fixed `frames.py` captures (`tui:overview` 54 columns with
category sizes visible, `tui:progress` live mid-apply progress bar).

Kept deliberately: the headline, the pitch, the palette and every token. The
two reference products studied for ideas (fix-storage, poo) contributed ideas
only: open on the user's question, show the product working above the fold, and
state what each label means. No copy, layout, palette or visual came from them.

## Direction
- **The proof is the product.** The pictures are devtrim's own screens,
  rendered in their real colours from the real binary
  (`scripts/site/frames.py`, which maps the TUI's `38;5;0-15` and `30-37/90-97`
  colours to the frame classes); no mock-ups, stock imagery or illustration.
  A screen that `frames.py` cannot capture honestly is left out, never drawn.
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
  Banded sections carry `class="band"`; position never decides a band, so a
  section that is not shown cannot flip the ones after it.
- **Order is shown by position and words**, never by `01/02/03` numbering; a
  real sequence is an `<ol>`.
- **Commands are grouped by what happens**, in the TUI's own words:
  READ-ONLY (see before anything moves), TRASH (moves to the Trash first),
  PERMANENT (a typed command, `trash-empty`, or `--shred`).

## Slots
A slot is a `<!-- tui:NAME -->` / `<!-- /tui:NAME -->` marker pair that
`frames.py` fills. Until it does, CSS (`:has(.term-frame)`) keeps its step or
section out of the page, on screen and on paper, and the walkthrough script
leaves it out of the rail, so the page never shows an empty box or a claim
without its screen. Filling a slot is the whole change: no markup edit follows.
Copy inside a slot describes plan 019's behaviour and is reviewed against the
built binary before the frame is regenerated.

## Landing page
1. **First viewport** (1440×900): the headline "Clean your Mac like you deploy
   code.", the pitch, the install command `brew install mneves75/devtrim/devtrim`
   with a Copy button, a text link to the manual, and the walkthrough with its
   whole first screen. At 390×844 the install command and the first step's
   name sit above the fold; the frame follows.
2. **Walkthrough**, in the order a person meets it: Scan (`tui:scanning`),
   Overview (`tui:overview`), Preview (`tui:results`), Leave one out
   (`tui:select`), Confirm (`tui:confirm`), Apply (`tui:progress`). Every
   walkthrough frame shares one type size (`--term-max: 1rem`, `--cols: 100`)
   and one box, so a narrower capture (`tui:select` and `tui:confirm` are 84
   columns today) keeps the size of the 100-column ones; new captures are 100
   columns.
   - Without script every step shows in order with its caption.
   - With script the captions become a rail of tabs (one `button role="tab"`
     per step, named by the step's name and described by the rest of its
     caption). On ≥ 72rem, where a 100-column frame keeps about 13 px type, the
     rail stands beside the screen, vertical, and only the selected step shows
     its note; narrower, it is a single scrollable row of names with
     scroll-driven fade masks above the screen and the caption shown in the panel.
   - Keys follow the APG tabs pattern as vercel-labs/ui implements it: the
     arrows of the rail's own axis (Up/Down beside the screen, Left/Right
     above it), wrapping at the ends, Home, End, roving tabindex, automatic
     activation; `aria-orientation` follows the layout.
     The selected step carries the TUI's own cursor `›` in the accent, set on
     the tab outside the span that names it, so it is never read aloud.
   - All panels share one grid cell; inactive ones use `hidden="until-found"`
     with a `beforematch` listener (no `inert`) and `visibility: hidden` so
     find-in-page can reveal them while the rail never moves when a shorter
     screen shows.
   - The Apply step (`tui:progress`) is captured by `frames.py` mid-apply
     (waiting for `"1 of 4"`) in permanent mode inside a disposable demo home,
     keeping anything out of the developer's Trash while showing live progress.
3. **Demo video**, secondary, with its transcript; no autoplay.
4. **Find where the space went** (`#analyze`, slot `tui:analyze`): the
   `analyze` screen beside two sentences and the four verdicts as a `<dl>`
   (OFFERED, HOLDS, IN OFFER, PROTECTED, NOT OFFERED), set in ink, not danger colours,
   because a verdict is not a danger level. The frame is captured at 80
   columns so it reads at the column's width without falling to the 11 px floor.
5. **Commands**: one `<table>` (command, what it does in one line, what
   happens) grouped by the written label, coloured and spelled out; every
   command links to its card in the manual (`MANUAL.html#cmd-<name>`). Below
   a 44rem container width it stacks into two-line rows. The `devtrim scan` frame follows it.
6. **Safety**: a short "never touched" list beside the real refusals frame,
   then the release verification commands and a link to SECURITY.md.
7. **Install**: Homebrew, the Apple-silicon download and the source build.

Ids the demo film quotes stay put: `hero-h`, `brew-hero`, `step-preview`,
`step-select`, `step-confirm`, `demo-h`, `commands-h` (`video/film/facts.json`
records each fact with its anchor). Page text changes make that file stale:
regenerate it with `video/film/scripts/facts.py`.

## Manual
Same top bar, tokens and code styling as the landing page; a reading scale
of its own (title `clamp(2.25rem, …, 3.5rem)`, sections up to 2.25rem) so the
docs do not shout at landing-page size; a sticky contents rail with search
(accent caret) and the theme toggle; tables use tabular numerals; command
cards carry `id="cmd-<name>"` and open when a link lands on them. Light mode
is neutral white, never cream. Every existing id stays: other pages and the
README link to them. The version beside the title
(`    <span class="chip g">v…</span>`) and in the footer
(`  <span>devtrim <b>v…</b></span>`) are whole lines that `scripts/release.sh`
matches exactly; keep their markup and indentation.

## Palette
Dark: bg #0a0a0a · surface #121212 · raised #1a1a1a · high #262626 · ink
#efefec · muted #b4b4ae · accent #c8ff38 on ink #101400 · amber #ffc53d ·
red #ff6b57. Light (manual): bg #ffffff · surface #f3f3f3 · ink #141414 ·
muted #4d4d4d · accent #3a6300 · amber #7a4d00 · red #ad2a19.

Measured 2026-11-17 from computed colours in Chrome, every text node against
its composited background (`color-mix` surfaces included), all details open:
no pair below 4.5:1. Resting text nodes only: pseudo-element text (the `$`
prompt, the rail cursor), placeholders and hover, focus and Copied states were
not walked. Lowest: landing 5.87:1 (frame dim grey on #050505);
manual dark 6.68:1 (red label on #121212); manual light 5.22:1 (danger-scale
numeral #111111 on the #e8553f fill), then 5.37:1 (accent command name on the
`:target` card tint).

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
   attributes, none set from script either; elements limited to `ELEMENTS` in
   `scripts/site/csp.py`.
6. Motion is transform and opacity, short, inside
   `prefers-reduced-motion: no-preference`; nothing reveals on scroll; print
   shows every filled step and card and no empty slot.
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
