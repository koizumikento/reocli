---
name: codex-reviewer
description: Perform rigorous code reviews focused on bugs, regressions, security issues, compatibility risks, and missing tests. Use when asked to review code, review a PR/diff, assess release risk, or provide review findings with severity and file/line evidence.
---

# Codex Reviewer

## Overview
Produce high-signal review findings that protect correctness, security, and maintainability.
Report findings first, ordered by severity, with concrete file/line evidence and actionable fixes.

Read [references/review-best-practices.md](references/review-best-practices.md) when you need the full checklist or rationale.

## Review Workflow
1. Define scope.
2. Inspect changed files and affected call sites.
3. Run relevant checks/tests when possible.
4. Evaluate findings using the checklist below.
5. Report findings first, then open questions/assumptions, then a brief summary.

## Severity Rubric
- `P0`: Security vulnerability, data loss, auth bypass, or production outage risk.
- `P1`: Likely functional bug or regression in normal usage.
- `P2`: Reliability, edge-case, or maintainability issue likely to cause future defects.
- `P3`: Minor issue with limited impact but still worth fixing.

## Core Checklist
- Verify behavior against expected logic and edge cases.
- Verify error handling paths and failure messages.
- Verify security basics: input validation, authorization checks, secret handling.
- Verify API compatibility and migration safety.
- Verify tests cover changed behavior and regressions.
- Verify observability impact where relevant (logs/metrics/errors).

## Rust-Specific Checklist
- Prefer `Result` propagation over `panic!` in library paths.
- Flag risky `unwrap`/`expect` in non-test code.
- Check error context quality and error type boundaries.
- Check ownership and lifetime changes for hidden behavior changes.
- Check clippy/rustfmt/test compliance for changed code.
- Check public API/documentation updates when behavior changes.

## Output Contract
- Start with `Findings` and list each item as:
  - `[Px] <title>`
  - `Why it matters:`
  - `Evidence:` absolute file path + line reference
  - `Recommended fix:`
- Add `Open Questions / Assumptions` only when needed.
- Add a short `Summary` after findings.
- If no findings, state `No findings` and note residual test/coverage risk.
