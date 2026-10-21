# 0018 Database reset: slot first, then template, and Supabase replays

Status: accepted · Date: 2026-10-21

## Context

Probes need a fresh database each (ADR-0009); the plan never says what a slot does when a run dies.

## Decision

- A reset runs only while the app is stopped, enforced by a `StoppedApp` token whose only source is
`AppProcess::stop`. Postgres: drop slot, `DROP ... WITH (FORCE)`, `CREATE ... TEMPLATE`, recreate
slot; the slot goes first because it pins WAL the drop cannot reclaim.
- SQLite snapshots with its online backup API, and resets by copying the file back after deleting
`-wal` and `-shm`: a plain copy would miss the WAL, so it would look right and be wrong.
- Supabase resets with `supabase db reset`, replaying the repo's migrations: slower by orders of
magnitude, so not the engine to pick when reset count matters.
- Teardown is idempotent, and Postgres also cleans up from `Drop` on a bounded one-purpose thread.
- A missing capability is `not_measured` with a reason, never a failure. Credentials leave no
memory, and every message carries `RedactedUrl`, not a connection URL.

## Consequences

A reset cannot run against a live app without a token nothing can construct; without logical
decoding only B03, B04 and B09's row-diff half is `not_measured`. `SIGKILL` runs no code, so
`substance doctor` lists what a killed run left; every other way out is bounded by one `Drop`.