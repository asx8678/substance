# 0006 Agent layer off

Status: accepted · Date: 2026-10-21

## Context

Principle 3 lets agents explore, label and propose, but every verdict must come
from a deterministic oracle. An LLM in the pipeline would add cost, latency,
network access and non-determinism to a tool promising "same input, same verdict".

## Decision

- The layer is off in v1: `crates/agent` is a stub behind the `agent` cargo
  feature, which is off by default.
- The crate defines only the provider boundary — `AgentProvider::propose`, plus
  `AgentJob`, `UntrustedText`, `AgentArtifact` and `AgentError` — and ships no
  implementation.
- A01–A04, M12, UI-level B08 and build-ladder rung 4 are deferred (§7).

## Consequences

- No check needs network access, an API key or a model version, and no manifest
  records one.
- Repo text stays untrusted data (principle 12); when the layer lands, a proposal is
  an input to review, never a verdict.
