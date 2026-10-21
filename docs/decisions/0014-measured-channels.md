# 0014 `Measured<T>` channels in EffectRecord

Status: accepted · Date: 2026-10-21

## Context

The plan's EffectRecord JSON gives every channel a plain field, so a record cannot say that a channel was not measured — `queries` on SQLite, for example — although principle 8 requires exactly that.

## Decision

- Every channel of an EffectRecord is `Measured<T>`: the value with how it was obtained, or an explicit not-measured marker carrying a reason.
- A channel without an adapter, permission or support is `not_measured("…")` rather than an empty collection, so "nothing happened" and "nothing was watched" stay distinct.
- This deviates from the plan's JSON sample (review issue 7, ADR-0013).

## Consequences

- An oracle that needs a missing channel reports `not_measured` instead of passing on an empty diff.
- Every consumer asks a channel whether it was measured before reading it, and each record grows slightly.
- The normalized form compares marked channels as data, so equal runs still match (ADR-0009).
