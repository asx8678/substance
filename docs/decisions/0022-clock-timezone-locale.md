# 0022 Clock, timezone and locale

Status: accepted · Date: 2026-10-21

## Context

The inputs must be frozen before `run_id`, or two hosts share an id while seeing different clocks.

## Decision

- Browser: `clock.install({ time: as_of })` before the first navigation, `timezoneId: "UTC"`,
  `locale: "en-US"`. Not a fixed instant: that stops every timer a page schedules.
- Server, where `libfaketime.so.1` is found on Linux: `LD_PRELOAD`,
  `FAKETIME="@YYYY-MM-DD HH:MM:SS"` (the `@` start-at form) and `FAKETIME_DONT_RESET=1` so a child of
  npm does not restart it; `server_clock: fixed_start`.
- Elsewhere (macOS, Windows, Linux without it): no `LD_PRELOAD` at all rather than one naming a
  missing path, `server_clock: real`, and B24's server part reports `not_measured`.
- Always: `TZ=UTC`, `LANG=en_US.UTF-8`, `LC_ALL=en_US.UTF-8` on every repo-controlled process.
- One detection, before the manifest is hashed, decides both the manifest and the environment.

## Consequences

A statically linked binary (Go) bypasses the libc symbols `libfaketime` interposes, so it sees the real
clock even on a `fixed_start` host; detecting that means inspecting every binary a repo runs, so the
limit is recorded rather than guessed. `TZ` works for any binary, so B23 is unaffected.
