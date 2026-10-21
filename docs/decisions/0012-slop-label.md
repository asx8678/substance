# 0012 Slop label

Status: accepted · Date: 2026-10-21

## Context

Principle 10 requires agreeing evidence from at least two independent check
families before calling slop, but the plan never defines "independent family" and
never says whether hints count. Left open, one family could satisfy the rule twice.

## Decision

- A check's family is its ID prefix letter.
- Only B, I, D, O, L, S, T, P and H count. V (provenance), A (agent) and G
  (generic) never count.
- A `hint`-confidence finding counts like any other finding.
- A slop label needs fails from at least two of the counting families.

## Consequences

- A label never rests on one family: H plus S qualifies, H plus V does not, because V
  never counts.
- The rule is family-wide, so many fails inside one family still count once.
- An app with fails in B only is not labelled slop, and a check that cannot run
  reports `not_measured` rather than contributing (ADR-0013).
