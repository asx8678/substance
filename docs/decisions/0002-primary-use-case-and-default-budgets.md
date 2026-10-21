# 0002 Primary use case and default budgets

Status: accepted · Date: 2026-10-21

## Context

The plan asks who runs substance and how long a run may take. The owner set the
primary use case as a CI gate over code they trust: their repos and their own
coding agents' output.

## Decision

- Primary use case: a CI gate for code the owner trusts.
- Budgets: triage 2 minutes, full run 30 minutes, default `--fail-on gate`.
- Execution is allowed on trusted repos only; in CI the runner VM is the isolation.

## Consequences

- The triage lane stays offline and execution-free, so it fits the 2-minute budget.
- A wall-time cap that fires makes the affected checks report
  `not_measured("budget exhausted")` plus the machine-speed note (ADR-0009).
- Evaluating untrusted forks, and the sandbox that would allow it, is deferred
  (ADR-0003).
