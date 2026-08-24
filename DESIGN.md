# DESIGN.md — devtrim docs visual world (v2, replaces v1)

## Committed direction (imagegen framework)
- **Theme paradigm:** Deep Dark default — graphite `oklch(16% 0.012 260)`, elegant
  phosphor-green accent; pristine light mode as first-class alternative.
- **Background character:** subtle technical grid (fine 1px lines / dot field),
  never flat-dead, never gradient blobs.
- **Typography:** Swiss rational grotesk system stack (`system-ui` set) with
  strong scale contrast + terminal mono. Tight tracking on display sizes.
- **Hero architecture:** Giant statement masthead — oversized wordmark,
  hairline rules, one-line tagline, version chip. No illustration.
- **Section system:** Swiss grid discipline with a persistent left rail.
- **Narrative spine:** Tool / precision instrument — calibrated meters,
  machined hairlines, honest labels.
- **Signature components (4):** vertical rhythm lines · danger meter strip ·
  product UI panel stack (terminal window mockups) · off-grid editorial notes.
- **Second-read moment:** giant ghost section numerals (01–10) anchoring each
  section's top-left, structural not decorative.

## Palette (dark)
bg oklch(15% 0.012 260) · surface oklch(19% 0.014 260) · ink oklch(93% 0.008 260)
muted oklch(66% 0.014 260) · line oklch(28% 0.014 260)
accent (signal green) oklch(78% 0.17 155) · warn amber oklch(80% 0.14 80) · bad red oklch(68% 0.18 25)

## Palette (light)
bg oklch(97.5% 0.004 260) · surface white · ink oklch(22% 0.02 260)
muted oklch(46% 0.02 260) · line oklch(88% 0.008 260)
accent oklch(50% 0.16 155) · warn oklch(62% 0.14 70) · bad oklch(52% 0.19 25)

## Hard rules from critique (must-fix in v2)
1. Theme toggle must actually override: set `documentElement.style.colorScheme`.
2. All content reflows at 320px: tables become stacked definition lists, `pre`
   gets horizontal scroll within its own box only.
3. Copy buttons: only on single safe commands. Multi-step recipes copy nothing;
   output samples get no button. Danger-carrying blocks show a warning chip.
4. Command cards grouped by task: Inspect → Reclaim → Nuclear.
5. Chevron/rotation transitions inside `prefers-reduced-motion: no-preference`.
6. Filter shows empty-state message + result count.
7. Em-dash density in prose reduced (detector advisory).

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
