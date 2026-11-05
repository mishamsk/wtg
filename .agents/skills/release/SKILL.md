---
name: release
description: Prepare and publish a wtg release, only when explicitly invoked by the user.
argument-hint: "[version]"
disable-model-invocation: true
---

# Release wtg

Run only on explicit user invocation: `$release [version]` in Codex or
`/release [version]` in Claude Code. Mentions, quoted examples, and general
release-preparation requests do not invoke this workflow. Take the optional version
from the user's invocation; otherwise propose a version from the changes.

## Prerequisites

Require a clean worktree on `main`, passing CI for the current HEAD, and a previous
release tag. Check the current workspace version, remote state, and existing tags
before changing release files. Report unmet prerequisites without switching branches,
rewriting tags, or discarding work.

## Changelog approval

Compare commits since `git describe --tags --abbrev=0` and merged GitHub PRs against
`CHANGELOG.md`'s Unreleased section. Add missing user-facing changes using the
existing categories, verbosity, and PR-link format. Show the changelog diff and
wait for user approval before assigning the version or publishing anything.

## Prepare the version

Use the requested version; otherwise propose a semver bump: major for breaking
changes, minor for features, patch for fixes and compatibility updates. Date the
release section, retain an empty Unreleased section, update `[workspace.package]`
in `Cargo.toml`, and refresh `Cargo.lock` with `cargo tree --depth 0`.

Run `just ci` with the toolchains used by CI. Review and commit only the release
files (`CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`) as
`chore: prepare release vX.Y.Z`, then push to `origin/main`. Stop and report failed
checks or unexpected changes rather than bypassing them.

## Validate TestPyPI

Dispatch `publish-test-pypi.yml` with `gh workflow run publish-test-pypi.yml --ref main`. Track that run's
ID and commit; wait for completion with a 15-minute timeout. The workflow publishes
`X.Y.Z.devN`, where `N` is its run number. Verify that exact version rather than
allowing an older TestPyPI release to satisfy the check.

Set `test_version` to that published version, then clear the package cache and run:

```bash
uv cache clean wtg-cli
uvx --index-url https://test.pypi.org/simple/ --refresh --from "wtg-cli==$test_version" wtg --help
uvx --index-url https://test.pypi.org/simple/ --from "wtg-cli==$test_version" wtg --version
uvx --index-url https://test.pypi.org/simple/ --from "wtg-cli==$test_version" wtg v0.1.0
```

Confirm the reported version matches. Repeat with `--python` for newly supported
Python versions. If publishing or installation fails, or the run times out, report
the results and await guidance; do not proceed to production.

## Production approval and publishing

Present the TestPyPI results and obtain explicit final release confirmation.
After confirmation, ensure the release commit is still the intended `main` HEAD,
then create and push its signed tag:

```bash
git tag -s -m "release vX.Y.Z" vX.Y.Z
git push origin vX.Y.Z
```

The tag triggers `.github/workflows/release.yml`: version validation, CI, wheel
builds, crates.io and PyPI publishing, then a draft GitHub release. Report the
workflow link and status; distinguish a triggered workflow from successful
publishing. Leave the GitHub release as a draft unless the user asks to publish it.
