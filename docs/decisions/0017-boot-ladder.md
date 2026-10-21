# 0017 The boot ladder replaces the container ladder

Status: accepted · Date: 2026-10-21

## Context

The plan booted apps through a container ladder (Dockerfile, devcontainer, Buildpacks, an agent
reading the README). ADR-0003 removed the sandbox, leaving the real question: how does substance
start a repository it has never seen?

## Decision

- Three rungs, in order: README shell blocks under install/setup/getting-started/run headings, then
`substance.toml [app]`, then the stack adapter's detected commands.
- Probes boot from rung 2, or rung 3 without `[app]`; the README rung is the fallback, and when it
is the rung that worked the outcome says the app may be a dev server.
- The README rung runs in its own export (`work-readme/`), so a half-finished install cannot dirty
the copy the other rungs use. It measures O02 only: whether the repo's own instructions work.
- Boot steps never download a tool: `npx` fetches a missing package, so it is unused. npm and yarn
run `node_modules/.bin/<bin>`, pnpm `exec`, bun `x --no-install`. Every failed rung is friction.

## Consequences

B28 reports `not_measured` when the README rung booted the app: a dev server carries the markers it
looks for. M4-T02's probes and M10's test command read the trait; interactive steps fail the rung.
