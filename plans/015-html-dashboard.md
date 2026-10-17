# Plan 015: Complete the offline disk dashboard and verified release candidate

Planned at `ef066b6f11ee1879af98ba226e3084b6d50c0e70`, 2026-10-17.

## What the review changed

The rendered prototype exposed an operational problem: the chart began at
y=1065 on a 390px-wide screen. Replace the marketing headline and oversized
cards with compact data hierarchy. Add one nested overview level, retaining
drilldown for the full retained hierarchy. Source review also found missing
planted proof for the new refusal assertions; close that verification gap
before acceptance. Preserve startup root warnings in the HTML diagnostics.

The final fixed-commit adversarial review found P2 at
`src/dashboard.rs:298`: copied preview commands discard explicit `--root`
overrides. Preserve those roots only for categories that scan roots, safely
quote them as inert shell arguments, and execute the copied preview in a
regression fixture outside the default roots. Control characters must survive
the command field; never turn a lossy path display into executable scope.
This correction and the original audit's report-only `leftovers --apply` hint
are implemented. The first implementation launch exhausted its admission wait;
a later normal slot succeeded without overriding the host guard. The corrected
full suite and MSRV 1.88 suite each pass 312 tests, and a new
compiling injection mutant fails its designated assertion. Final independent
acceptance and the complete release gates remain blocked or pending. The
corrected UI was re-exercised at all four widths. A fresh independent acceptance
launch could not obtain an agent slot after 15 minutes; no verifier ran in that
attempt. Heavy fuzz checks require disk headroom or the owner's explicit
low-disk approval. The existing 50ms page-load failure has no new waiver.

## Objective and frozen acceptance

Developers can run `devtrim dashboard [PATH]` to understand local disk usage,
starting at HOME by default. A private self-contained HTML file opens in the
default browser. `--output FILE` creates a new chosen file; `--no-open` saves
without opening. Existing files and symlink leaves are never overwritten.
The map, breadcrumbs, details, search and list work by pointer and keyboard
at desktop/mobile/320px widths. Real available volume space is distinct from
estimated logical inventory lengths. Existing category previews keep their
own cleanup roots; the report grants no deletion authority. Copied commands
are fresh previews. Unreadable inventory/category/volume inputs are disclosed
and return nonzero while retaining a useful report. Hostile names remain data.

## First-principles decision and alternatives

Chosen: immutable HTML snapshot with compiled static CSS/JS, bounded retained
detail, one filesystem measurement traversal and existing category scan owners.
This costs a refresh command but has no server lifecycle, API authentication,
new dependencies or browser deletion surface to maintain five years from now.

Alternative: localhost service with live updates. It would improve refresh but
adds process/port lifecycle and a persistent authority boundary without a
confirmed need. Premise challenge: keep only the existing `analyze` TUI. That
is simpler but does not deliver the requested graphical hierarchy/shareable
offline artifact. Reject both for this feature.

The visual decision follows overview → zoom/filter → details, rather than
fashion-driven decoration. Primary evidence: [Heer and Shneiderman](https://www.cs.umd.edu/users/ben/papers/Heer2012Interactive.pdf).
The responsive checks follow [W3C Reflow](https://www.w3.org/WAI/WCAG22/Understanding/reflow.html)
and the equivalent-control exception in [Target Size](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html):
tiny proportional map cells retain equivalent full-size list controls.
These references inform the design; they do not certify WCAG conformance.

## Ownership and current code

- Implementation lane: `src/dashboard.rs`, `src/dashboard/*`,
  `tests/dashboard.rs`, minimal `src/{cli,app,lib,safety}.rs` wiring and
  dashboard-only cases in `scripts/tests/planted-violations.py`.
- Main: framing, product/UX, docs/version/release integration and acceptance.
- `src/dashboard.rs::inventory` already retains depth 6 / 10,000 nodes, reserves
  top-level slots, and accumulates omitted bytes; preserve those controls.
- `src/dashboard.rs::recommendations` already calls `ops::scan_all`, filters
  to map-contained filesystem findings and omits unscoped typed commands.
- `Ctx::from_cli` now skips journal rotation only for Dashboard; preserve all
  configuration validation and every other command's behavior.
- Match `tests/dashboard.rs` real-binary isolated HOME/PATH fixtures and the
  existing planted-violations integration-target mechanism.

No new dependency, deletion authority, cleanup-name list, async runtime,
user-HOME mutation, broad cache cleanup or unrelated redesign is permitted.
Preserve others' edits. Resume the same implementation lane for corrections.

## Ordered implementation and proof

1. Keep the eight passing CLI cases; add a boundary-reaching regression for
   startup warnings and tagged overwrite/symlink/mutation-flag assertions.
   Show the meaningful regression fail before its fix.
2. Compact HTML/CSS and nested map rendering. Measure layout in actual CSS
   pixels, bound tiles, stop nested activation bubbling, preserve byte areas,
   accessible names/focus and list equivalents. Separate partial-map status
   from category failures; keep full errors in diagnostics.
3. Add focused planted cases that compile and fail only the designated
   observable assertion when a refusal is deliberately permitted.
   Run `python3 scripts/tests/planted-violations.py dashboard/`.
   All six cases passed locally. The root-command correction must also prove
   that spaces, apostrophes, control characters and command-substitution text
   remain data when the copied preview runs. See the primary
   [shell quoting specification](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html).
4. With the 1.98.1 compiler bin first on PATH and the task-local Cargo home,
   run format, structural tests/scan, all-target Clippy, full Rust tests,
   real PTY/read-only suites and `node --check src/dashboard/app.js`.
   Expected: zero failures; report actual Clippy driver version separately.
5. Generate fixture-derived HTML with the actual binary. Run the isolated
   CDP browser harness at 1280/640/390/320px; inspect screenshots and exercise
   pointer/keyboard drilldown, breadcrumbs/up, search/sort, copy intent and
   denied-clipboard fallback. Assert no external requests, runtime/CSP errors
   or horizontal overflow. Inspect empty/partial/hostile-name states too.
6. Fresh source-blind `mneves-verify` acceptance plus full review of fixes.
   No more than two review/fix cycles; one acceptance correction/reverify.
   A usage-limit failure is BLOCKED, never independent PASS.
7. Finish affected docs, version/changelog using the existing release script
   convention; explicit task-owned staging, Conventional Commit and push.
   Latest owner message authorizes merging after required gates pass.
8. Before any beta/public release trigger, run `mneves-security` on the exact
   candidate commit and complete required release gates. Check local/remote
   tags and current default head. The repository accepts `vX.Y.Z-betaN`, not
   a reused tag. Verify immutable assets and the actual served documentation.
   Present concrete version/commit/target/evidence before any production step
   requiring final approval; no credential changes or gate waivers.

## Done / stop conditions

All criteria above must be supported by observed results. Do not equate a
build, HTTP 200 or release receipt with the user's visible outcome. Preserve
review/release evidence; remove only task-created disposable files/processes
with exact ownership. Do not remove active/uncertain scratch or other sessions'
artifacts. Stop the gated action for missing quota, protected-action approval,
failed security/release gates, under-20GB bulky-output policy or unresolved
blocking review findings; continue independent authorized work.

Five-year maintenance priorities: immutable snapshot schema, clear partial
states, no hidden network service or widening of cleanup roots, and real
interface tests. Do not add frameworks or live mutation merely to modernize.
