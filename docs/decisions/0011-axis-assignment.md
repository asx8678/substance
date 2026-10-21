# 0011 Axis assignment

Status: accepted · Date: 2026-10-21

## Context

16 checks feed no scoring axis in the plan's axis table: B22, B27, B29, B31, B32,
I02, L02, P05, P06, S01, S03–S07 and S12. Each needs a stated axis, or a stated
"none", before a score can be computed.

## Decision

- B22 and B31 are works_now; B27, B29 and S12 are honest; L02 is robust; P05 is
  maintainable; P06 is operable.
- B32 and I02 get two axes: works_now and safe.
- S01 and S03–S07 get no axis: they are hints, shown under maintainable, counted
  toward the slop label and attached to findings as `cause`.

## Consequences

- Every check is classified, so each axis score is computable from the registry.
- Hint-only checks cannot move a score on their own; they act only through the slop
  label (ADR-0012) or as the cause of a behavioral finding.
- The other axes come from the plan's axis table unchanged.
