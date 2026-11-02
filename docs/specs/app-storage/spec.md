# Read-only application storage inventory

Status: implementation authorized by the owner on 2026-11-02.
Baseline: a35f767950e592539640a578a1458fc30578f785.

## Outcome

`devtrim app-storage` measures known WhatsApp storage without reading messages
or creating cleanup authority. Explicit `--root` paths additionally discover saved
Screen Studio projects. This is a disk inventory, not a cleanup preview.

## Acceptance

1. Four observed macOS WhatsApp Group Containers are the default known scope:
   `group.net.whatsapp.WhatsApp.shared`, `group.net.whatsapp.WhatsApp.private`,
   `group.net.whatsapp.WhatsAppSMB.shared`, `group.net.whatsapp.family`.
   Unknown layouts are not inferred. Separate Message/Media, direct ChatStorage
   database/WAL/SHM files and remaining regular-file blocks without overlap.
2. No message, media or database contents are opened. Missing containers are
   absent; unreadable or malformed inputs are errors. No orphan/duplicate-media
   identification or disposal recommendation is made.
3. Screen Studio discovery requires explicit `--root` paths. Configured/default
   project roots and the whole home folder are not searched implicitly. Saved
   `.screenstudio` directories are source projects, never actionable cache.
4. Root symlinks/ancestors are refused before canonicalization in the report.
   Both inventories follow no links or other devices. Screen Studio discovery
   and bundle measurement exclude `.git`,
   `node_modules`, `target`, `.build`, `.next`, `.turbo`, `.venv`, `venv`, `.cache`,
   `.app`, `.framework`, `.photoslibrary` and `.xcarchive` trees, including an
   explicit root inside one. WhatsApp measures regular files inside its known
   containers without interpreting media directory names as project caches.
   Scope and known skips remain visible.
5. Use allocated regular-file blocks, count each physical file once across the report,
   including hard links, root aliases and overlapping application views;
   disclose APFS clone limitations and live-snapshot drift. Detail is limited to
   100 bundles; omitted measured bytes remain in an aggregate. Retained errors
   are bounded and further errors counted. Partial reads return nonzero.
6. All findings use Action::Info. Reject --apply, --yes, --yolo and --shred.
   JSON is exactly one existing report document; errors name app-storage.
   Human filenames remain inert. No cleanup registry/TUI, journal, subprocess,
   deletion boundary or dependency is added.
7. Real-binary tests use disposable HOME/PATH and preserve the fixture tree,
   covering ordinary measurements, sparse/hard-linked files, explicit/overlapping
   roots, missing/invalid/linked inputs, unreadable children, pruning, detail
   limits, mutation-flag refusal and hostile names. Keep positive controls.
8. README, manual, changelog and security documentation describe the resulting
   contract. AGENTS/CLAUDE stay identical; film facts remain synchronized with
   page text. Final acceptance includes relevant repository gates and independent
   adversarial verification before any release.

## Decisions and exclusions

One CLI report reuses the existing informational finding/envelope and DiskUsage;
it owns no mutation capability. The smaller rejected alternative was documenting
only existing analyze/largest commands; the owner chose implementation.

Existing project/Docker authorities are unchanged. A lockfile or timestamp alone
cannot prove edited node_modules reconstructible, so the comparison with Mole is
not itself a reproducible defect or authority for an unproven new cleanup rule.
OneDrive sync eviction, arbitrary Group Containers deletion, direct VM-disk
manipulation and a website/film redesign are outside this feature.

Screen Studio's vendor documents saved projects and configurable locations:
https://screen.studio/guide/saving-your-project
The observed WhatsApp directory names are not a vendor-guaranteed database schema.
