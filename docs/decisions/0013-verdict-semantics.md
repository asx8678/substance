# 0013 Verdict semantics

Status: accepted · Date: 2026-10-21

## Context

The plan uses `not_measured` and `unverifiable` without distinguishing them, and many H checks have no threshold, so a "pass otherwise" default would make them always pass.

## Decision

- `not_measured`: the check could not run or lacked an input — a missing tool, adapter or permission, a crash, a disabled check, or an exhausted budget.
- `unverifiable`: the check ran, but the evidence cannot decide; H01 on commit dates that predate the repo is the example.
- `reported`: the check ran and recorded metrics but has no threshold yet, uncalibrated or context-only, such as H19; it is neither pass nor fail, counts as measured for coverage, and never lowers a grade.
- Neither `not_measured` nor `unverifiable` counts as pass, and both carry a reason.

## Consequences

- Coverage next to every score counts `reported` as measured, so a check without a threshold cannot be read as a pass.
- A `reported` check can gain a threshold later and become pass or fail without its ID changing.
- Every `not_measured` and `unverifiable` verdict shows its reason in report.json.
