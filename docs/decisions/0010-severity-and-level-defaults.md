# 0010 Severity and level defaults

Status: accepted · Date: 2026-10-21

## Context

"Scoring and report" fixes severity per check, but the catalog has no severity column, and levels exist only for B checks, with B33 given as a range. The registry (M0-T03) needs exactly one severity and one level per check.

## Decision

- Severity, uncalibrated: critical is B02, B05, B10, B11, B13, B33, L01, L05 and S11; info is H01–H22, V01 and V03, and also the placeholder for I03 and the 8 reserved IDs; the rest take high, medium or low exactly as the M0-T03 severity table lists them.
- Level: B, I and L checks take the catalog's level, a range taking its maximum, so B33 is L3.
- Level, others: L0 for H, S, P and V (not reserved V02), T04–T06, D01, D02, D04 and O08–O10; L1 for O01, O02, O04, O05 and T01–T03; L2 for D03; L3 for O03, O06, O07, O11.
- I03 and the 8 reserved IDs (V02, A01–A05, G01, G02) have no level.

## Consequences

- Both tables are provisional: the thresholds they feed carry `calibrated = false` until the benchmark corpora exist (ADR-0016).
- In v1 a level is recorded but gates nothing, because only repo mode exists.
- A reserved ID carries info severity and no level, so it registers without being scored.
