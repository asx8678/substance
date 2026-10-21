# 0015 Git CLI for history

Status: accepted · Date: 2026-10-21

## Context

Neither gix nor git2 offers the move and copy detection of `git blame -w -M -C` (decision 13), and H03, H08 and V01 depend on it: a line moved or copied by a refactor must keep its original author, or the big-bang ratio credits the refactor.

## Decision

- History and blame run the `git` CLI, spawned only through `run_tool_capture` (M0-T08), with `git` in `tools.toml` and no gix or git2 dependency.
- The git version found on `PATH` is recorded in the run manifest, so a verdict traces to the git that produced it.
- Output shapes are fixed by flags, never by user config: `-c core.quotePath=false`, `--no-color`, `--no-pager`, `-M -C`, and `--diff-merges=first-parent` so a merge reads as the change its branch brought.
- `git blame` has no `--use-mailmap`; identities are mapped afterwards through `git check-mailmap`, once per file.
- Parse failures are typed errors naming the command, the repository and the line; no parser panics.

## Consequences

- A host without git reports `not_measured` for every H check, with the reason in report.json.
- git's output is now an interface: an unexpected shape fails loudly instead of yielding a wrong metric.
- `blame_max_files` bounds the work by count, not wall time (§4.3, ADR-0009); the report states the files blamed against the files authored.