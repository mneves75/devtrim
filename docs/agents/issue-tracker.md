# Issue tracker: Local Markdown

Feature specs and implementation tickets live in `docs/specs/`. Do not create
GitHub issues unless the owner explicitly requests them.

## Conventions

- One directory per feature: `docs/specs/<feature>/`.
- A longer feature's specification is `spec.md`; tickets are individual files
  in `issues/NN-<slug>.md`, numbered from `01`.
- Record a ticket's state in a `Status:` line; append decisions under
  `## Comments` rather than replacing its history.
- Keep the specification; remove completed ticket files in the finishing commit.
- A single-session feature needs no formal spec or tickets. Keep its acceptance
  criteria and evidence in the task's git-ignored `.scratch/` record.

When an engineering skill says to publish or fetch a ticket, use those local
files. Existing `plans/` documents remain historical context; the owner's
current request and confirmation define the task.
