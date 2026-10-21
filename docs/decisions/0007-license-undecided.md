# 0007 License undecided

Status: accepted · Date: 2026-10-21

## Context

The plan leaves the license and the question of a public repo open. Adding a
license now, or publishing crates, would fix a choice the owner has not made.

## Decision

- No LICENSE file is added in v1.
- Every crate keeps `publish = false` through `[workspace.package]`.
- No release artifact leaves the owner's machines.

## Consequences

- Choosing a license later is one new file plus one workspace key, with no code
  change.
- The repository makes no distribution promise, so nothing in v1 depends on
  license metadata.
