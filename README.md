# substance

substance is a command-line tool that judges whether an app is solid or surface without substance, using deterministic, evidence-backed checks.

It answers one question with evidence: does this app do what it claims, survive failure, and show real development? Slop, here, means claims exceeding evidence: a polished UI, a confident README and lots of code, but features that don't work, fail silently, or were dumped in one shot.

substance judges the artifact, not the author. It never scores whether code was written by an AI; provenance is reported and never scored.

## How it works

A run goes through stages:

1. **Intake** resolves the repository and commit, pins the inputs (seed, clock, timezone, locale, tool versions, thresholds and rule hashes) and writes a run manifest. The run ID is derived from that manifest, so the same inputs give the same run.
2. **Triage** reads git history, the code and the dependency lockfiles without executing anything from the repository: history signals (H01–H22) and an archetype such as "one-shot dump" or "solo maintained", static checks (S01–S12, D04, O01, O10, T04–T06, L05), dependency checks (P01–P07) and provenance (V01, V03, never scored).
3. **Runtime** (only with `--allow-exec`) exports the commit to its own working copy, boots the app on the host through a boot ladder (README steps, then `substance.toml`, then detected commands), provisions a per-run database, seeds it, creates test accounts and snapshots the seeded state. Operability checks O01–O05 judge this stage.
4. **Explore, probe and judge** drive the running app through a Playwright driver, record every effect (UI, network, storage, database rows, queries, logs, outbound traffic, email) as an effect record, and judge those records with pure oracles. This part is being built (see Status).
5. **Report** writes `report.json`, a self-contained `report.html` and one file per finding.

Every check returns one of: `pass`, `fail`, `flaky`, `reported` (metrics only, no threshold yet), `not_measured` (with a reason, for example a missing tool or permission) or `unverifiable`. Something that was not measured is never shown as a pass.

## Status

Work follows `docs/ACTION_PLAN.md`; its section 9 tracks every task.

| Milestone | State |
| --- | --- |
| M0 Foundations (workspace, check registry, data model, config, CLI, schemas, CI) | done |
| M1 Triage without running the app (50 checks, report v0) | done |
| M2 Runner and boot (working copy, process supervisor, boot ladder, databases, seeding, O01–O05, `doctor`) | done |
| M3 Recorder and driver (Playwright driver, driver host, recorder channels, egress proxy, Mailpit, normalization, clock, replay) | done |
| M4–M11, M13 Crawler, behavioral checks, benchmark, API layer, resilience, front end, test truthfulness, Python and Rust packs, scoring, SARIF | not started |
| M12 Agentic layer | deferred (ADR-0006) |

`substance bench` and `substance report` are not implemented yet and say so when run.

## Requirements

- Rust (the toolchain is pinned in `rust-toolchain.toml`) and git.
- Node.js (version in `.nvmrc`) for the browser driver in `driver/`.
- Optional, each enabling more checks: Postgres with logical decoding (`SUBSTANCE_PG_ADMIN_URL`), the Supabase CLI, `sqldiff`, gitleaks, osv-scanner, OpenSSF Scorecard, Mailpit, Playwright's Chromium, and the other tools listed in `tools.toml`. A missing tool makes its checks report `not_measured`.

`substance doctor` lists what this machine supports.

## Usage

Build and install the binary:

```sh
cargo install --locked --path crates/cli
```

Common commands:

```sh
substance doctor
substance checks list --status implemented
substance run --triage-only <repo>
substance run --allow-exec <repo>
substance replay <record-or-finding-id> --run <run-dir> --source <repo> --allow-exec
substance schema validate --kind config docs/substance.example.toml
```

Useful `run` options: `--rev <sha>`, `--as-of <rfc3339>`, `--seed <u64>`, `--config <path>`, `--snapshot-from <run-dir>` (replay forge, registry and network-tool data from an earlier run), `--format json,html` and `--fail-on gate|critical|high|none`.

Exit codes: `0` ok, `1` the `--fail-on` threshold was hit (or a replay differed), `2` usage, config or intake error, `3` internal error or interrupted run.

A run writes `runs/<run_id>/` with `manifest.json`, `report.json`, `report.html`, `findings/`, `records/`, content-addressed `blobs/`, and saved `forge/`, `registry/` and `tools/` data.

Project settings live in `substance.toml` at the root of the evaluated repository; `docs/substance.example.toml` documents every key.

## Safety

There is no sandbox (ADR-0003). With `--allow-exec`, substance runs the repository's install, build, migrate, seed and start commands as processes on your machine. Use it only on repositories you trust; in CI, the runner VM is the isolation. Without `--allow-exec`, nothing from the repository is executed, and the checks that would need it report `not_measured("needs --allow-exec")`.

The app's outbound traffic goes through a logging proxy that denies by default, but clients that ignore proxy settings are not blocked (ADR-0021).

## Repository layout

- `crates/`: the Rust workspace (`core`, `orchestrator`, `runner`, `recorder`, `history`, `statics`, `adapters`, `report`, `cli` and others).
- `driver/`: the TypeScript Playwright driver; `driver/src/generated/` is generated from the Rust types.
- `rules/`: rule packs and data files (ast-grep rules, normalization, provenance, log errors, licenses).
- `config/thresholds.toml`: provisional check thresholds, marked uncalibrated (ADR-0016).
- `schemas/`: generated JSON Schemas for every file a run writes.
- `docs/ACTION_PLAN.md`: the v1 task plan; `docs/decisions/`: the ADRs.
- `Substance — architecture plan for a deterministic app evaluator.md`: the design.

## Development

By owner decision (ADR-0008) the repository has no tests; invariants are compile-time assertions, guards and generated-schema checks. The CI gates are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked
scripts/guards.sh
npm --prefix driver ci
npm --prefix driver run typecheck
npm --prefix driver run lint
```

## License

Not decided yet (ADR-0007). Every crate is `publish = false`.
