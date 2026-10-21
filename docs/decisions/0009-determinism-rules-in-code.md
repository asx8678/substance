# 0009 Determinism rules in code

Status: accepted · Date: 2026-10-21

## Context

Principle 7 promises "same input, same verdict", but ULID-shaped record IDs, `settle_ms`, wall-clock defaults and unseeded randomness make runs differ. §4.3 fixes the rules that make output reproducible.

## Decision

- IDs are content-derived: `<prefix>_<first 26 chars of lowercase base32(sha256(canonical parts))>`, hashing a stated tuple per prefix; blobs are `sha256:<hex>`.
- Collections are `BTreeMap`/`BTreeSet` with every output vector sorted by a stated key; no floats in records or reports (ms, bytes, permille); normalized JSON is canonical JSON with keys sorted.
- The wall clock is reached only through `substance_core::clock`, once, to default `as_of`; `Instant` is for timeouts and is never serialized.
- Randomness is only `ChaCha8Rng::seed_from_u64(seed)`, one seed per generator, derived from the master seed and stored in the manifest.
- Verdict-changing budgets are counts in sorted order (`blame_max_files`, `forge_max_requests`, `crawl_max_*`, `probe_max_targets`); wall time is a safety cap.

## Consequences

- A fired time cap makes the affected checks report `not_measured("budget exhausted")` and adds `time cap hit: <stage>; this result depends on machine speed`; the guarantee covers runs where no cap fired.
- The normalized form omits `id` and `timing.settle_ms`, so equal runs give equal bytes.
