# Progress — 0.18.0 (plans/022)

Branch `feat/mole-lessons` from `3b6bde9`. Mole clone (untrusted data) in the session scratchpad.

- Baseline `verify.sh focused` passes on 3b6bde9; offline running.
- Plan 022 written: npm/pnpm narrowing, incident ledger gate, apply-stop formal model.
- Live evidence: pid 26224 typescript-language-server runs from ~/.npm/_npx (746 MB of 781 MB).
- Step 1 done: npm `_cacache` only, pnpm metadata folders only, apply reasserts both; 3 planted cases caught.
- Host degraded since ~11:40: `pgrep` fails (sysmond missing), `hdiutil create` fails; 7 host-dependent tests fail for that reason only.
- Step 2 done: docs/incidents.md (8 rows), audit + 13 controls in verify.sh and CI. Found: 33fe4e2's two catcher tests were deleted in 0.11.0 unnoticed; rows now name the evals that guard them.
- Next action: step 3, stop-protocol model.
