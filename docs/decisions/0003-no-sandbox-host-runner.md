# 0003 No sandbox: the host runner

Status: accepted · Date: 2026-10-21

## Context

The plan put a Docker sandbox, build-ladder rungs, digest-pinned images and container egress blocking under every app. The owner removed it: substance runs the app and its tools as host child processes.

## Decision

- `crates/sandbox` becomes `crates/runner`; no Docker, gVisor or Kubernetes, and everything runs in `runs/<run_id>/work/`, never in the user's checkout.
- Steps that run repo-controlled code need `--allow-exec`, enforced by an `ExecPermit`; without it those checks report `not_measured("needs --allow-exec")`.
- External tools come from `PATH` with versions recorded; a missing tool makes its checks `not_measured`.
- Egress is advisory: a logging, deny-by-default proxy, and the manifest records `egress_enforced: false`.

## Consequences

- Substance is for trusted repos; in CI the runner VM is the isolation.
- Build-ladder rungs 1–3, image-digest pinning and Trivy image scans are deferred (§7); O05 and O08 are reworked (ADR-0020).
- Traffic that ignores the proxy variables is invisible; the report says so.
