# 0008 No tests

Status: accepted · Date: 2026-10-21

## Context

The plan's criteria and conventions assume tests; the owner instructed production code only.

## Decision

- Production code only: no `#[test]`, `#[tokio::test]` or `#[cfg(test)]`, no
  `tests/` or `benches/`, no `*.test.ts` or `*.spec.ts`, no goldens or fixtures, no
  test-only dependencies; C-GUARD (`scripts/guards.sh`) fails on each.
- Replace invariant tests with `const _: () = assert!(…);`; C-SCHEMA codegen replaces
  round-trip tests.
- T01–T03 and `bench/` stay in scope: they run the *evaluated app's* tests and benchmark.
- Never run `cargo test`, `cargo nextest`, `npm test`, `vitest`, `jest`, `pytest`, the
  Playwright runner, or `substance run|replay|bench` on an app; those are owner runs (§8).

## Consequences

- Checks stay build, lint, codegen and guard runs. Deferred by this ADR (§7): round-trip
  tests for core types, golden-file oracle tests (`crates/oracles/tests/fixtures/`),
  per-probe integration tests, the M4 double-run diff, the CI benchmark recall gate, and
  "tests alongside the code".
