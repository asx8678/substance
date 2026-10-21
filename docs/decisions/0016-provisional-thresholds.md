# 0016 Provisional thresholds

Status: accepted · Date: 2026-10-21

## Context

H04, H05, H09, H10, H12, H16, H20 and the archetype rules need numbers. The plan sets them from the benchmark corpora (M5), which do not exist yet; leaving them out would make every such check `reported` and the archetype `unclassified`.

## Decision

- `config/thresholds.toml` holds every threshold, with `version` and `calibrated = false` until the corpora set the values. The starting numbers are the plan's.
- The file is embedded at build time, so a repository under test cannot raise its own thresholds. Its sha256 is the manifest's `thresholds_version`.
- A missing key means "no threshold": the check reports metrics and neither passes nor fails (ADR-0013). A misspelt key is a parse error, never a silent removal.
- An archetype rule whose number is missing cannot match, and H22 names the metrics that were missing when that blocked every rule.

## Consequences

- Every thresholded verdict rests on an uncalibrated number; the report says "thresholds uncalibrated" while the flag is false.
- Calibrating is editing one file and flipping one flag; no code changes, and no check ID changes.
- Editing the file changes `thresholds_version`, so two runs with different thresholds have different run IDs.
- Every report must be read as provisional until the flag is true.
