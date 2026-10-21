# 0028 Tool table and snapshot keys

Status: accepted · Date: 2026-10-21

## Context

The plan fixes neither where `tools.toml` is read from nor how responses are named.

## Decision

- The table is embedded with `include_str!`, never read from the repo under test: a
  repo must not choose which binary substance runs.
- Tool versions are probed before `run_id`, so they are part of the manifest.
- A snapshot key is `sha256(method || url || body)`; files are `<kind>/<key>.json`
  echoing the request, so a file names itself.
- Saved headers come from a short allowlist, asserted at compile time to name no
  credential (`auth`, `cookie`, `token`, `secret`, `apikey`, `session`).
- Replay spends no request budget; retries are three attempts 500 ms apart, no jitter.
- `ExecPermit::grant` is the only constructor; `--snapshot-from` reaches only
  `invocation.json`.

## Consequences

- Equal requests replay for equal bytes; a token cannot reach disk by a header rename.
- A new tool version means rebuilding substance, which keeps the manifest honest.
