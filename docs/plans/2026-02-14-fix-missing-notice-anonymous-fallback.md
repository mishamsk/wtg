# Fix: Missing notices when anonymous fallback fails with non-rate-limit errors

## Overview

When `GitHubClient` falls back from an authenticated client to the anonymous backup client (e.g., due to bad credentials), and the backup client fails with a non-rate-limit error (timeout, generic API error, etc.), no notice is emitted. This causes the `integration_invalid_github_token_falls_back_to_anonymous` test to fail because it sees `None` with an empty notices list.

## Context

- Files involved:
  - `crates/wtg/src/github.rs` (fallback logic in `call_api_and_get_client`, lines 969-1038)
  - `crates/wtg/src/notice.rs` (Notice enum)
  - `crates/wtg/tests/integration.rs` (the failing test, lines 336-384)
- Root cause: In `call_api_and_get_client` (github.rs:1033-1036), when the backup client fails with a non-rate-limit, non-SAML error (e.g., `Timeout` or `GitHub(OctoError)`), the error is returned without emitting any notice. The caller converts this to `None` via `log_err()`, and no notice trail is left.

## Diagnosis

The fallback flow in `call_api_and_get_client`:
1. Main client fails with bad credentials -> falls through to backup
2. Backup client succeeds -> OK (line 1018-1021)
3. Backup client fails with rate limit -> emits `GhRateLimitHit { authenticated: false }` (line 1023-1031)
4. Backup client fails with SAML -> returns original error, no notice (line 1032)
5. Backup client fails with anything else -> returns error, NO NOTICE (line 1033-1036) <-- BUG

Scenario 5 is the bug. When backup fails with timeout or generic GitHub error, no notice is emitted, so the test's assertion on notices fails.

## Development Approach

- **Testing approach**: Regular (code first, then tests)
- Complete each task fully before moving to the next
- **CRITICAL: every task MUST include new/updated tests**
- **CRITICAL: all tests must pass before starting next task**

## Implementation Steps

### Task 1: Add a notice for anonymous fallback failure

**Files:**
- Modify: `crates/wtg/src/notice.rs` - add a new `GhAnonymousFallbackFailed` notice variant (or similar name) that captures that the anonymous fallback was attempted but failed for a non-rate-limit reason
- Modify: `crates/wtg/src/github.rs` - in `call_api_and_get_client`, emit this notice in the generic backup error arm (line 1033-1036) before returning the error

- [x] Add new Notice variant to capture anonymous fallback failure
- [x] Emit the notice in the generic backup error catch-all arm of `call_api_and_get_client`
- [x] Update the test in `integration.rs` to also accept the new notice variant as valid (in addition to `GhRateLimitHit`)
- [x] Run `just test` - must pass before next task

### Task 2: Handle notice display in CLI

**Files:**
- Modify: whatever file formats notices for CLI display (need to check where Notice variants are matched for user-facing output)

- [ ] Add display/formatting for the new notice variant, keeping it consistent with existing notice messages
- [ ] Run `just test` - must pass

### Task 3: Verify acceptance criteria

- [ ] Run `just fmt` and `just fmt-check`
- [ ] Run `just lint`
- [ ] Run `just test`
- [ ] Manually verify: the test `integration_invalid_github_token_falls_back_to_anonymous` handles both rate-limit and non-rate-limit backup failures gracefully
