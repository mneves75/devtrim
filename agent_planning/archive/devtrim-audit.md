# devtrim audit and implementation plan

Status: all accepted findings implemented and verified for source version 0.8.1. All delivery gates pass, and the confirming GPT-6 Astra P3 autoreview is scoped-clean for every changed source. The generated video passed separate independent artifact verification. The user authorized takeover of the existing edits, OpenAI review transfer, and source commit/push; no production release is part of this task.

Scope: remove unnecessary code, deliver measured performance improvements, fix confirmed security defects, improve agent instructions and verification, update the source version and documentation, commit and push. A production release or tag is not part of this request.

Baseline: `fc934ec` on `main`, plus pre-existing uncommitted Rust, test, video, and performance-script changes. The user explicitly approved integrating those edits after being told another session had the checkout open. The inherited changes are inputs to review, not original work performed by this audit.

## Acceptance criteria

- A real binary removes eligible cache data while preserving authentication state and unrelated files.
- Every retained optimization preserves JSON, errors, cancellation, authorization, and fresh apply checks; measured workloads and counterexamples accompany performance claims.
- Existing unnecessary wrappers and scaffolding are removed without deleting meaningful behavioral or security tests.
- Local, CI, and release verification agree about the gates they run; a missing tool, failed process, or non-rendering UI fails visibly.
- AGENTS.md and CLAUDE.md remain byte-identical, with concise task routing and explicit acceptance criteria grounded in current model guidance.
- The version, lockfiles, changelog, README, manual, security documentation, project memory, and explainer describe the verified result.
- Independent standards/spec review and P3 autoreview have no unresolved accepted blockers before committing and pushing the reviewed state.

## Confirmed findings and ordered work

### 1. Preserve Hugging Face authentication state — P1

`src/ops/caches.rs:14` authorizes the whole `.cache/huggingface` directory. This is Hugging Face's default state directory, containing both credentials and cache data. An independent reviewer ran the existing binary against a disposable HOME with synthetic `token` and `hub/model` files: permanent cache cleanup returned success and removed both.

Change the closed built-in cache target to `.cache/huggingface/hub`. Do not treat arbitrary HF_HOME overrides or the parent directory as cleanup authority. Scanner and apply authorization already use the same closed list. Keep other state directories outside scope unless separately corroborated.

Proof: add a real-binary regression in the existing CLI harness. First reproduce failure; then require model data to disappear while `token`, `stored_tokens`, and an unrelated parent sentinel retain their bytes. Cover preview shape and refused parent authority in existing cache tests. Re-run cache and shared deletion-boundary tests.

### 2. Correct the release compiler pin — P1

The repository and hosted workflows pin Rust 1.98.0. The Rust release team's 1.98.1 advisory describes a vtable-generation miscompilation in 1.98.0 that can emit a null function pointer. This is a toolchain defect; no claim is made that the devtrim binary has triggered it.

Update each current toolchain reference individually to 1.98.1, keeping historical changelog entries intact and MSRV at 1.88.0. Build and test with the actual pinned toolchain, not whichever standalone Cargo appears first on PATH. Run the MSRV suite separately.

### 3. Repair performance evidence before optimizing — P1

The existing untracked `scripts/perf/` helpers have four confirmed design defects and two additional robustness gaps:

- Output names depend on the binary basename; two binaries named `devtrim` overwrite the first ordering's results.
- Real-PATH timing accepts failures without proving equivalent work, so a faster refusal can appear as an improvement.
- Corpus construction splits whitespace-containing paths through `xargs`.
- Stub source interpolates the corpus path without shell escaping.
- Hyperfine command strings need quoting for binary paths containing spaces.
- Load is recorded only around the whole run, and binary identity/build settings are not persisted.

Use explicit ordering IDs, runtime HOME in literal stubs, NUL-safe paths, and correctly quoted command arguments. Compare successful equivalent workloads before timing. Keep failed-probe timing separate, with matched outcomes. Record binary hashes, compiler/build mode, corpus counts, warmup, trial order, exit status, and load for each ordering. Validate the harness with same-basename binaries, spaces in paths, a deliberately wrong-output binary, and a deliberately failing binary.

The Hugging Face repair intentionally changes scan paths and sizes. Build the performance baseline after that security correction, then compare it with the optimization candidate using the same compiler and corpus. Update the corpus to contain `huggingface/hub` model data and separate credential sentinels. Do not demand byte equality between intentionally different security contracts or silently normalize away relevant differences.

### 4. Finish scan-scoped probe sharing — P1

`tests/cli.rs:1357` expects one shared build-process observation and one Git observation per repository. Production `node_modules` and `artifacts` scanners still issue separate probes. The actual full Rust test run fails this test with observed count 2, expected 1. Preserve this meaningful test: it proves both categories produce findings before counting external subprocesses.

Share observations only within one read-only scan. Cache failures as failures so both affected categories refuse consistently. Apply must perform new liveness and staleness checks after confirmation, never consume cached preview observations. Prefer a small scan-owned context over process-global state or a general cache framework.

Proof: make the current failing test pass; prove shared failure propagation and fresh apply revalidation. Compare scan output and subprocess counts on stale/recent/active/error fixtures, then run alternating baseline/candidate benchmarks.

### 5. Stop unnecessary terminal redraws — P2

`analyze` and `status --watch` redraw every 80 ms even with unchanged state. `analyze` formats all entries into a Ratatui list on every draw, including offscreen entries. Render after progress, relevant input, or resize; preserve prompt handling and immediate cancellation. Keep the existing polling interval unless measured input latency justifies changing it.

Proof: a real PTY with explicit rows/columns, disposable HOME, and controlled PATH must show initial content, progress, selection, help, resize, quit, and terminal restoration. Measure idle CPU on the same wide-directory corpus before and after. Do not claim lower CPU from reduced output bytes alone: Ratatui already diffs terminal output.

### 6. Accept smaller cuts only when their evidence holds — P2/P3

- Consume analyze JSON entries by value to avoid unnecessary PathBuf clones.
- Inline `project::is_within`, which merely delegates to `Path::starts_with`.
- Remove direct setup-mirroring assertions in the report test while preserving JSON, hidden-authority, and terminal-escaping checks.
- Correct the missing `installers` branch in `clean_operation_from_args`. The actual binary invoked as `clean installers --unsupported-audit-flag --json` returns exit 2 but labels the error envelope's operation `clean`; other recognized targets retain their category. Add the missing category and a JSON-error regression.
- Reuse installer eligibility metadata within one scanner invocation; preserve fresh apply checks and preview identity.
- Compare top-N selection followed by sorting against the current full sort in `largest`; retain deterministic size/path ordering and tie tests. Keep it only if representative measurements justify the extra logic.

Review the inherited removal of redundant wrappers, duplicate geometry, subprocess-result parsing, and unused video scaffold. Re-run video lint, types, formatting, dependency audit and bundle build. Do not claim inherited deletions as new work from this session.

### 7. Improve agent DX and verification loops — P1/P2

Keep a short entry point in AGENTS.md/CLAUDE.md: project map, safe execution boundaries, focused checks by changed surface, full delivery gates, and links to detailed standards/security docs. Do not duplicate the global assistant contract or put API harness implementation details into this Rust application's instructions. Remove irrelevant Apple/Swift workflow text from the Rust-only project guidance.

Document Fable 5.1/Astra task handling: concrete outcome and stopping condition, one owner per writable file, independent reads batched, targeted edits, continued primary work during independent reviews, focused testing before broader checks, exact blockers, and durable state at compaction. Model selection belongs to the harness; verify runtime availability instead of assuming a model label works.

Provide one local verification entry point that reports each executed gate's status and fails on missing prerequisites. Reuse existing gate commands rather than introducing another test framework. Include real PTY QA in ordinary CI, not only releases. Replace the current release PTY sleep-and-Escape check: it asserts exit status but does not assert that any UI rendered or set a terminal size.

Worktree/setup guidance: stay in the current checkout by default; record the baseline and ownership before edits. For explicitly authorized isolated worktrees, use distinct Cargo target/evidence directories and ports. Never share a mutable corpus between runs. Debug through isolated CLI fixtures, stderr, JSON, targeted test output, and optional local profiler access; production data and credentials are unnecessary.

### 8. Review, version, and deliver — P1

After implementation stabilizes, run independent Matt Pocock standards and spec reviews against the pinned baseline and complete current patch, including untracked files. Resolve evidence-backed findings. Run P3 autoreview with the authorized current model and inspect its actual structured result. After accepted fixes, rerun affected proof and the required confirming pass.

Bump the source patch version from 0.8.0 to 0.8.1 unless the integrated change adds a new public capability. Regenerate root and fuzz lockfile package versions. Update current packaged docs and changelog; keep the public landing download on the verified stable artifact until a separate release is authorized. Inspect and explicitly stage the complete approved patch, commit with Conventional Commits, push normally, and verify the remote commit and final checkout state. Do not create a release, tag, or Homebrew publication under a commit-and-push request.

## Ten alternatives considered

| Approach | Decision and reason |
| --- | --- |
| Independent scanners with no sharing | Lowest implementation cost and fresh observations, but retains duplicate subprocesses; use as the performance control. |
| Lazy observation cache inside the general context | Small call-site diff, but hidden lifetime/invalidation can leak stale data into apply; reject unless scope is enforced structurally. |
| Explicit observations owned by one scan | Preferred sharing design: more visible plumbing, clear lifetime and fresh apply boundary; require subprocess and failure-path proof. |
| Pass observations separately to each category scanner | Explicit and testable but adds specialized scan entry points; choose only if it avoids a wider context API. |
| Merge node_modules and artifact directory walks | Potential I/O reduction, but couples category pruning and safety semantics; defer until profiling shows traversal dominates. |
| Run independent system probes concurrently | Can reduce sample latency, but needs bounded process ownership and failure aggregation; defer pending measured latency. |
| Render only when state changes | Preferred UI approach: small state flag, immediate input response, lower idle work; require resize and progress tests. |
| Throttle renders to a lower fixed frequency | Simpler scheduling, but still wastes idle work and can delay progress/input; retain as a benchmark alternative. |
| Maintain a bounded heap for largest directories | O(n log k), bounded ranking storage; more comparator bookkeeping and no reduction to the totals map; compare if ranking dominates. |
| Partial selection followed by sorting the top N | O(n) selection plus O(k log k) ranking using stdlib; prefer over a custom heap only when measured benefit exceeds added code. |

The selected package is the security repair, corrected evidence, explicitly scan-owned observations, state-change rendering, and one installer metadata observation per eligibility check. Partial selection and a custom heap were not adopted: there is no application measurement justifying more ranking logic. This keeps lifetimes visible and avoids a long-lived cache or combined scanner framework that future maintainers must invalidate correctly.

## Delivery gates

Run focused regressions first, then the complete delivery set once the integrated patch is stable:

1. Pinned-toolchain format, strict Clippy, all-target/all-feature tests, and locked arm64 release build; independently execute the MSRV 1.88 suite.
2. Structural-rule fixtures and scan, agent-doc byte equality, shell syntax, ShellCheck, actionlint, release-policy tests, and PTY positive/negative controls.
3. Fresh root and fuzz Cargo audits; Gitleaks runtime synthetic-token positive control and redacted full-history scan; TruffleHog full-history scan with errors fatal.
4. All five existing fuzz targets with the configured nightly, 60 seconds each; retain focused parser/boundary regressions for the actual changes.
5. Video strict clean install, low-severity dependency audit, lint/types, formatting, and bundle build. If rendered media changes, verify the output stream and menu frame as the repository requires.
6. Correctness-controlled A/B measurements, actual terminal interactions for changed surfaces, independent standards/spec review, and P3 autoreview.

Each gate reports its own exit status. A missing prerequisite or excluded test is disclosed; a successful unrelated gate never substitutes for it. Verify the final diff and normal push against the reviewed commit; do not repeatedly monitor after the requested delivery is complete.

Five-year test: preserve explicit ownership, safety capabilities, portable fixtures, and executable proof. Avoid tying repository rules to one model generation's quirks, machine-specific timing thresholds, or a permanent orchestration layer. Future revisions should be able to rerun the evidence without this conversation.

## Verification observed in this audit

- `cargo fmt --all -- --check`: passed.
- `ast-grep test --skip-snapshot-tests`: all four rule suites passed.
- `ast-grep scan --config sgconfig.yml`: passed.
- `actionlint`: passed.
- Fresh `cargo audit --json`: passed with no vulnerabilities in 216 dependencies; the fetched advisory database reports commit `5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5`, updated 2026-09-02. The first cached run did not report freshness, and the first refresh was blocked by the sandbox advisory-cache lock; an approved retry completed.
- Fresh video `npm audit --package-lock-only --audit-level=low --json`: passed with zero vulnerabilities.
- Fuzz lockfile audit against the refreshed database: passed with zero vulnerabilities in 198 dependencies. Gitleaks' synthetic-token control detected its fixture; the full-history scan covered 73 commits and found no leaks.
- The original scan-sharing regression failed with two probes where one was expected. Its expanded success/liveness-failure/Git-failure cases now pass. The integrated Rust 1.98.1 and MSRV 1.88.0 suites each passed 212 unit tests and 56 CLI tests; the one ignored test is a helper subprocess exercised by its parent concurrency test.
- The Hugging Face regression first reproduced token deletion, then passed after narrowing cleanup to `hub`. Installer error-envelope regression similarly failed before the missing branch was restored and passed afterward. Cache authority positive/negative controls and five installer safety tests pass.
- Strict all-target/all-feature Clippy and the locked arm64 release build passed with Rust 1.98.1. Both tracked Cargo lockfiles change only the path package's version to 0.8.1. An offline fuzz-lock regeneration proposed unrelated downgrades; those were discarded and locked metadata/build verification retained the original dependency graph.
- Real-binary PTY menu/help/cancel/quit and terminal restoration passed. A non-rendering executable was rejected. ShellCheck, Bash syntax, actionlint, and release-policy tests passed for the new verification tooling. AGENTS.md and CLAUDE.md are byte-identical; `git diff --check` passed.
- Fresh local verification caught that `bash -n file1 file2`/`sh -n file1 file2` only check the first file. Helper and hosted workflows now loop over individual scripts. All six actual loops passed valid fixtures and rejected a malformed last script with exit 2; shell/workflow/release-policy checks passed again.
- All five nightly fuzz targets ran for 60 seconds each and exited zero. Final TruffleHog full-history scan covered 1,735 chunks / 1,750,483 bytes with zero verified or unverified secrets.
- Video strict clean install, fresh zero-vulnerability audit, lint/types, formatting, bundle, and final render passed. The first install was refused at the sandboxed npm cache; an isolated writable cache succeeded. Chromium required local process permission for rendering. Visual review caught and corrected a stale demo version, missing Installers menu item, and understated danger scores. The final video is one H.264 stream, 1920×1080, 12 seconds, without audio.
- The manual's cache disclosure was inspected at 1440×1000 and 390×844 with no document overflow. Independent security review verified every inline script against the HTML CSP hashes; release policy and Homebrew formula positive/negative tests pass.
- The new read-only PTY gate exposed a harness synchronization race. A body substring could arrive before a completed frame. The harness now waits for completed frames and separates resize from help dismissal, retaining its two-second quit bound. Full QA, five navigation confirmation trials, and an independent Spec rerun pass; incomplete frames and a non-rendering binary are rejected.
- Matt Pocock's independent Standards and Spec reviews report zero confirmed findings. Full source security review found no additional confirmed vulnerabilities beyond the corrected cache boundary and compiler issue. This is not a penetration test or a claim that all vulnerabilities are absent.
- P3 autoreview initially could not pass its sandboxed scan preflight. Automatic approval rejected the external transfer until the user explicitly approved sending this diff/context to OpenAI; the authorized review then completed with all isolation and secret-scanning controls intact.
- The first completed GPT-6 Astra P3 review found one benchmark defect: overload sampled after the final ordering was recorded but not refused. A real-Hyperfine regression reproduced the false success. The harness now validates the exact recorded sample both before and after each ordering; rejection and explicit-override warning controls pass. The confirming full source review returned scoped-clean, exit zero, with no accepted findings. Review evidence is `/private/tmp/devtrim-autoreview-final.json` and `/private/tmp/devtrim-autoreview-confirm.json`.
- Autoreview cannot inspect binary diffs. Every changed source is reviewed; the generated MP4 is verified separately by a fresh reviewer, exact SHA-256 (`502ee96d4205b12b66c028b106272e2eebe219ff384493ca873ae709b3404445`), stream metadata, and menu/preview frames. A suspected spacing defect was withdrawn after original-resolution pixel checks showed the intended gaps; no speculative media fix was made.

### Performance evidence and limits

Both saved comparison binaries used Rust 1.98.1, release profile, arm64 target and version 0.8.0, after the intentional Hugging Face correction. Baseline SHA-256: `1021f0d8abddd3a5132413c87d40d2ee26b87b099a5f30d8e8eca1a2712a2228`; candidate: `bd529e624962d9f3423c6fd95ce1c03a2fe17a19c91ccef5cdd2449173bb9ac1`.

- Full scan: 32,180 fixture entries, including 20 stale and five recent repositories plus 20,000 noise directories; both binaries returned 110 findings and identical 35,150-byte JSON with no errors.
- Installers: 5,000 stale files; both returned successful, identical 1,880,088-byte JSON. Eligibility now reads metadata once instead of four times; preview identity and fresh apply checks remain separate.
- The A/B harness refused timing at load 264.97 on 18 CPUs (threshold 9). No forced timing or wall-clock speedup claim is made.
- Six-second idle process-CPU samples at 120×40: analyze with 6,000 children used 0.25 seconds before and below the 0.01-second reporting resolution after; status used 0.03 versus 0.01 seconds. Load was 279–308, so these are raw observations, not a reliable percentage improvement. Progress and blocked-probe quit remained within four milliseconds in the independent QA run.
- Session evidence: `/private/tmp/devtrim-performance.pVohSI/`, `/tmp/devtrim-view-baseline.json`, `/tmp/devtrim-view-candidate.json`; reproducible harnesses live in `scripts/perf/` and `scripts/tests/`.

Self-assessment: the fixes and deterministic controls support the implementation. A quiet-host latency distribution would strengthen the performance evidence; inventing a percentage from this host would weaken it. The durable improvement is explicit preview ownership and tests that prove both permitted behavior and refusal. Future work should retain those contracts while replacing tools or models, and revisit ranking or combined walks only when profiling identifies them as material costs.

## External review basis

These are published expert and vendor references, not claims that their authors reviewed this patch:

- [Hugging Face environment variables](https://huggingface.co/docs/huggingface_hub/en/package_reference/environment_variables): HF_HOME includes token state; HF_HUB_CACHE identifies cached repositories.
- [Rust 1.98.1 advisory](https://blog.rust-lang.org/2026/09/03/Rust-1.98.1/): reason for replacing the affected compiler pin.
- [Nicholas Nethercote, Rust benchmarking](https://nnethercote.github.io/perf-book/benchmarking.html) and [profiling](https://nnethercote.github.io/perf-book/profiling.html): representative workloads and measurement before optimization.
- [Brendan Gregg, performance methodology](https://www.brendangregg.com/methodology.html): investigate workload and bottlenecks rather than choosing tools at random.
- [Martin Fowler, test pyramid](https://martinfowler.com/bliki/TestPyramid.html): keep broader tests purposeful while retaining focused behavioral proof.
- [Fable 5.1 prompting](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-fable-5-1) and [GPT-6 Astra guidance](https://developers.openai.com/api/docs/guides/latest-model?model=gpt-6-astra): explicit scope, targeted edits, completion criteria, bounded testing, and clear instruction precedence.
