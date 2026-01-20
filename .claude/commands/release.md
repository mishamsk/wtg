---
description: Prepare and publish a release with changelog review and test validation
argument-hint: [version]
---

## Release Command

Execute the release flow for wtg. Optional version argument (e.g., `/release 0.3.0`).

**Target version:** $ARGUMENTS (if empty, will be inferred from changes using semver)

## Context Collection

- Current branch: !`git branch --show-current`
- Working tree status: !`git status --porcelain`
- Last tag: !`git describe --tags --abbrev=0 2>/dev/null || echo "no-tags"`
- Current Cargo.toml version: !`grep '^version = ' Cargo.toml | head -1`

## Pre-flight Checks

Before proceeding, verify:
1. Working tree is clean (no output from git status --porcelain)
2. Currently on main branch
3. A previous tag exists

If any check fails, stop and report the issue.

## Execution Steps

### Step 1: Changelog Review (Use Subagent)

Launch an Explore subagent to thoroughly review changes between HEAD and last tag:
- List all commits since last tag
- For each commit, check if it relates to a PR (look for PR numbers in commit messages)
- Cross-reference merged PRs via `gh pr list --state merged --base main`
- Compare findings against @CHANGELOG.md Unreleased section
- Identify user-facing items that are missing

Present findings and propose CHANGELOG.md updates. Match the verbosity and style of existing entries. Include PR links in format `([#N](https://github.com/mishamsk/wtg/pull/N))`.

### Step 2: User Verification

After updating CHANGELOG.md, show the diff and ask user to confirm changes are good.

### Step 3: Version Assignment

If version argument provided, use it. Otherwise, infer from changelog:
- If Unreleased has entries in "Added" → bump MINOR
- If only "Fixed" entries → bump PATCH
- If breaking changes noted → bump MAJOR

Then:
1. Create new empty Unreleased section in CHANGELOG.md
2. Change `[Unreleased]` to `[X.Y.Z]` with today's date
3. Update version in workspace Cargo.toml

### Step 4: Commit and Push

```bash
git add CHANGELOG.md Cargo.toml
git commit -m "chore: prepare release vX.Y.Z"
git push origin main
```

### Step 5: Test PyPI Validation

```bash
gh workflow run publish-test-pypi.yml
```

Poll workflow status every 2 minutes until completion (max 15 minutes). When complete:

```bash
uvx --index-url https://test.pypi.org/simple/ --from wtg-cli wtg --help
uvx --index-url https://test.pypi.org/simple/ --from wtg-cli wtg --version
uvx --index-url https://test.pypi.org/simple/ --from wtg-cli wtg v0.1.0
```

### Step 6: Final Confirmation

Report test results. If all passed, ask user: "Ready to trigger the production release?"

### Step 7: Tag and Release

If user confirms:

```bash
git tag -s -m "release vX.Y.Z" vX.Y.Z
git push origin vX.Y.Z
```

Report success with link to GitHub Actions release workflow.

## Important Notes

- Follow @CLAUDE.md for code standards
- Run `just fmt` and `just lint` if any Rust files are modified
- Never use --no-verify when committing
