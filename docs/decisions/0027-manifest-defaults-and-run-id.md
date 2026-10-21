# 0027 Manifest defaults and run ID

Status: accepted · Date: 2026-10-21

## Context

M0-T08 fills the manifest before any task computes the rule pack, threshold and
scoring-weight versions, and the plan does not name a default master seed.

## Decision

- Absent `--seed`, `master_seed` is the first 8 bytes of
  `sha256("substance-seed" || commit_sha)`, read big-endian. `--seed` replaces it.
- `rule_pack_hash`, `thresholds_version` and `scoring_weights_version` hold the literal
  `"none"` until M1-T01 and M13 compute them.
- `host {os, arch}` come from `std::env::consts`; `frozen_clock` starts at `as_of`, and
  `server_clock` is `real` until M3 can move a clock.
- `run_id` hashes the canonical JSON of the manifest with `run_id` removed, so
  `invocation.json` (CLI flags, run directory) stays outside every hash.

## Consequences

- Two runs of one commit, seed and `as_of` share `run_id` and every derived ID.
- A manifest may carry `"none"`; a reader must not treat it as a content hash, and
  checks that need those inputs report `not_measured` until their task lands.
