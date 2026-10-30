# DESIGN.md — devtrim site visual world (v3, replaces v2)

v3 (2026-10-30) makes the landing page and the manual one world and drops v2's
decorative devices — ghost section numerals, a background grid, hairline
rules, a tagline and monospace labels — after an impeccable and taste review
found them generic and against the owner's rules.

## Direction
- **The proof is the product.** The pictures are devtrim's own screens,
  rendered from the real binary (`scripts/site/frames.py`); no mock-ups, no
  stock imagery, no illustration.
- **Theme:** near-black ground, one signal accent; the manual also has a
  first-class light mode that follows the OS until the visitor chooses.
- **Type:** the platform sans for everything readable; the monospace face only
  for real code, commands, terminal frames and keys. Strong scale contrast,
  tight tracking on display sizes, headlines of two lines or fewer.
- **Surfaces:** separated by a step in tone, never a line or a shadow; real
  tables and form fields may keep their lines. Near-square corners
  (`--radius: 0.25rem`), flat buttons, no gradients, no glows.
- **Order is shown by position and words**, never by `01/02/03` numbering; a
  real sequence is an `<ol>`.
- **Commands are grouped by what happens**, in the TUI's own words:
  READ-ONLY (see before anything moves), TRASH (moves to the Trash first),
  PERMANENT (a typed command or `--shred`).

## Palette (dark, both pages)
bg #0a0a0a · surface #121212 · raised #1a1a1a · ink #efefec · muted #b4b4ae
(secondary notes only; running text is full ink) · accent #c8ff38 on ink
#101400 · amber #ffc53d · red #ff6b57

## Palette (light, manual)
Neutral white ground, ink near #141414, an accent dark enough for AA on white,
and AA amber/red variants. Never a cream or warm off-white ground.

## Rules
1. Danger semantics: green = safe, amber = think, red = irreversible, and each
   meaning is also written (READ-ONLY, TRASH, PERMANENT).
2. All content reflows at 320 px; terminal frames keep an 11 px floor and
   scroll inside their own box; tables stack into labelled rows.
3. Copy buttons only on single safe commands; multi-step recipes copy nothing.
4. Inline scripts and styles are admitted by CSP hash only; no `style`
   attributes (`scripts/site/csp.py`).
5. Motion is transform and opacity, with a `prefers-reduced-motion` fallback
   and a print style that shows everything.
6. Targets at least 24×24 px, visible focus rings, AA contrast.

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
