# 0020 No containers, and the checks that follow

Status: accepted · Date: 2026-10-21

## Context
ADR-0003 runs the app on the host, so nothing here can start an image, inspect a container's user
or scan one for vulnerabilities. The catalog still has to answer O05 and O08.

## Decision

- O05 is static: read the repo's Dockerfile and judge the **final** stage. Fail when it names no
`USER`, or names `root`/`0` (case-insensitive, `name:group` judged on the name). No Dockerfile is
`not_measured`, not a pass: there is no image whose user could be wrong.
- Earlier stages are not judged. They compile; only the last one ships, and requiring a `USER` in a
builder would fail every ordinary multi-stage frontend build for no security reason.
- The finding says what the evidence supports -- "the final stage names no USER" -- not "the image
runs as root", because a hardened base or a privilege-dropping entrypoint can make it false.
- O08 (image scans) reports `not_measured("no container: substance runs the app on the host")`.
- Containers stay out of scope. ADR-0003's isolation rungs are unaffected.

## Consequences
A repository with a Dockerfile and no `USER` fails O05 while running non-root in practice, and the
note says why. A future container mode replaces this check rather than extending it.
