# 0004 Repo mode only

Status: accepted · Date: 2026-10-21

## Context

The plan supported `repo`, `url` and `repo+url` intake, with `--i-own-this` as an
extra ownership assertion. URL mode needs a live deployment and a second
permission story that v1 does not pay for.

## Decision

- v1 intake accepts `repo` only.
- `url` and `repo+url` stay in the schema as values, so configs and reports keep
  their shape, but intake rejects them with a clear error.
- `--i-own-this` is not implemented.

## Consequences

- Every check may assume the source tree under test is present, so none needs a
  URL-mode fallback and an affected check never reports `not_measured` for a
  missing deployment.
- URL mode and `--i-own-this` are deferred (§7) and can be added later without a
  schema change.
