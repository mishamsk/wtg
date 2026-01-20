# Agent Operating Manual

This document defines the baseline process for agents working in the `wtg` repository. Follow it strictly to minimize turnaround time and ensure reliable outputs.

## Repository Intent
- `wtg` is a compact Rust CLI that reports “who did what” activity for git/GitHub projects.
- Feature work must stay narrowly scoped to that goal; reject requests that expand beyond contributor analytics or degrade responsiveness.

## Architecture
- Separation first: backends are swappable and isolate data sources (local git, hosted APIs, combined strategies).
- Library core is pure: it accepts structured inputs, returns results or errors, and emits optional notices via hooks/listeners only.
- CLI owns orchestration and all side effects (I/O, environment, exit codes, printing notices).
- Layers are explicit even if implementation evolves; preserve boundaries over convenience.

Layered flow and primary types

CLI args/env
   |
   v
Cli + parse_input -> ParsedInput { Query, GhRepoInfo? }
   |
   v
backend::resolve_backend -> ResolvedBackend { Backend, notice? }
   |
   v
resolution::resolve -> IdentifiedThing (EnrichedInfo | FileResult | TagOnly)
   |
   v
output::display -> stdout/stderr

Backend abstraction

Backend (trait)
  |-- GitBackend       (local git)
  |-- GitHubBackend    (GitHub API)
  `-- CombinedBackend  (local first, API fallback)

Notes
- `Backend` provides low-level operations; `resolution` composes them into higher-level answers.
- Cross-project lookups should use `Backend::backend_for_pr` to swap to the correct repo backend.
- Backend selection must be data-driven: `ParsedInput` in, `ResolvedBackend` out.
- Non-fatal notices should flow through a listener/hook so the CLI decides how to surface them.

## GitRepo Role and Caching
- `GitRepo` is a higher-level abstraction over the git crate, focused on query-friendly operations (commit lookup, file history, tags, remotes).
- Prefer local-first execution: use cached refs, tags, and commit metadata before any network calls.
- Cache behavior should be explicit and deterministic: only fetch when the caller explicitly allows it.
- Caching is a performance tool and a rate-limit defense; use it to reduce API calls and keep startup fast.
- The library should treat cache reads as pure data access; any network fetches are orchestrated by the CLI or by backends under explicit permission.

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
- Dead code is not allowed. Remove any unused functions, methods, or types immediately. This includes `pub` APIs that become unused (which the Rust compiler does not flag as dead code). Never use `#[allow(dead_code)]` to suppress warnings—delete the code instead.

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

## Release Flow

This section defines the process for preparing and publishing a release. Execute via `/release [version]` command.

### Prerequisites
- Clean working tree (no uncommitted changes)
- On `main` branch
- CI passing on current HEAD

### Phase 1: Changelog Review
1. Identify the last released tag (`git describe --tags --abbrev=0`)
2. Gather all commits since last tag (`git log <last-tag>..HEAD --oneline`)
3. Cross-reference with GitHub PRs merged since last release
4. Compare against current CHANGELOG.md Unreleased section
5. Identify missing user-facing items:
   - New features, enhancements, bug fixes
   - Breaking changes
   - Security updates
6. Update CHANGELOG.md preserving existing format:
   - Keep same verbosity level as existing entries
   - Include PR references (e.g., `([#4](https://github.com/mishamsk/wtg/pull/4))`)
   - Categorize into Added/Changed/Deprecated/Removed/Fixed/Security

### Phase 2: User Verification
1. Present changelog diff to user
2. Wait for user confirmation before proceeding
3. If rejected, iterate on changelog updates

### Phase 3: Version Assignment
1. Determine target version:
   - Use argument if provided (e.g., `/release 0.3.0`)
   - Otherwise infer from changes using semver:
     - MAJOR: Breaking changes
     - MINOR: New features (Added section has entries)
     - PATCH: Bug fixes only
2. Create new empty Unreleased section at top of CHANGELOG.md
3. Assign version and date to current changelog section
4. Update workspace `Cargo.toml` version field

### Phase 4: Commit and Push
1. Stage changes: `CHANGELOG.md`, `Cargo.toml`
2. Commit with message: `chore: prepare release vX.Y.Z`
3. Push to origin/main

### Phase 5: Test PyPI Validation
1. Trigger `publish-test-pypi.yml` workflow via `gh workflow run`
2. Wait for workflow completion (poll every 2 minutes, timeout 15 minutes)
3. Test installation and basic commands:
   ```bash
   uvx --index-url https://test.pypi.org/simple/ --from wtg-cli wtg --help
   uvx --index-url https://test.pypi.org/simple/ --from wtg-cli wtg --version
   uvx --index-url https://test.pypi.org/simple/ --from wtg-cli wtg v0.1.0
   ```

### Phase 6: Release Confirmation
1. Report test results to user
2. If issues found, stop and await guidance
3. If successful, ask user for final release confirmation

### Phase 7: Tag and Release
1. Create signed tag: `git tag -s -m "release vX.Y.Z" vX.Y.Z`
2. Push tag: `git push origin vX.Y.Z`
3. Report: Release workflow triggered, link to GitHub Actions

### Abort Conditions
- Uncommitted changes in working tree
- Not on main branch
- CI failing
- Test PyPI workflow fails
- Test installation fails
- User declines at any verification step
