# Agent Operating Manual

This document defines the baseline process for agents working in the `wtg` repository. Follow it strictly to minimize turnaround time and ensure reliable outputs.

## Repository Intent
- `wtg` is a compact Rust CLI that reports “who did what” activity for git/GitHub projects.
- Feature work must stay narrowly scoped to that goal; reject requests that expand beyond contributor analytics or degrade responsiveness.

## Immediate Preparation
- Read the `justfile` at repository root before running any tasks; it is the authoritative source of project commands.
- Confirm you can run Rust tooling locally (`cargo`, `rustfmt`, `clippy`). Obtain access before proceeding with changes.

## Required Workflow
1. Keep the change set minimal. If a request requires multiple concerns, clarify or split it.
2. Implement updates following existing patterns in `src/` and `crates/wtg/`.
3. Update documentation and examples when behavior changes.
4. Execute the following commands before presenting work:
   - `just fmt`
   - `just fmt-check`
   - `just lint`
   - `just test`
   - `just ci` when preparing a release or major change
5. Include verification details (what was run, results, outstanding gaps) in your handoff message.

## Implementation Principles
- Avoid new dependencies unless they deliver measurable benefit to command output quality or performance.
- Optimize for fast startup and low noise. Every log line must aid in understanding contributor activity.
- Maintain deterministic behavior; if external APIs are touched, defend against rate limits and transient failures.

## User Experience Requirements
- CLI responses must remain concise, actionable, and lightly snarky per product positioning, but never at the expense of accuracy.
- Ensure flags, command names, and error messages stay consistent across updates.

## Collaboration Standards
- Ask for clarification when requirements are ambiguous; do not guess.
- Document non-obvious logic with brief comments only where essential for future maintenance.
- Keep the git history clean. Do not commit without passing formatting and clippy checks.

## Escalation & Safety
- If you encounter missing prerequisites, environmental blockers, or suspect corrupted state, halt and notify the maintainer immediately with observed details and attempted mitigations.
- Use workspace caches listed in the harness configuration for temporary artifacts; avoid writing outside allowed paths.
