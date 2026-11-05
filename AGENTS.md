# Working on wtg

`wtg` is a Rust CLI that identifies commits, issues, PRs, files, and tags and
traces them to releases. Keep startup fast and output concise, accurate, and
lightly snarky.

## Architecture

- The CLI owns argument/environment handling, orchestration, output, and exit codes.
- `crates/wtg/src/backend/` isolates local git, GitHub, and combined data access;
  `resolution.rs` composes backend operations into answers. Keep resolution logic
  independent of terminal output and emit non-fatal notices through callbacks.
- Backend selection takes parsed input and returns a backend plus optional notice.
  Use `Backend::backend_for_pr` for cross-project PR lookups.
- `GitRepo` provides query-oriented access to commits, file history, tags, and
  remotes. Prefer local metadata and cached refs; fetch only with explicit caller
  permission. Handle API rate limits and transient failures.
- `crates/wtg-python/` and `src/wtg_cli/` expose the Rust CLI to Python;
  `pyproject.toml` and `.github/workflows/build-py-wheels.yml` define packaging.

## Changes and verification

- Follow existing patterns and keep changes within the requested scope. Ask when
  a missing decision materially affects behavior or authority; resolve routine
  implementation and recoverable environment issues without stopping unrelated work.
- Add dependencies only when their benefit justifies the cost. Remove unused code,
  including unused public helpers; do not suppress it with `allow(dead_code)`.
- Keep flags and errors consistent. Update docs and examples when behavior changes;
  explain non-obvious logic with brief comments where needed.
- Use the root `justfile` for commands and match CI's Rust toolchain when validating
  Rust changes; see `.github/workflows/ci.yml`.
- For Rust changes, run `just fmt`, `just fmt-check`, `just lint`, and `just test`.
  Add focused regression coverage when behavior changes. Run `just ci` for release
  preparation or major changes. Packaging changes also need wheel/source-install
  checks on the affected interpreters and platforms.
- For documentation or skill-metadata changes, check formatting, links, and metadata;
  Rust checks are unnecessary unless Rust or build behavior also changes.
- Report verification results and remaining gaps. Once appropriate checks pass,
  repeat them only for new changes, failures, or unresolved concerns.

## Releases

The shared [release skill](.agents/skills/release/SKILL.md) is available only through
explicit user invocation: `$release [version]` in Codex or `/release [version]` in
Claude Code. It owns the release procedure and its changelog and production-release
approval gates. Ordinary implementation or release-preparation requests do not
invoke it or authorize publishing.
