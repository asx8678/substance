# 0005 GitHub only

Status: accepted · Date: 2026-10-21

## Context

The history and provenance checks need issues, pull requests and commit metadata
from a forge. The plan named GitHub and GitLab; v1 pays for one client and one
API's quirks.

## Decision

- v1 supports GitHub only, behind a `Forge` trait, so another forge is an
  implementation rather than a rewrite.
- The client is read-only, its request count is the `forge_max_requests` count
  budget, and its responses are snapshotted so a run replays offline.
- A non-GitHub origin, whether a GitLab remote or no remote at all, returns
  `ForgeUnavailable`, so forge-dependent checks report
  `not_measured("forge unsupported")`.
- REST only. GraphQL would save requests where a collection needs one call per item, but
  it needs a token even for a public repository, and the token is optional here; REST
  costs more of the `forge_max_requests` budget and works anonymously.

## Consequences

- GitLab and other forges are deferred (§7).
- Forge data is part of the deterministic snapshot, so a replayed run makes no
  network calls.
