# 0001 Name `substance`

Status: accepted · Date: 2026-10-21

## Context

The plan needs one name for the binary, the packages and the artifacts it writes.
The owner kept the working name, so v1 uses it everywhere rather than inventing a
product name and an internal one.

## Decision

- The binary is `substance`.
- Each crate is the package `substance-<dir>` under `crates/<dir>`.
- Nothing is published: `publish = false` in `[workspace.package]`.

## Consequences

- Paths, package names, commit messages, report fields and docs all carry the same
  word from M0 on.
- A later rename would touch every crate, the driver and the generated schemas;
  the name is treated as stable.
