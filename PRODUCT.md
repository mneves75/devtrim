# PRODUCT.md — devtrim

## What it is
devtrim is a macOS CLI that reclaims developer-machine disk space safely:
measure → classify by danger score → trim with Trash-first deletion.
Born from a real cleanup session that recovered 250+ GB.

## Who it serves
Professional developers running heavy local toolchains (Xcode, Rust, Node,
Python ML, Docker). They are terminal-fluent, skeptical of "cleaner" apps,
and allergic to tools that delete things without asking.

## Visitor success (docs surface)
Mode: **Read**. A visitor must (1) understand what devtrim will and will not
touch before running anything, (2) find the exact command for their situation,
(3) trust the safety model enough to run `--apply`.

## Brand personality
Precision instrument. Terminal-native. The design language of a calibrated
tool: machined detail, honest labels, visible guardrails. Confidence without
marketing voice. Never cute, never corporate-slop ("unleash", "elevate").

## Non-negotiables
- Safety information outranks aesthetics at every decision point.
- Danger semantics: green = safe, amber = think, red = irreversible.
- Works offline, dark-first (terminal audience), light fully supported.
- Single self-contained HTML file. No external fonts, no JS dependencies.
