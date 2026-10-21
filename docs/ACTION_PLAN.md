# Substance — v1 action plan for the implementing model

2026-10-21 · owner: Adam

**Design source:** `Substance — architecture plan for a deterministic app evaluator.md` in the repo root, called "the plan" below. A quoted heading such as "Core data model" refers to a section of the plan. The online original is at https://claude.ai/artifact/Lgf5vCggP4Fhhj5er3TLxN.

**This file sets scope and order.** It records the owner's answers to the plan's open questions and the fixes for gaps found in review. Where the plan and this file disagree, this file wins.

Contents: 1 Rules · 2 Owner decisions · 3 Plan review · 4 Architecture changes for v1 · 5 Milestone map · 6 Tasks · 7 Deferred and out of scope · 8 Owner acceptance runs · 9 Progress

---

## 1. Rules for the implementing model

1. **One task at a time,** in the order of §6, respecting `Depends`. Read the plan sections a task lists under `Read`; don't re-read the whole plan for every task.
2. **No tests (owner instruction, ADR-0008).** Write production code only.
   - Don't write `#[test]`, `#[tokio::test]` or `#[cfg(test)]` modules, `tests/` or `benches/` directories, `*.test.ts` or `*.spec.ts` files, golden or fixture files, or test-only dependencies.
   - Don't run `cargo test`, `cargo nextest`, `npm test`, `vitest`, `jest`, `pytest` or the Playwright test runner. Don't run `substance run`, `substance replay` or `substance bench` against any app; those are owner acceptance runs (§8).
   - This doesn't conflict with T01–T03 or `bench/`. Those are product features that make substance run *the evaluated app's* tests and a benchmark, and they are code you write.
   - Wherever you would normally write a test for an invariant, use a compile-time assertion instead: `const _: () = assert!(…);`.
3. **Allowed commands:**
   - Cargo: `cargo fmt`, `check`, `clippy`, `build`, `add`, `metadata`, `tree`, `generate-lockfile`, and `cargo run -q -p substance-cli -- <subcommand>` for the substance subcommands listed below.
   - In `driver/`: `npm ci`, `npm install`, `npm run typecheck`, `npm run lint`, `npm run build`.
   - Git and search: `git status`, `diff`, `add`, `commit`; `rg`, `grep`, `find`, `jq`.
   - substance subcommands that inspect without evaluating: `--help`, `schema export`, `schema validate`, `checks list`, `doctor`.
   - Not allowed: `npx` (it can download packages at run time) and scaffolders (`npm create`, `npm init`, `uv init`). Scripts that need them, such as `scripts/scaffold-hashes.sh` (M1-T08), are written by you and run by the owner (§8).
4. **Dependencies:**
   - Add them with `cargo add <crate>` or `npm install --save-exact <pkg>`. Never type a version number by hand.
   - Before calling a crate API, confirm it in the docs for the version you added (docs.rs). Never guess method names.
5. **Commits:** one per task, with the message `<task id>: <summary> [<check IDs>]`, for example `M1-T05: commit-history signals [H01 H02 H03 H04 H05 H06 H07 H08 H09 H10]`. In the same commit, tick the task in §9.
6. **Check IDs are stable.** Use them in code (`CheckId::H03`), in reports and in commit messages. Never rename, renumber or reuse one. From M1-T12 on, a task that flips a check to `implemented` also adds its arm to `handler_for` and runs `C-IMPL` with its IDs.
7. **When the plan and this file are silent,** choose the more deterministic option and write an ADR using the template from M0-T02. ADR numbers 0001–0026 are reserved by this file; new ones start at 0027.
8. **Stop and report instead of improvising** when:
   - an external tool's flags or output differ from what a task says;
   - a task would need containers, an LLM call or URL mode (ADRs 0003, 0004 and 0006 forbid them);
   - a task's checks still fail after two honest fixes;
   - a check ID would have to change.
9. **Repo hygiene** (substance evaluates its own repo; see S05 and S06):
   - Never add these names: `*_SUMMARY.md`, `*_COMPLETE.md`, `FIXES_*.md`, `IMPLEMENTATION_*.md`, `debug_*`, `verify_*`, `test_fix*`, `*_old.*`, `*_v2.*`, `*.backup`.
   - Never commit build output.
10. **Done** means every **Outcome** bullet is true and every **Checks** command passes.

### Standard checks

Tasks refer to these by name.

| Name | Command | Passes when |
| --- | --- | --- |
| C-RUST | `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo build --workspace --locked` | exit 0 |
| C-AGENT | `cargo clippy -p substance-agent --features agent --locked -- -D warnings` | exit 0 |
| C-SCHEMA | `cargo run -q -p substance-cli -- schema export --out schemas && git add -N schemas driver/src/generated && git diff --exit-code -- schemas driver/src/generated` | exit 0: the regenerated files equal the staged ones, and none are untracked |
| C-GUARD | `scripts/guards.sh` | exit 0: no tests, no wall clock, no unseeded randomness, no HashMap or HashSet |
| C-TS | `npm --prefix driver ci && npm --prefix driver run typecheck && npm --prefix driver run lint` | exit 0 (from M3 on) |
| C-IMPL `<IDs>` | `cargo run -q -p substance-cli -- checks list --json --status implemented > target/impl.json` then `jq -r '.[] \| select(.handler) \| .id' target/impl.json` and `jq '[.[] \| select(.handler \| not)] \| length' target/impl.json` | the first lists every given ID; the second prints `0` (from M1-T12 on) |

Run C-SCHEMA after any change to a type in `crates/core`. Stage the regenerated `schemas/` and `driver/src/generated/` (`git add`) first; they go into the task's single commit (rule 5). C-SCHEMA compares the working tree with the index, so it fails on unstaged or untracked output. In CI the index equals HEAD, so the same command catches uncommitted drift.

---

## 2. Owner decisions for v1

The plan asks seven questions before any code is written. These are the answers as of 2026-10-21; M0-T02 records each one as an ADR.

| # | Question | Decision | ADR |
| --- | --- | --- | --- |
| 1 | Name | Keep `substance`: binary `substance`, packages `substance-*` | 0001 |
| 2 | Main use case | CI gate for code the owner trusts: the owner's repos and the output of their own coding agents. Triage budget 2 min, full run 30 min, default `--fail-on gate` | 0002 |
| 3 | Sandbox runtime | **No sandbox.** substance runs the app and the tools as host child processes. Running anything the repo controls needs `--allow-exec`. Use it on trusted repos only; in CI, the runner VM is the isolation | 0003 |
| 4 | Forges | GitHub only, behind a `Forge` trait | 0005 |
| 5 | LLM layer | Off in v1. `crates/agent` is a stub behind the `agent` cargo feature; A01–A04 and M12 are deferred | 0006 |
| 6 | URL mode | Repo mode only. `url` and `repo+url` exist in the schema, but intake rejects them | 0004 |
| 7 | License, public repo | Undecided. No LICENSE file yet; `publish = false` in every crate | 0007 |

The owner answered 3–6 directly. Decisions 1, 2 and 7 are planning defaults, and the owner can override them by editing their ADRs.

---

## 3. Plan review: issues and fixes

These issues came up while checking the plan. Each fix is built into the tasks in §6.

| # | Issue | Where in the plan | Fix |
| --- | --- | --- | --- |
| 1 | The repo copy of the plan was a truncated export. It stopped after P07, so it had no System overview, History, Provenance, Agent layer, Scoring, Benchmark, Milestones, Roadmap or Engineering conventions | repo-root file | Restored from the online doc |
| 2 | "No sandbox" drops Docker/gVisor isolation, build-ladder rungs 1–3, tool images pinned by digest and container egress blocking. It also changes O05 and O08 | "Sandbox, safety and access modes"; "Language, framework and database adapters" | Host runner (§4.1, ADR-0003); O05/O08 rework (ADR-0020) |
| 3 | The scoring rules say severity is "fixed per check in the catalog", but the catalog has no severity column | "Scoring and report" | Default severities in the registry (M0-T03, ADR-0010) |
| 4 | Destructiveness levels are missing for D, O, S, T, P, H and V checks, and B33 has a range (0–3) | catalog tables | Level rule and defaults (M0-T03, ADR-0010) |
| 5 | 16 checks feed no scoring axis: B22, B27, B29, B31, B32, I02, L02, P05, P06, S01, S03–S07 and S12 | "Scoring and report" | Axis assignment (M0-T03, ADR-0011) |
| 6 | EffectRecord IDs look like ULIDs (`rec_01H…`), which embed time and randomness. That breaks "byte-identical normalized records" and "equal manifests give equal reports". `timing.settle_ms` also varies from run to run | "Core data model"; "Determinism engineering" | Content-derived IDs; `settle_ms` left out of the normalized form (§4.3, ADR-0009) |
| 7 | An EffectRecord can't say that a channel was not measured (for example `queries` on SQLite), although principle 8 requires it | "Core data model" | A `Measured<T>` wrapper per channel (M0-T04, ADR-0014) |
| 8 | M3 promises "all channels", but the egress proxy and Mailpit only arrive in M8. B13 in M6 is a hard gate and needs the egress channel | "Milestones" | Minimal egress proxy and Mailpit move to M3 (M3-T04, ADR-0021); M8 adds stubs and faults |
| 9 | B32 (M6) needs the route table, which belongs to the Spec stage in M7 | "Milestones"; "System overview" | Route-table extraction moves to M6 (M6-T01) |
| 10 | There is no seeding or test-account strategy, yet B10, B11, B13, B14, B17 and B25 need two users per role and seeded data | "Determinism engineering" | Seeding config (M2-T04, ADR-0019) |
| 11 | Thresholds are meant to come from the benchmark corpora, but no milestone builds the corpora, while M1 already ships archetypes, H04 and H20 | "Evaluating the evaluator" | Provisional thresholds file marked `calibrated = false` (M1-T06, ADR-0016) |
| 12 | There is no project config format, though checks need one: O06 probe path, B25 retained tables, budgets, credentials, normalization rules | whole plan | `substance.toml` (M0-T05) |
| 13 | Neither gix nor git2 offers the move and copy detection of `git blame -w -M -C`, which H03, H08 and V01 need | "Tech stack and repository layout" | git CLI for history, with its version recorded (M1-T02, ADR-0015) |
| 14 | A frozen server clock (libfaketime) stalls timers and event loops. LD_PRELOAD also fails on static binaries and on macOS | "Determinism engineering" | Start-at clock that keeps advancing, Linux only; elsewhere `server_clock = real` and the affected checks report not_measured (M3-T06, ADR-0022) |
| 15 | The settle rule's "no requests in flight" never holds on pages that poll or keep sockets open | "Determinism engineering" | The in-flight count ignores idle-baseline patterns (M3-T01, M3-T05) |
| 16 | Resetting Postgres from a template needs the app disconnected and the logical slot dropped first. The plan doesn't say how the app moves to the fresh copy | "Determinism engineering" | Stop app → drop slot → drop and recreate DB from template → recreate slot → start app (M2-T03, ADR-0018) |
| 17 | Principle 10 doesn't define "independent family", or whether hints count | "Design principles" | Family = ID prefix; hints count; V, A and G never count (ADR-0012) |
| 18 | `not_measured` and `unverifiable` aren't distinguished | "Core data model" | Definitions in ADR-0013 |
| 19 | The summary matrix doesn't define "Broken now" or "Maintained" | "Scoring and report" | Definitions in M13-T01 |
| 20 | A05, V02, G01 and G02 appear in the axis table but are scheduled "Later" | "Scoring and report" | Registered as `reserved` IDs (M0-T03) |
| 21 | M7's acceptance criteria fuzz a FastAPI app, but the FastAPI adapter only arrives in M11 | "Milestones" | M7 is accepted on TS apps; the FastAPI run moves to M11 (§8) |
| 22 | S11 is listed for M1, but its "keys in built bundles" part needs a build, the same split L05 already has | "Check catalog: static, test and dependency checks" | The tree and history part goes in M1; the bundle part in M4 (M4-T04) |
| 23 | The plan's acceptance criteria and engineering conventions depend on tests: round-trip tests, oracle golden tests, probe integration tests, benchmark gates and double-run diffs in CI | "Milestones"; "Engineering conventions" | Replaced by build, lint, codegen and guard checks; test work listed in §7; owner runs in §8 |
| 24 | Dogfooding: substance judges its own repo, and with no tests it will flag T01 against itself | "Engineering conventions" | Expected. The CI dogfood job reports but doesn't gate (M1-T12) |
| 25 | No login mechanism: the crawler, B10, B14, B24, B33 and the `existing` account mode all need a logged-in session, but nothing defines how to log in | whole plan; this file | `[login]` config (M0-T05), driver `login` command (M3-T01), login detection and verification by effect (M4-T01) |
| 26 | The archetype rules use undefined metrics (`big_bang`, `active_days`, `edit_share`, `test_lines_change_with_code`, `maintainers`), and "Solo: answers issues" was dropped | M1-T06 | Metric definitions and a `solo` response condition (M1-T06) |
| 27 | Missing dependencies: H13 and H14 need registries (M1-T04), T06 needs `thresholds.toml` (M1-T06), and H17 needs S04's scaffold hashes (M1-T08) | §6 | Depends fixed; H17's scaffold match moves to M1-T08 |
| 28 | C-SCHEMA required committed output while rule 5 allows one commit per task | §1 | C-SCHEMA compares against the index (§1) |
| 29 | `--forge-snapshot` replayed only `forge/`; registry data and network-dependent tools (osv-scanner, scorecard) were fetched live again. A snapshot flag in the manifest would also change `run_id` and every finding ID | M1-T01; §8 | `--snapshot-from <run_dir>` covers forge, registry and network tools, and is kept out of the manifest (M1-T01) |
| 30 | Wall-time budgets (blame, triage, requests) make verdicts depend on machine speed, breaking "same input, same verdict" | §4.3 | Count budgets decide; wall time is only a safety cap, noted in the report when it fires (§4.3, ADR-0009) |
| 31 | The dogfood job's shallow checkout makes intake exit 2, and `tools.toml` has no exact version for CI to install | M0-T09; M1-T12 | `fetch-depth: 0`; a `pin` field in `tools.toml` (M1-T01, M1-T12) |
| 32 | Crawl-judged checks (B01, B31 and other `OracleOnly` checks) bypass the 3-of-3 rule | M4-T02 | Confirmation replays of failing crawl records (M4-T02) |
| 33 | `canonical_json` relied on serde_json's `preserve_order` staying off, which any dependency can switch on through feature unification | M0-T03 | Explicit recursive key sort (M0-T03) |
| 34 | ts-rs maps 64-bit integers to `bigint`, which `JSON.parse` never produces; seeds above 2⁵³ lose precision | M0-T04 | Seeds as strings; bounded integers as `number` (M0-T04, M0-T07) |
| 35 | libfaketime restarts the clock at `as_of` in every child process (npm → node) | M3-T06 | `FAKETIME_DONT_RESET=1` (M3-T06, M8-T05) |
| 36 | `setrlimit` from a crate only limits substance itself; limiting a child needs `pre_exec`, which `unsafe_code = "forbid"` rules out | M2-T01 | `prlimit` wrapper, Linux only (M2-T01) |
| 37 | O03 and O07 measured the `npm start` wrapper's PID, not the server | M2-T05; M8-T08 | Measure the whole process group (M2-T05, M8-T08) |
| 38 | `pg_stat_statements_reset()` is server-wide (it breaks parallel runs on a shared database) and needs extra privileges | M3-T03 | Before/after delta filtered by `dbid` (M3-T03) |
| 39 | `NODE_V8_COVERAGE` is per process, so "files hit during DB-writing actions" can't be read from one coverage run | M10-T01 | Differential coverage: boot-only run vs. write-actions run (M10-T01) |
| 40 | The AppModel was cached by commit SHA only, ignoring config, seed and driver version | M4-T01 | Full cache key (M4-T01) |
| 41 | gitleaks reads the repo's own config and ignore files, so a hostile repo can allowlist its own secrets | M1-T08 | substance's own gitleaks config; repo ignores disabled (M1-T08) |
| 42 | The handler table arrived in M6-T00, after M1–M4 checks already ran end to end through some other dispatch | M6-T00 | `handler_for` and C-IMPL start in M1-T12; M4-T02 adds probe variants |
| 43 | Many H checks have no threshold, so "pass otherwise" made them always pass | M1-T05; M1-T06 | New verdict `reported`: metrics only, neither pass nor fail (M0-T04, ADR-0013) |
| 44 | Commands the plan needs were missing from the allowed list (`cargo generate-lockfile`, `cargo run`), and S04's template hashes need scaffolders | §1; M1-T08 | Allowed list extended; scaffold hashes come from an owner-run script (§1, M1-T08, §8) |
| 45 | Level table contradictions: O01 was both L0 and L1; the reserved V02 fell under "V checks" | M0-T03 | O01 is L1; reserved IDs have no level (M0-T03) |
| 46 | Python install commands: `uv sync --frozen` differs from the plan's locked install (`--locked`); `poetry install --sync` is deprecated in Poetry 2 | M11-T01 | `uv sync --locked`; `poetry sync` (M11-T01) |
| 47 | `npx stryker run` may download packages at run time | M10-T03 | Stryker from PATH at a pinned version (M10-T03) |
| 48 | B24's `dst` instant uses Los Angeles rules, but the browser context was fixed to UTC | M8-T05 | `dst` runs in `America/Los_Angeles` (M8-T05) |
| 49 | B33 `cross_user_rows` sent only the anon key, so the request ran as `anon`, not as user B | M6-T11 | `apikey` plus `Authorization: Bearer <B's token>` (M6-T11) |
| 50 | B05's `redeploy` ran the full boot plan, which reseeds the database | M6-T02 | Redeploy skips seed and accounts (M6-T02) |
| 51 | O01, S11 and L05 were marked implemented while parts of them arrive later | M1-T08; M1-T09 | Per-part verdicts in `CheckVerdict.parts` (M0-T04, M0-T08) |
| 52 | The example `substance.toml` had one user, while ADR-0019 requires two per role | M0-T05 | Second user in the example (M0-T05) |

---

## 4. Architecture changes for v1

### 4.1 Host runner (replaces the sandbox, ADR-0003)

`crates/sandbox` in the plan's layout becomes `crates/runner`. It runs everything as processes on the host.

- **Working copy.** substance never runs code in the user's checkout. It exports the commit under test to `runs/<run_id>/work/` (`git worktree add --detach`, or `git archive` piped to `tar -x`) and runs everything there.
- **Permission.** Some steps execute code the repo controls: install, build, start, migrate, seed, the repo's own tests, and tools that load repo config (knip, eslint). All of them need `--allow-exec`.
  - The runner enforces this with an `ExecPermit` value that only intake can create, and only when `--allow-exec` is set. Every function that spawns repo-controlled code takes `&ExecPermit`.
  - Without the permit, those checks report `not_measured("needs --allow-exec")`.
  - Triage of git history, files and lockfiles never executes repo code.
- **Processes.** Each app process gets:
  - its own process group, started with `std::os::unix::process::CommandExt::process_group(0)`;
  - a clean environment: `[app.env]` from config plus substance's own variables;
  - stdout and stderr written to `runs/<run_id>/logs/`, with a byte-offset index;
  - a free port from the OS;
  - SIGTERM, then SIGKILL after the grace period (via the `nix` crate);
  - wall-time budgets per stage.
- **Databases.** substance never starts Postgres itself.
  - Postgres: substance takes an admin URL (`SUBSTANCE_PG_ADMIN_URL`; in CI, a service container) and creates a database per run.
  - SQLite: a file in the working copy.
  - Supabase: driven through the Supabase CLI, which needs Docker on the host. Without the CLI, Supabase checks report not_measured.
- **External tools** (Schemathesis, osv-scanner, gitleaks, Toxiproxy, Mailpit and the rest) come from `PATH`.
  - `tools.toml` lists each tool with its version command, the checks it serves, and whether it executes repo-controlled code.
  - The manifest records the version found.
  - A missing tool makes its checks not_measured.
  - `substance doctor` lists what is missing.
- **Egress.** Without a sandbox, substance can't block egress.
  - It points the app at a logging forward proxy (proxy environment variables plus a per-run CA) that denies by default.
  - It records `egress_enforced: false` in the manifest.
  - Traffic that ignores the proxy variables is invisible; the report says so under coverage.
- **Browser.** The TypeScript driver runs on the host with Node and Playwright's Chromium (`npm --prefix driver ci`, then `npx --prefix driver playwright install chromium`).

### 4.2 Crates

| Dir | Package | Holds |
| --- | --- | --- |
| `crates/core` | substance-core | IDs, the `CheckId` registry, EffectRecord, Finding, Verdict, Evidence, AppModel, RunManifest, Report, config types, driver protocol types, `clock`, canonical JSON and hashing. No IO except `clock` |
| `crates/orchestrator` | substance-orchestrator | Pipeline stages, scheduling, budgets, the 3-of-3 rule, the run directory |
| `crates/runner` | substance-runner | Working copy, host processes, ports, the external-tool runner, the egress proxy, Toxiproxy and Mailpit control |
| `crates/recorder` | substance-recorder | Driver host, DB/log/egress/email channels, normalization, record store, replay |
| `crates/crawler` | substance-crawler | Deterministic crawler and AppModel |
| `crates/probes` | substance-probes | Active probes, one module per check family |
| `crates/oracles` | substance-oracles | Pure oracles, one module per check family |
| `crates/history` | substance-history | Git engine, GitHub forge, registries; H, V and P checks |
| `crates/statics` | substance-statics | tree-sitter and ast-grep rule runner; S checks and the other file-based checks |
| `crates/adapters` | substance-adapters | `StackAdapter`, `DbAdapter`, the TS/Python/Rust stacks, Postgres/SQLite/Supabase, lockfile parsers, route tables |
| `crates/agent` | substance-agent | Stub traits behind the `agent` feature (off) |
| `crates/report` | substance-report | Scoring and report.json/html/sarif |
| `crates/cli` | substance-cli | The `substance` binary (clap) |

`core` depends on no other workspace crate. `oracles` depends only on `core` plus pure libraries, so it stays IO-free. `cli` depends on `orchestrator`, which depends on the rest. There are no dependency cycles.

### 4.3 Determinism in code (ADR-0009)

- **IDs are content-derived:** `<prefix>_<first 26 chars of lowercase base32(sha256(canonical parts))>`.
  - `run_id` hashes the manifest without the `run_id` field.
  - `rec` hashes (run_id, check ID, target, step index, repetition).
  - `fnd` hashes (run_id, check ID, target, sub-check).
  - `snap` hashes (run_id, snapshot name).
  - Blobs are referenced as `sha256:<hex>`.
- **Collections:** maps and sets are `BTreeMap` and `BTreeSet`. Every vector written to output is sorted by a stated key.
- **Wall clock:** only through `substance_core::clock`, which is used once, to default `as_of`. `std::time::Instant` is allowed for timeouts but is never serialized.
- **Randomness:** only `rand_chacha::ChaCha8Rng::seed_from_u64(seed)`. Each generator gets its own seed, derived as `sha256(master_seed, generator_name)` and stored in the manifest.
- **No floats in records or reports:** durations in ms, sizes in bytes, ratios in permille (`u32`).
- **Normalized form:** the normalized form of an EffectRecord omits `id` and `timing.settle_ms`. Normalized JSON is canonical JSON: keys sorted, no insignificant whitespace.
- **Budgets:** limits that can change a verdict are counts, applied in sorted order: `blame_max_files`, `forge_max_requests`, `crawl_max_*`, `probe_max_targets`. The same input then hits the same limit on any machine.
  - Wall-time budgets (`triage_s`, `run_s`, per-stage timeouts) are safety caps only.
  - When one fires, the affected checks report `not_measured("budget exhausted")`, and the report adds the note `time cap hit: <stage>; this result depends on machine speed`.
  - The "same input, same verdict" guarantee covers runs where no time cap fired.

---

## 5. Milestone map

```text
M0 Foundations
 ├─► M1 Triage, no app execution ────────────────────────────────────────────────────────────┐
 └─► M2 Runner + boot ─► M3 Recorder + driver ─► M4 Crawler + first oracles ─► M5 Bench v1     │
                                                  └─► M6 Data/trust ─► M7 API ─► M8 Resilience │
                                                       ─► M9 Front end + data model ─► M10 Test │
                                                       truthfulness ─► M11 Python + Rust packs ─┴─► M13 Scoring v1 + report
M12 Agentic layer: deferred (ADR-0006)
```

- M1 needs only M0, so it can run in parallel with M2–M4.
- M5 needs M4. M6 needs M4 but not M5.
- M13 comes last, but M0 fixes the `report.json` v0 shape, and every milestone writes into it.
- Every milestone is broken into tasks with exact files, an outcome and checks. M6–M11 share the conventions at the start of M6.

---

## 6. Tasks

Each task lists **Depends** · **Read** (plan sections) · **Do** · **Outcome** · **Checks**. C-RUST and C-GUARD apply to every task, even where a task doesn't repeat them.

### M0 Foundations

#### M0-T01 Workspace scaffold

- **Depends:** — · **Read:** "Tech stack and repository layout", "Engineering conventions"
- **Do:**
  - Cargo workspace at the repo root with the 13 crates of §4.2.
    - `[workspace.package]`: `edition = "2024"`, `publish = false`. `resolver = "3"`.
    - Each library starts with a crate doc comment and an empty module tree.
  - `rust-toolchain.toml` pinning the current stable toolchain, with the `rustfmt` and `clippy` components.
  - Lints:
    - `[workspace.lints.rust]`: `unsafe_code = "forbid"`.
    - `[workspace.lints.clippy]`: `unwrap_used`, `expect_used`, `todo`, `unimplemented`, `panic`, `dbg_macro`, `print_stdout` and `print_stderr`, all `deny`.
    - Library crates use `[lints] workspace = true`. `crates/cli` defines its own `[lints]` table with the same entries minus `print_stdout` and `print_stderr`.
  - Error handling: `thiserror` in libraries, `anyhow` only in `crates/cli`, and `tracing` for logs.
  - `crates/agent`: feature `agent`, off by default. With the feature on, the crate exposes `pub trait AgentProvider` with one method, `fn propose(&self, job: AgentJob, untrusted: UntrustedText) -> Result<AgentArtifact, AgentError>`, plus those types. There is no implementation.
  - `scripts/guards.sh`, executable, with exactly the content below.
  - `.gitignore`: add `/target`, `/runs`, `/bench-runs`, `/driver/node_modules`, `/driver/dist`.
  - Run `cargo generate-lockfile` once and commit `Cargo.lock`; C-RUST uses `--locked` and fails without it.

```bash
#!/usr/bin/env bash
# Repository guards, run by CI and by every task's checks.
# ADR-0008: no tests. ADR-0009: determinism hazards.
set -uo pipefail
command -v rg >/dev/null || { echo "guard: ripgrep (rg) is required"; exit 2; }
fail=0
check() { # $1 = message; the remaining args are a command that must find nothing
  local msg=$1; shift
  "$@"; local rc=$?
  if [ $rc -eq 0 ]; then echo "guard: $msg"; fail=1
  elif [ $rc -gt 1 ]; then echo "guard: error while checking: $msg"; fail=1; fi
}
roots=()
for d in crates driver; do [ -d "$d" ] && roots+=("$d"); done
[ ${#roots[@]} -eq 0 ] && exit 0
check "test attributes are not allowed (ADR-0008)" \
  rg -n --glob '*.rs' '#\[(tokio::)?test\]|#\[cfg\(test\)\]' "${roots[@]}"
check "tests/, benches/ and __tests__/ directories are not allowed (ADR-0008)" \
  sh -c 'find "$@" -type d \( -name tests -o -name benches -o -name __tests__ \) -not -path "*/node_modules/*" | grep -q .' _ "${roots[@]}"
check "test files are not allowed (ADR-0008)" \
  sh -c 'find "$@" \( -name "*.test.ts" -o -name "*.spec.ts" \) -not -path "*/node_modules/*" | grep -q .' _ "${roots[@]}"
check "wall clock outside core/src/clock.rs (ADR-0009)" \
  rg -n --glob '*.rs' --glob '!**/core/src/clock.rs' 'SystemTime::now|Utc::now|Local::now|OffsetDateTime::now|Timestamp::now|Zoned::now' "${roots[@]}"
check "unseeded randomness (ADR-0009)" \
  rg -n --glob '*.rs' 'thread_rng|rand::random|from_entropy|OsRng|rand::rng\(' "${roots[@]}"
check "HashMap/HashSet: use BTreeMap/BTreeSet (ADR-0009)" \
  rg -n --glob '*.rs' '\bHash(Map|Set)\b' "${roots[@]}"
exit $fail
```

- **Outcome:** 13 crates build. `target/debug/substance --help` runs. The guard script exists.
- **Checks:** C-RUST, C-AGENT, C-GUARD. `cargo metadata --format-version 1 --no-deps | jq '.packages | length'` prints `13`.

#### M0-T02 ADRs for decisions and review fixes

- **Depends:** M0-T01 · **Read:** §2, §3 and §4 of this file
- **Do:**
  - Write the template `docs/decisions/0000-template.md` with the headings `# NNNN Title`, `Status: accepted · Date: YYYY-MM-DD`, `## Context` (1–3 sentences), `## Decision` (1–5 bullets) and `## Consequences` (what reports not_measured, what is deferred).
  - Write ADRs 0001–0014, at most 25 lines each:
    - 0001 Name `substance`
    - 0002 Primary use case and default budgets
    - 0003 No sandbox: the host runner (§4.1)
    - 0004 Repo mode only
    - 0005 GitHub only
    - 0006 Agent layer off
    - 0007 License undecided
    - 0008 No tests by owner instruction, listing everything in §7
    - 0009 Determinism rules in code (§4.3), including count budgets vs. wall-time caps
    - 0010 Severity and level defaults (the tables in M0-T03)
    - 0011 Axis assignment for unassigned checks (M0-T03)
    - 0012 Slop label: family = ID prefix; hints count; a slop label needs fails from at least 2 families among B I D O L S T P H; V, A and G never count
    - 0013 Verdict semantics:
      - `not_measured`: the check couldn't run or lacked an input (missing tool, adapter or permission; a crash; disabled; out of budget).
      - `unverifiable`: the check ran, but the evidence can't decide. Example: H01 with commit dates that predate the repo.
      - `reported`: the check ran and recorded metrics, but has no threshold yet (uncalibrated or context-only, such as H19). It is neither pass nor fail, counts as measured for coverage, and never lowers a grade.
      - Neither `not_measured` nor `unverifiable` counts as pass, and both carry a reason.
    - 0014 `Measured<T>` channels in EffectRecord, a deviation from the plan's JSON sample
  - Later tasks add 0015–0024.
- **Outcome:** 15 files in `docs/decisions/`.
- **Checks:** `ls docs/decisions | wc -l` prints `15`. `rg --files-without-match '^Status:' docs/decisions` prints nothing.

#### M0-T03 Core: IDs, enums and the check registry

- **Depends:** M0-T01 · **Read:** every "Check catalog" section; "Repo history and vitality"; "Provenance"; "The small agentic layer"; "Scoring and report"
- **Do:**
  - `crates/core/src/ids.rs`:
    - newtypes `RunId`, `RecordId`, `FindingId` and `SnapshotId` over `String`, each with `derive(parts: &[&[u8]]) -> Self` as in §4.3;
    - `BlobRef`, holding `sha256:<hex>`.
  - `crates/core/src/hash.rs`: sha256 and base32 helpers, and `canonical_json<T: Serialize>(&T) -> Result<Vec<u8>, CanonicalJsonError>` (fallible, so a serialization error can never hash as a placeholder; `canonical_json_value(&Value) -> Vec<u8>` is infallible). It serializes to `serde_json::Value`, then writes it with its own recursive writer that sorts every object's keys by their UTF-8 bytes. It must not rely on serde_json's map order: any dependency can turn on `preserve_order` through feature unification.
  - `crates/core/src/clock.rs`: the only wall-clock access in the workspace, `pub fn wall_now_utc() -> Timestamp`. `Timestamp` is a newtype over the `time` crate's `OffsetDateTime`, serialized as RFC 3339 UTC with whole seconds.
  - `crates/core/src/check.rs`:
    - `enum CheckId` with every ID, serialized as its string (`"B01"`): B01–B33, I01–I04, D01–D04, O01–O11, L01–L05, S01–S12, T01–T06, P01–P07, H01–H22, V01–V03, A01–A05, G01–G02. That's 114 IDs. Also `const COUNT: usize = 114`.
    - Enums:

      | Enum | Values |
      | --- | --- |
      | `Family` | the ID's prefix letter |
      | `Severity` | critical, high, medium, low, info |
      | `Confidence` | confirmed, flaky, hint |
      | `Level` | L0, L1, L2, L3 |
      | `Axis` | works_now, honest, robust, safe, data_integrity, operable, maintainable, vitality, provenance, generic |
      | `Need` | repo, exec, forge, registry, browser, db, egress, mail, faults, clock, spec |
      | `Milestone` | M1 … M13, later |
      | `CheckStatus` | planned, implemented, reserved, support |
      | `GateRule` | none, always, sub_checks(&'static [&'static str]) |

  - `crates/core/src/registry.rs`:
    - `pub const REGISTRY: &[CheckMeta]`, one entry per `CheckId` in ID order. Each entry has `id`, `title` (from the catalog), `family`, `level: Option<Level>`, `severity`, `axes: &[Axis]`, `gate: GateRule`, `needs: &[Need]`, `milestone`, `status` and `default_confidence`.
    - Add `pub const fn meta(id: CheckId) -> &'static CheckMeta`.
    - Add compile-time assertions that `REGISTRY.len() == CheckId::COUNT` and that `REGISTRY[i].id as usize == i` for every `i`, using a `const fn` with a `while` loop.
    - Fill the registry from the catalog plus the tables below.

**Severity defaults (ADR-0010, uncalibrated):**

| Severity | Checks |
| --- | --- |
| critical | B02, B05, B10, B11, B13, B33, L01, L05, S11 |
| high | B01, B03, B04, B06, B07, B08, B09, B12, B14, B15, B16, B17, B25, B27, B28, B32, D03, D04, I01, I02, L03, L04, O01, O02, O06, P01, P02, S12, T01 |
| medium | B18, B19, B20, B21, B23, B24, B26, B30, B31, D01, D02, I04, L02, O03, O04, O05, O07, O08, O09, O10, O11, P04, P06, S08, T02, T03, T04, T05 |
| low | B22, B29, P03, P05, P07, S01–S07, S09, S10, T06 |
| info | H01–H22, V01, V03; also I03 and the 8 reserved IDs, as a placeholder |

**Levels (ADR-0010).** B, I and L checks take the catalog's level; a range takes its maximum, so B33 is L3. For everything else:

| Level | Checks |
| --- | --- |
| L0 | H, S, P and V checks (not the reserved V02); T04–T06; D01, D02, D04; O08, O09, O10 |
| L1 | O01, O02, O04, O05, T01–T03 |
| L2 | D03 |
| L3 | O03, O06, O07, O11 |
| none | I03 and the 8 reserved IDs |

In v1 the level is recorded but doesn't gate anything, because only repo mode exists.

**Axes.** Use the plan's axis table, plus these assignments (ADR-0011):

| Check(s) | Axis |
| --- | --- |
| B22, B31 | works_now |
| B27, B29, S12 | honest |
| B32 | works_now, safe |
| I02 | works_now, safe |
| L02 | robust |
| P05 | maintainable |
| P06 | operable |
| S01, S03–S07 | none: hints. They are shown under maintainable, count toward the slop label and attach as `cause` |

**Gates:**

| Gate | Checks |
| --- | --- |
| `always` | B02, B10, B11, B13, B33, L01, L05, S11 |
| `sub_checks(&["restart", "redeploy"])` | B05 |
| `sub_checks(&["ignored_auth"])` | I01 |
| `none` | everything else |

**Status:**

| Status | Checks |
| --- | --- |
| `reserved` | V02, A01–A05, G01, G02 |
| `support` | I03 |
| `planned` | all others; a task flips its checks to `implemented` |

**Default confidence:** `hint` for S01–S10, S12, D04, P05 and T04–T06; `confirmed` for everything else. The 3-of-3 rule sets `flaky` at runtime.

- **Outcome:** a compile-time-checked registry of 114 IDs.
- **Checks:**
  - C-RUST.
  - `rg -c 'id: CheckId::' crates/core/src/registry.rs` prints `114`.
  - `rg -n 'f32|f64|HashMap' crates/core/src` prints nothing.

#### M0-T04 Core: data model types

- **Depends:** M0-T03 · **Read:** "Core data model", "Driver protocol and effect recorder", "Determinism engineering"
- **Do:** in `crates/core/src/model/`, with every type deriving `Serialize`, `Deserialize`, `schemars::JsonSchema` and `ts_rs::TS`, using `#[serde(rename_all = "snake_case")]`, and adding `deny_unknown_fields` on everything that is read from disk:
  - `measured.rs`:

    ```rust
    #[serde(tag = "status")]
    enum Measured<T> {
        Measured { value: T },
        NotMeasured { reason: String },
    }
    ```

  - `effect.rs`: `EffectRecord`, following the plan's JSON with these changes:
    - New fields: `check_id`, `target`, `repetition`, and `setup: Vec<Action>`. `setup` holds the steps (login, navigation) that run on a fresh snapshot before `action`; replay needs it.
    - `Measured<…>` wrappers on `ws`, `db`, `queries`, `egress`, `email` and `logs.server_errors`.
    - `ui` carries `dom_hash_before` and `dom_hash_after` next to `dom_changed`.
    - Signals are `{kind: success|error, source: role_status|role_alert|aria_live|toast|config_selector, selector}`.
    - `net` entries carry `failed: Option<String>` for aborted requests.
    - Body fields are `Option<BlobRef>`.
  - `finding.rs`:
    - `Finding { id, check_id, sub_check: Option<String>, severity, confidence, title, target, evidence: Vec<Evidence>, replay: String, cause: Option<Cause>, related: Vec<FindingId> }`.
    - `Evidence` variants: record, blob, commit, file_range, forge_object, registry_object, tool_output, note.
  - `verdict.rs`:
    - `Verdict`: pass, fail, flaky, reported, not_measured{reason}, unverifiable{reason} (ADR-0013).
    - `Target` variants: repo, file, route, screen, control, endpoint, table, dependency.
    - `CheckVerdict { check_id, target, verdict, finding_ids, metrics: BTreeMap<String, i64>, parts: BTreeMap<String, Verdict> }`. `parts` holds per-part verdicts for checks built in stages (O01, S11, L05) and is empty otherwise.
  - **64-bit integers:** ts-rs maps `u64` and `i64` to `bigint`, which `JSON.parse` never produces.
    - Seeds and any value that can exceed 2⁵³ serialize as decimal strings (`#[serde(with = …)]` plus `#[ts(type = "string")]`).
    - Bounded values (ms, bytes, counts, permille) use `u32`, or `#[ts(type = "number")]` when they need 64 bits.
  - `app_model.rs`: `AppModel`, as in the plan. Users carry role and handle only, never secrets.
  - `manifest.rs`: `RunManifest` with the plan's fields plus:
    - `schema_version`, `substance_version`, `run_id`, `repo { origin, commit_sha }`;
    - `master_seed`, `seeds: BTreeMap<String, u64>`, `frozen_clock`, `server_clock` (`fixed_start` or `real`), `timezone`, `locale`;
    - `tools: BTreeMap<String, ToolVersion>`, `models` (empty in v1);
    - `rule_pack_hash`, `thresholds_version`, `scoring_weights_version`, `config_hash`;
    - `allow_exec`, `egress_enforced`, `repetitions` (3), `host { os, arch }`.
  - `report.rs`: `Report` with:
    - `schema_version: "0.1.0"`, `manifest`;
    - `verdicts`, sorted by (check_id, target);
    - `findings`, sorted by id;
    - `coverage { checks_total, checks_measured, routes, controls, server_code }`, where the last three are `Option<Ratio>` in permille;
    - `archetype: Option<Archetype>`, `provenance: Option<Provenance>`, `notes: Vec<String>`.
  - `protocol.rs`: the driver protocol from the plan's command table. `DriverCommand` is tagged `cmd`, `DriverEvent` is tagged `event`, and `RecordPart` holds the browser channels of one action.
  - `normalize.rs`: `pub fn normalized_bytes(r: &EffectRecord) -> Result<Vec<u8>, CanonicalJsonError>`, which is canonical JSON without `id` and `timing.settle_ms`.
  - `schemas/examples/effect-record.json`: the plan's sample, rewritten to the new shape. It is documentation, and M0-T07 validates it.
- **Outcome:** all model types compile, carry schema and TS derives, and contain no floats.
- **Checks:** C-RUST. `rg -n 'f32|f64' crates/core/src` prints nothing.

#### M0-T05 Project config (`substance.toml`)

- **Depends:** M0-T04 · **Read:** "Determinism engineering", plus the O06 and B25 rows of the catalog
- **Do:**
  - Add `ProjectConfig` to `crates/core/src/config.rs`, with serde, `JsonSchema`, `deny_unknown_fields` and defaults for every key.
  - Write `docs/substance.example.toml` with this shape:

```toml
[app]                         # explicit commands (boot ladder rung 2, ADR-0017)
install = "npm ci"
build = "npm run build"
start = "npm start"           # long-running; substance sets the port variable
migrate = "npx prisma migrate deploy"
seed = "npx prisma db seed"
port_env = "PORT"
ready_path = "/"
health_path = "/health"       # O06
env = { NODE_ENV = "production" }

[db]
engine = "postgres"           # postgres | sqlite | supabase
url_env = "DATABASE_URL"      # the variable the app reads
sqlite_path = "prisma/dev.db"
retained_tables = ["audit_log"]   # B25

[[users]]                     # at least two per role (ADR-0019)
role = "user"
handle = "user_a"
email = "a@example.test"
password_env = "SUBSTANCE_USER_A_PASSWORD"   # secrets only by env var name
create = "api"                # api | sql | supabase | ui | existing

[[users]]
role = "user"
handle = "user_b"
email = "b@example.test"
password_env = "SUBSTANCE_USER_B_PASSWORD"
create = "api"

[login]                       # M4-T01; omit it to detect a single password form
ui = { path = "/login", identifier = "input[name=email]", password = "input[type=password]", submit = "button[type=submit]" }
# or: api = { method = "POST", path = "/api/login", body = '{"email":"{email}","password":"{password}"}' }

[signup]
api = { method = "POST", path = "/api/users", body = '{"user":{"email":"{email}","password":"{password}","username":"{handle}"}}' }

[mail]
env = { SMTP_HOST = "{smtp_host}", SMTP_PORT = "{smtp_port}" }

[signals]                     # extra success/error selectors (stand-in for agent labels)
success = []
error = []

[normalize]
patterns = [{ regex = "inv-[0-9]{6}", replace = "<invoice-no>" }]

[egress]
allow = []                    # hosts allowed at runtime

[budgets]
triage_s = 120
run_s = 1800
settle_quiet_ms = 500
settle_timeout_ms = 10000
idle_baseline_ms = 5000
shutdown_grace_s = 30
crawl_max_screens = 200
crawl_max_actions = 2000
crawl_max_depth = 6
blame_max_files = 5000
forge_max_requests = 3000

[checks]
disable = []                  # reported as not_measured("disabled by config")
```

  - Loading order: `--config <path>`, then `<repo>/substance.toml`, then defaults. The manifest records `config_hash`. Every check disabled by config shows up as not_measured.
- **Outcome:** the config type exists, and `docs/substance.example.toml` parses into it.
- **Checks:** C-RUST. Once M0-T07 is done: `substance schema validate --kind config docs/substance.example.toml` exits 0. It converts the TOML to JSON before validating.

#### M0-T06 CLI skeleton

- **Depends:** M0-T05 · **Read:** "Tech stack and repository layout" (the CLI line), "Scoring and report" (outputs)
- **Do:** clap derive in `crates/cli`:

```text
substance run <REPO> [--rev <sha>] [--out <dir>] [--as-of <rfc3339>] [--seed <u64>] [--config <path>]
                     [--triage-only] [--allow-exec] [--snapshot-from <run_dir>]
                     [--format json,html,sarif] [--fail-on gate|critical|high|none]
substance replay <RECORD_OR_FINDING_ID> --run <dir>
substance bench [--suite <dir>] [--runs <n>] [--baseline <file>]
substance report <RUN_DIR> [--format json,html,sarif]      # re-render or re-judge stored data
substance schema export --out <dir>
substance schema validate --kind <effect-record|finding|manifest|report|config|injection> <file>
substance checks list [--json] [--status planned|implemented|reserved|support]
substance doctor [--json]
```

  - Exit codes: 0 ok; 1 the `--fail-on` threshold was hit; 2 a usage, config or intake error (including URL input); 3 an internal error.
  - Subcommands not built yet print `not implemented yet (Mx)` and exit 3.
  - `checks list` works now, reading `REGISTRY`.
- **Outcome:** every subcommand parses, and `checks list` prints the registry.
- **Checks:**
  - C-RUST.
  - `cargo run -q -p substance-cli -- checks list --json | jq length` prints `114`.
  - `cargo run -q -p substance-cli -- checks list --json --status reserved | jq length` prints `8`.

#### M0-T07 Schema and TypeScript type generation

- **Depends:** M0-T06 · **Read:** "Core data model" (the first paragraph)
- **Do:**
  - `schema export` writes:
    - `schemas/json/<name>.schema.json` (schemars) for EffectRecord, Finding, CheckVerdict, AppModel, RunManifest, Report, ProjectConfig, DriverCommand, DriverEvent, RecordPart and Injection (Injection is defined in M5-T01; add it then);
    - `driver/src/generated/*.ts` (ts-rs), inside the driver's own tree so `tsc` can compile them without reaching outside `driver/`. `schema export` writes there no matter what `--out` says.
  - Call ts-rs's programmatic export (`T::export_all_to(dir)`; confirm the exact name for your version). Don't rely on ts-rs's export during `cargo test`.
  - Output must be byte-stable: sorted keys, pretty-printed, trailing newline.
  - `schema validate` validates a JSON or TOML file with the `jsonschema` crate and prints each error with its JSON pointer.
- **Outcome:** `schemas/` and `driver/src/generated/` are committed and reproducible.
- **Checks:** C-SCHEMA. `cargo run -q -p substance-cli -- schema validate --kind effect-record schemas/examples/effect-record.json` exits 0. `rg -n 'bigint' driver/src/generated` prints nothing.

#### M0-T08 Orchestrator skeleton, run directory, report.json v0

- **Depends:** M0-T07 · **Read:** "System overview", "Determinism engineering", "Sandbox, safety and access modes" (access modes)
- **Do:**
  - `crates/orchestrator`:
    - a `Stage` enum: intake, triage, runtime, explore, spec, probe, judge, report;
    - `RunContext` holding manifest, config, run directory and budgets;
    - `pub async fn run(opts) -> Result<Report, RunError>`.
  - Intake:
    - Accept a local directory or a git URL. Clone with the git CLI, full clone only.
    - Spawn git only through `run_tool_capture(name, args, cwd, timeout)`, which you add now in `crates/runner/src/tool.rs`. It returns stdout in memory and saves nothing; it's meant for high-volume plumbing such as git. M1-T01 adds `run_tool`, which saves output as blobs.
    - Process spawning lives only in `crates/runner`. Add this guard to `scripts/guards.sh`, just before `exit $fail`:

      ```bash
      check "process spawning outside crates/runner" \
        rg -n --glob '*.rs' --glob '!**/runner/src/**' 'Command::new' "${roots[@]}"
      ```
    - Refuse shallow repos: if `git rev-parse --is-shallow-repository` prints `true`, exit 2.
    - Resolve the commit SHA, default `as_of` to `clock::wall_now_utc()`, and fill the manifest.
    - Derive `run_id` and reject every access mode except repo.
  - Run directory `runs/<run_id>/`:

```text
manifest.json  report.json  report.html  report.sarif
blobs/sha256/<2 hex>/<64 hex>          # content-addressed, written once
records/<rec_id>.json  findings/<fnd_id>.json
forge/  registry/  tools/               # raw external responses, saved with as_of
logs/                                   # app, driver and tool logs, with offset index
snapshots/<snap_id>/                    # SQLite copies, work-copy archives, template names
work/                                   # working copy (M2)
app_model.json  crawl.har  spec/        # M4, M7
```

  - Every `planned` or `implemented` check that doesn't run gets a not_measured verdict with a reason: `not implemented (M4)`, `needs --allow-exec`, `tool missing: gitleaks`, `disabled by config` or `budget exhausted`. Reserved and support IDs get no verdict.
  - Checks built in parts (O01, S11, L05) report each part not built yet in `parts` as `not_measured("not implemented (Mx)")`. Overall verdict: `fail` if any part fails, else `not_measured` if any part is, else `pass`.
  - `crates/report` writes `report.json` as canonical JSON, pretty-printed.
- **Outcome:** the run pipeline exists, and every v1 check has a not_measured path from day one (principle 8).
- **Checks:** C-RUST, C-SCHEMA.

#### M0-T09 CI workflow

- **Depends:** M0-T08 · **Read:** "Engineering conventions"
- **Do:**
  - `.github/workflows/ci.yml`, triggered on push and pull_request, running on `ubuntu-latest`. One job, `rust`, runs C-RUST, C-AGENT, C-GUARD and C-SCHEMA. M3 adds `driver` and M1-T12 adds `dogfood`.
  - No test steps.
  - Pin every action by its full commit SHA, with the tag in a trailing comment. Look up each SHA with `git ls-remote https://github.com/<owner>/<action> refs/tags/<tag>`; never invent one.
- **Outcome:** CI runs build, lint, guards and schema drift.
- **Checks:**
  - `rg -n 'cargo test|nextest|npm test|continue-on-error' .github/workflows` prints nothing.
  - Every `uses:` line matches `@[0-9a-f]{40}`.

### M1 Triage without app execution

Plan target: under 2 minutes after cloning, and the same `as_of` snapshot gives an identical report.

#### M1-T01 Tool runner and snapshots of external data

- **Depends:** M0-T08 · **Read:** "Determinism engineering" (history snapshots, pinned versions)
- **Do:**
  - `tools.toml` at the repo root. Each `[tools.<name>]` entry has:
    - `bin` and `version_args`;
    - `min_version`, the floor that `doctor` checks, and `pin`, the exact version CI installs;
    - `checks = [...]`;
    - `exec_repo_code = bool`;
    - `network = bool`, true when the tool's output depends on online data (osv-scanner, scorecard, `npm audit`, `cargo audit`, pip-audit).
  - `crates/runner/src/tool.rs`: `run_tool(name, args, cwd, timeout) -> Result<ToolOutput, ToolError>`.
    - The version is captured once per run and written to the manifest.
    - stdout and stderr go to blobs. An index entry `tools/<name>/<sha256(args with paths relative to the repo root)>.json` points at them.
    - A missing binary returns `ToolError::Missing`, and the caller reports not_measured.
    - A tool with `exec_repo_code = true` requires `&ExecPermit`.
    - git already goes through `run_tool_capture` (M0-T08). List `git` in `tools.toml` so its version is recorded too.
  - `crates/history/src/snapshot.rs`: `SnapshotHttp`, wrapping `reqwest` with rustls.
    - Every request and response is saved as `forge/<sha256(method+url+body)>.json` or under `registry/`, together with `as_of`. Auth headers are never saved.
    - `--snapshot-from <run_dir>` replays an earlier run's external data without network:
      - `forge/` and `registry/` responses;
      - the saved outputs of every `network = true` tool, which is then not executed.
      A missing entry makes the check not_measured.
    - The flag is not part of the manifest, so `run_id`, finding IDs and `report.json` stay identical. It is recorded in `runs/<run_id>/invocation.json`, which no hash covers.
    - Retries use a fixed backoff (no jitter). Each run stops after `forge_max_requests` requests, issued in a fixed order (§4.3).
  - All "last N days" windows are computed from `manifest.as_of`.
- **Outcome:** external data is replayable from disk, and tool versions are recorded.
- **Checks:** C-RUST, C-SCHEMA.

#### M1-T02 Git engine (ADR-0015)

- **Depends:** M1-T01 · **Read:** "Repo history and vitality" (the two rules)
- **Do:** in `crates/history/src/git/`, wrap the `git` CLI, spawned only through `run_tool_capture` (M0-T08), with `git` listed in `tools.toml`. There's no gix or git2; the manifest records the git version.
  - `commits()`: parse `git log --use-mailmap --numstat -M -C --no-color` with a fixed `--format`. Each commit gives SHA, parents, author and committer name, email and date, subject, body, trailers (via `%(trailers)`), and per-file added and deleted counts with renames.
  - `hunks(sha)`: parse `git show -U0 --no-color -M -C <sha>`. H08, H09 and T05 use this.
  - `blame(path)`: parse `git blame -w -M -C --line-porcelain <sha> -- <path>` into a map from line to (commit, author, author time).
    - Run it in parallel over authored files with a bounded worker pool (number of CPUs).
    - It blames at most `blame_max_files` files, in sorted path order, and records blamed-line coverage.
  - `authored.rs` excludes these from authored lines:
    - lockfiles, from a list;
    - paths with `linguist-generated` or `linguist-vendored`, via `git check-attr`;
    - files with `@generated` in their first 5 lines;
    - `vendor/`, `third_party/` and `node_modules/`;
    - minified files: `*.min.*`, or any line longer than 500 characters;
    - snapshots: `__snapshots__/`, `*.snap`;
    - fixtures: `fixtures/`, `testdata/`;
    - i18n JSON under `locales/` or `i18n/`;
    - images, fonts and media, by extension.
    Whitespace-only changes are ignored via `-w`.
  - Bots: names or emails containing `[bot]`, plus GitHub accounts whose `type` is `Bot` when forge data exists.
  - ADR-0015 records why the git CLI was chosen.
- **Outcome:** a typed, sorted history model plus blame.
- **Checks:** C-RUST. `rg -n '^(gix|git2)' crates/history/Cargo.toml` prints nothing.

#### M1-T03 GitHub forge client (ADR-0005)

- **Depends:** M1-T01 · **Read:** "Repo history and vitality" (H01, H11–H19)
- **Do:**
  - A `Forge` trait in `crates/history/src/forge/`, with a GitHub implementation using REST, and GraphQL where it saves calls. It goes through `SnapshotHttp`. The token comes from `GITHUB_TOKEN` and is optional; without it, rate-limited calls report not_measured.
  - Data to fetch:

    | Object | Fields |
    | --- | --- |
    | repo | created_at, fork and parent, template_repository, default branch, license |
    | PRs | created, merged, merged_by, author, merge method if known, reviews (author, state, submitted_at), comment count |
    | issues and comments | author login, author account created_at, created_at, author_association |
    | other | workflow runs (head_sha, conclusion, created_at), releases, tags, stargazers with `starred_at` (header `Accept: application/vnd.github.star+json`; capped at the 5,000 most recent), contributors |

  - Paging is deterministic and every list is sorted.
  - A non-GitHub origin returns `ForgeUnavailable`, which makes forge checks `not_measured("forge unsupported")`.
- **Outcome:** forge data is available offline from the snapshot.
- **Checks:** C-RUST.

#### M1-T04 Registries and lockfiles

- **Depends:** M1-T01 · **Read:** "Check catalog: static, test and dependency checks" (P), "Language, framework and database adapters" (Detect, Locked install)
- **Do:**
  - `crates/history/src/registry/` clients, all through `SnapshotHttp`:

    | Registry | Data |
    | --- | --- |
    | npm | packument: versions, `time`, `deprecated`, `repository`; downloads API for the last month |
    | PyPI | JSON API: releases, `upload_time`, `yanked`, `project_urls` |
    | crates.io | versions with `created_at`, `yanked`, repository, downloads; requires a `User-Agent` header |
    | deps.dev v3 | dependents count |

  - `crates/adapters/src/{ts,python,rust}/deps.rs` parse lockfiles: `package-lock.json` (v2 and v3), `pnpm-lock.yaml`, `yarn.lock` (v1 and berry), `bun.lock`, `Cargo.lock`, `uv.lock`, `poetry.lock` and `requirements*.txt`. Only `==` pins count as locked.
    - They output a sorted `Dependency { ecosystem, name, version, direct, dev, manifest_path }`.
  - Date added: walk each manifest file's history once (`git log -p -- <manifest>`) and take the first commit that adds each direct dependency.
- **Outcome:** dependency lists with registry metadata and dates added.
- **Checks:** C-RUST.

#### M1-T05 History signals H01–H10

- **Depends:** M1-T02, M1-T03 · **Read:** "Repo history and vitality"
- **Do:**
  - Implement each signal as a triage check in `crates/history/src/signals/`. Each one writes integer metrics to `CheckVerdict.metrics`, and the top 20 contributing commits as evidence.
  - Verdict:
    - `fail` when its threshold from M1-T06's file is crossed;
    - `pass` when below the threshold;
    - `reported` when the check has no key in `thresholds.toml` (metrics only, until calibrated; ADR-0013);
    - `not_measured` when an input is missing;
    - `unverifiable` where the formula says so.
  - Formulas:

| ID | Formula |
| --- | --- |
| H01 | Server-side first dates (repo created_at, first PR, issue, CI run, release) against commit dates. Commits more than 1 day older than the repo → `unverifiable("imported or backdated history")`, plus the count |
| H02 | Distinct ISO weeks with at least one non-bot commit or merged PR in the 90 and 365 days before `as_of` |
| H03 | Permille of blamed authored HEAD lines whose commit falls on the UTC calendar day of the first non-bot commit |
| H04 | Median and p90 (nearest rank) of authored added lines per commit; permille of HEAD lines from commits over 1,000 authored lines; a separate metric for giant commits after day one |
| H05 | Per commit: authored added lines ÷ max(1, minutes since the same author's previous commit); p90, and the count above threshold |
| H06 | Commits whose subject is exactly "Add files via upload": count and authored lines |
| H07 | Chains of 3 or more commits by one author, each within 30 min of the last, touching overlapping files, subject matching `(?i)\bfix`, with a failing CI run between them |
| H08 | From hunks: moved-line permille (a removed line of 20+ trimmed chars re-added elsewhere in the same commit); copy-paste permille (an added line repeated 2+ times in one commit); median age in days of lines deleted in the last 200 commits |
| H09 | Permille of lines added in the last 365 days that a later commit deletes, identically and in the same file, within 7 days |
| H10 | Share of commits whose whole subject, case-insensitive, is in `rules/history/generic-messages.txt` (update, fix, wip, changes, minor, cleanup, stuff, commit, `.`); also the count of those on diffs over 300 authored lines |

- **Outcome:** H01–H10 are `implemented` in the registry.
- **Checks:** C-RUST. `cargo run -q -p substance-cli -- checks list --json --status implemented | jq -r '.[].id'` includes H01–H10.

#### M1-T06 History signals H11–H22, archetypes, thresholds (ADR-0016)

- **Depends:** M1-T05, M1-T04 · **Read:** "Repo history and vitality" (the whole section, including false positives)
- **Do:**

| ID | Formula |
| --- | --- |
| H11 | Distinct non-bot humans who merged PRs or own blamed HEAD lines, deduplicated with mailmap plus GitHub login. Bus factor = the fewest authors covering at least 50% of blamed authored lines |
| H12 | Permille of merged PRs with an APPROVED review from someone other than the author; count of rubber stamps (merged under 60 s after creation with no reviews or comments) |
| H13 | Median hours to the first maintainer response on outsiders' issues and PRs (last 365 days); median days to merge security-labelled dependency PRs; libyear in permille-years; CI conclusion at HEAD |
| H14 | Outside issue authors whose accounts were 180+ days old when they opened the issue (cap 50 user lookups); deps.dev dependents; registry downloads |
| H15 | Fail when the repo has at least 3 issues and only the owner or author accounts ever opened or commented on them |
| H16 | CHANGELOG versions (`## [x.y.z]` or `## x.y.z`) with no tag or release; README badges pointing at workflow files that don't exist, or static `img.shields.io/badge/` images claiming build or coverage; "production-ready", "battle-tested" or "enterprise-grade" with active weeks below threshold |
| H17 | When `template_repository` is set: permille of files whose blob SHA equals the template's (via the API's recursive tree). The match against S04's scaffold hashes is added in M1-T08 |
| H18 | Days with 50+ stars where at least 50% of the stargazers' accounts were under 30 days old (cap 200 lookups). Never a positive signal |
| H19 | Repos the owner created in the 30 days around this repo's creation. Context only; info |
| H20 | Authored lines per active person-day, where a person-day is a distinct (author, UTC date) |
| H21 | `scorecard --repo=github.com/<o>/<r> --format=json` from PATH (needs `GITHUB_AUTH_TOKEN`). Import Maintained, Code-Review, Contributors, Branch-Protection, CI-Tests, Dangerous-Workflow and Pinned-Dependencies as 0–10 integers |
| H22 | Archetype: H01 unverifiable → Unverifiable history. Otherwise the first match in this order: One-shot dump, Burst then abandoned, Team maintained, Solo maintained, Young and iterating. No match → `unclassified`. The report shows the metrics that triggered it |

  - Archetype metrics. Each is computed once, stored in H22's metrics, and taken from the check named:

    | Metric | Definition |
    | --- | --- |
    | `age_days` | `as_of` − the first non-bot commit (or the first public push, under the private-then-public rule), in days |
    | `big_bang` | H03's permille |
    | `active_weeks_90`, `active_weeks_365` | H02 |
    | `active_days` | distinct UTC dates with a non-bot commit, over the repo's life |
    | `edit_share` | permille of non-bot commits after day one that delete or change at least one existing authored line (numstat deleted > 0) |
    | `test_lines_change_with_code` | permille of non-bot commits changing authored non-test files that also change test files. Test paths come from `rules/history/test-paths.txt` (`tests/`, `__tests__/`, `*.test.*`, `*.spec.*`, `test_*.py`, `*_test.py`) |
    | `outside_issue_authors` | distinct non-bot issue authors whose `author_association` isn't OWNER, MEMBER or COLLABORATOR |
    | `bus_factor` | H11 |
    | `maintainers` | H11's distinct non-bot humans who merged a PR in the last 365 days |
    | `reviewed_prs` | H12's permille |
    | `answers_issues` | true when H13's median first response is ≤ `solo_max_response_hours` (default 168), or when there were no outsider issues in the last 365 days |

    A condition on a not_measured metric is false. When no rule matches only because of that, the result is `unclassified`, with a note naming the metric.
  - False positives, from the plan:
    - **Squash merges:** if at least 70% of merged PRs were squashed, H02 and H04 use PR dates and sizes.
    - **Forks:** exclude commits reachable from upstream, using the forge compare API.
    - **Private, then public:** if the repo's created_at is more than 30 days after the first commit, H03 and H04's "day one" becomes the first public push day, with a note.
    - **Bus factor 1** is a flag only.
  - `config/thresholds.toml` holds `version`, `calibrated = false` and the values below; its sha256 goes into the manifest as `thresholds_version`. The HTML report prints "thresholds uncalibrated" while `calibrated` is false. Starting values, provisional:

| Key | Value |
| --- | --- |
| `one_shot` | big_bang ≥ 700‰ AND active_weeks_365 ≤ 3 AND outside_issue_authors = 0 |
| `burst_abandoned` | age ≥ 120 days AND active_weeks_90 = 0 |
| `young_iterating` | age < 90 days AND active_days ≥ 15 AND edit_share ≥ 400‰ AND test_lines_change_with_code ≥ 300‰ |
| `solo` | active_weeks_90 ≥ 2 AND bus_factor = 1 AND answers_issues |
| `team` | maintainers ≥ 2 AND reviewed_prs ≥ 500‰ |
| H04 | giant commit = 1,000 lines; fail when share_from_giant ≥ 500‰ |
| H05 | fail when p90 ≥ 100 lines/min |
| H07 | fail when chains ≥ 3 |
| H09 | fail when ≥ 250‰ |
| H10 | fail when ≥ 500‰ |
| H12 | fail when reviewed < 200‰ and maintainers ≥ 2 |
| H20 | fail when median ≥ 2,000 lines per person-day over ≥ 14 active days |

- **Outcome:** H11–H22 are implemented, and the archetype appears in `report.json`.
- **Checks:** C-RUST, C-SCHEMA. `checks list --status implemented` includes H11–H22.

#### M1-T07 Static rule engine

- **Depends:** M1-T02 · **Read:** "Check catalog: static, test and dependency checks" (the intro)
- **Do:**
  - `crates/statics`: walk files from `git ls-files -z` at the commit, filtered by `authored.rs`. Detect the language by extension.
  - Load ast-grep YAML rules from `rules/<lang>/*.yml` with `ast-grep-config`, and match them with `ast-grep-core` and `ast-grep-language` (TypeScript, Tsx, JavaScript, Python, Rust). Confirm each API on docs.rs first.
  - Each rule carries `metadata.check_id` and `metadata.sub_check`.
  - Output is a hint-confidence finding with `file_range` evidence, sorted by (path, line, rule id).
  - `rule_pack_hash` = sha256 over the sorted rule files and their contents.
  - Files over 1 MB are skipped and counted.
- **Outcome:** a rule runner that every static check reuses.
- **Checks:** C-RUST.

#### M1-T08 Static checks S01–S12

- **Depends:** M1-T07 · **Read:** the S table
- **Do:**

| ID | Implementation |
| --- | --- |
| S01 | Run aislop, antislop and AI-SLOP Detector, whichever are on PATH (confirm their flags with `--help`), and parse their output. None present → not_measured |
| S02 | `rules/{ts,rust,python}/escape-hatches.yml` with the catalog's patterns. Rust `unwrap()`/`expect()` count only inside functions that take Axum or Actix extractor parameters. Metric: count per 1,000 authored lines |
| S03 | Rules for: TS DB-client calls (insert, update, delete, create, upsert, save, execute, query) that aren't under `await`, `return` or `.then`; Rust `let _ = $Q.execute($$$)`; Python `except: pass` in a `try` that calls `commit()` |
| S04 | `rules/scaffolds.toml`: sha256 of files from official templates (create-vite, create-next-app, uv init) at a recorded version, written by `scripts/scaffold-hashes.sh`. You write the script; the owner runs it (§8). Until then the file holds only the text markers, and the hash part is `not_measured` in `parts`. Also add H17's match against these hashes. Plus the catalog's text markers (`vite-project`, "Add your description here", `Vite + React + TS`, `[year] [fullname]`) |
| S05 | The catalog's filename patterns: report-style markdown in the root, one-off scripts, duplicate versions |
| S06 | Tracked paths under `node_modules/`, `target/`, `__pycache__/` or `dist/`, plus `.env` (not `.env.example`) and `.DS_Store` |
| S07 | Two configs for one tool (for example `.eslintrc*` alongside `eslint.config.*`); `"strict": false` in tsconfig; `#![allow(warnings)]` in a crate root; dependency sets that differ between `pyproject.toml`, `setup.py` and `requirements.txt` |
| S08 | Commands in README `sh`/`bash`/`shell`/`console` blocks that don't exist (npm, pnpm or yarn scripts; make and just targets; cargo bins; referenced paths). Environment variables read in code (`process.env.X`, `import.meta.env.X`, `os.environ[...]`, `os.getenv`, `std::env::var`, `env!`) but missing from the README and `.env.example` |
| S09 | TS interfaces or abstract classes with exactly one `implements`; Rust traits with exactly one `impl … for`; functions whose body only returns a call with the same arguments |
| S10 | Number of files importing a DB client (prisma, drizzle, knex, typeorm, pg, sqlx, diesel, sea_orm, sqlalchemy, django.db, psycopg) |
| S11 | `gitleaks git` (full history) and `gitleaks dir` (tree), JSON report. Evidence keeps rule, file, line, commit and fingerprint only, never the secret. Run gitleaks with substance's own config (`rules/gitleaks.toml`, gitleaks' default rules at the pinned version) and with the repo's `.gitleaks.toml` and `.gitleaksignore` disabled, so a repo can't allowlist its own secrets. Confirm the flags with `--help`, and stop and report if repo ignores can't be disabled. Confirmed. The bundle part is in M4-T04 |
| S12 | Patterns from `rules/evaluator-manipulation.txt` in README, comments, HTML and markdown: text addressed to AI reviewers, evaluators, graders or "language models", "ignore previous instructions", requests for a high score or rating, hidden HTML comments addressed to an assistant. Confirmed |

- **Outcome:** S01–S12 are implemented, apart from S11's bundle part.
- **Checks:** C-RUST. `checks list --status implemented` includes S01–S12.

#### M1-T09 Other triage checks: D04, O01 (static), O10, T04–T06, L05 (repo)

- **Depends:** M1-T07, M1-T04, M1-T06 · **Read:** the D04, O01, O10, T04–T06 and L05 rows
- **Do:**

| ID | Implementation |
| --- | --- |
| D04 | Rules for: `prisma db push` in start, dev or postinstall scripts, a Procfile or a Dockerfile CMD; TypeORM `synchronize: true`; Sequelize `sync({ force: true })`; SQLAlchemy `drop_all(` at startup; `DROP TABLE` in startup code; a SQLite path under `/tmp` |
| O01 (static) | Lockfile missing for a detected ecosystem; direct dependencies absent from the lockfile; requirements without `==` pins. The exec part is in M2-T05 |
| O10 | Parse `.github/workflows/*.yml` with a maintained YAML crate. Flag test steps with `\|\| true`, `continue-on-error: true`, `--passWithNoTests`, or a trailing `exit 0`, and a package.json `test` script that exits 0 without running tests |
| T04 | Per framework: test count, skip markers (`it.skip`, `test.skip`, `xit`, `describe.skip`, `@pytest.mark.skip`, `#[ignore]`), assertions per test, and the share of weak matchers (`toBeTruthy`, `toBeDefined`) |
| T05 | From hunks: commits that touch non-test files and also delete assertion lines, change expected literals in test files, or add skip markers, without adding test lines |
| T06 | Fail when at least 70% of today's test lines (by blame) arrived after the point in history where 80% of today's non-test lines existed. Both thresholds go in `thresholds.toml` |
| L05 (repo) | Variable names matching `^(VITE_\|NEXT_PUBLIC_\|PUBLIC_\|REACT_APP_\|EXPO_PUBLIC_).*(OPENAI\|ANTHROPIC\|GEMINI\|MISTRAL\|GROQ\|COHERE\|REPLICATE).*(KEY\|TOKEN\|SECRET)` in tracked env files and code; provider key patterns in client source (`sk-`, `sk-ant-`, `AIza`). Confirmed |

- **Outcome:** these checks are implemented.
- **Checks:** C-RUST. `checks list --status implemented` includes D04, O01, O10, T04, T05, T06 and L05.

#### M1-T10 Dependency checks P01–P07

- **Depends:** M1-T04, M1-T06 · **Read:** the P table
- **Do:**

| ID | Implementation |
| --- | --- |
| P01 | `osv-scanner` JSON over the lockfiles: one tool for every ecosystem, and it executes no repo code. Add `npm audit`, `cargo audit` and `pip-audit` when present; pip-audit only with `--allow-exec`, since it can build packages |
| P02 | Package not found in its registry; first published less than 30 days before the project added it; no source repo; downloads and version count below thresholds |
| P03 | Per direct dependency: was the version added already a major version behind the latest release on the day it was added? Fail when the stale share is at least 500‰ |
| P04 | npm `deprecated` on the version in use, or a yanked release on PyPI or crates.io |
| P05 | Dependencies per 1,000 authored lines; overlap groups from `rules/overlap.toml` (HTTP clients, date libraries, state managers, per language); unused dependencies via knip (needs `--allow-exec`), cargo-machete and deptry |
| P06 | Project license (LICENSE text hash or the manifest's `license` field) against dependency licenses from registry metadata, using `rules/licenses.toml`. Missing licenses are flagged. Use cargo-deny, license-checker or pip-licenses when present |
| P07 | Forge-only history (H02, H11, H13, plus the archetype without blame) for each direct dependency's GitHub repo; first 50 by name, with coverage recorded. Abandoned = no commit or release in 365 days |

- **Outcome:** P01–P07 are implemented.
- **Checks:** C-RUST. `checks list --status implemented` includes P01–P07.

#### M1-T11 Provenance V01, V03

- **Depends:** M1-T02, M1-T04 · **Read:** "Provenance"
- **Do:**
  - V01 reads declared provenance using patterns in `rules/provenance/declared.toml` (data, not code):
    - commit trailers: Claude Code's trailer and session links, Codex's `Co-authored-by`, Aider's "(aider)" author name and model co-author;
    - bot authors, and coding-agent branch prefixes taken from PR head refs;
    - the instruction files the plan lists.
    It maps them onto HEAD lines with blame, giving the share of lines per tool and per model.
  - V03 estimates the knowledge window from two inputs:
    - the release dates of the dependency versions at the time they were added;
    - `rules/provenance/api-vintage.yml`, ast-grep patterns for outdated APIs (for example `openai.ChatCompletion.create`), each with the date range in which it was current.
    Output: a window, with its evidence.
  - Neither check is ever scored. The panel follows the plan's "What the panel shows" list.
- **Outcome:** V01 and V03 are implemented, and the provenance block appears in `report.json`.
- **Checks:** C-RUST, C-SCHEMA.

#### M1-T12 Report v0 and triage end to end

- **Depends:** M1-T05 through M1-T11 · **Read:** "Scoring and report" (outputs), "Engineering conventions" (dogfooding)
- **Do:**
  - `--triage-only` runs intake, then the triage checks, then the report.
    - Independent checks run concurrently, but their results are sorted before writing.
    - When the triage time cap (`triage_s`) fires, the remaining checks report `not_measured("budget exhausted")`, and the report adds the time-cap note (§4.3).
  - `crates/orchestrator/src/handlers.rs`:
    - `pub enum Handler { Triage(TriageFn) }`; M4-T02 adds the probe variants.
    - `pub fn handler_for(id: CheckId) -> Option<Handler>`, written as one exhaustive `match` over `CheckId`, filled for every check implemented in M1.
    - The orchestrator dispatches checks only through `handler_for`.
  - `checks list --json` adds `"handler": true|false` per entry. C-IMPL applies from this task on.
  - `report.html` v0 is built with minijinja from templates in `crates/report/templates/`, embedded via `include_str!`. It is self-contained: inline CSS, no JavaScript, no CDN. Sections:
    - summary: verdict counts per axis, hard gates hit;
    - archetype with its metrics;
    - findings table: check, severity, confidence, title, evidence links;
    - the not_measured list with reasons;
    - the provenance panel, labelled "not scored";
    - the manifest.
  - Exit code follows `--fail-on`.
  - CI: add a `dogfood` job.
    - It checks out with `actions/checkout` and `fetch-depth: 0`, because intake refuses shallow repos.
    - It installs the M1 tools from `tools.toml`, each at its `pin` version.
    - It runs `substance run --triage-only --fail-on none --format json,html .` and uploads `runs/` as an artifact.
    - No `continue-on-error`; O10 would flag it.
- **Outcome:** triage works end to end on a repo, without executing repo code.
- **Checks:** C-RUST, C-SCHEMA, `C-IMPL` with every M1 ID. `rg -n '<script|https?://' crates/report/templates` prints nothing.

### M2 Runner and boot (host processes)

#### M2-T01 Working copy and process supervisor

- **Depends:** M0-T08 · **Read:** §4.1 of this file; "Sandbox, safety and access modes" (quotas)
- **Do:**
  - `crates/runner`: `WorkCopy::create(repo, sha, run_dir)`.
  - `ExecPermit`, as described in §4.1.
  - `AppProcess::spawn(spec, &ExecPermit)`:
    - clean environment, process group;
    - logs with an offset index (`logs/index.jsonl`);
    - a free port, by binding port 0 and releasing it.
  - `wait_ready`: HTTP GET on `ready_path` until a status below 500 comes back, or the budget runs out.
  - `stop(grace)`: SIGTERM to the group, wait, then SIGKILL.
  - `ServiceProcess::spawn(spec)` for substance's own long-running helpers (the driver, Mailpit, Toxiproxy): the same process-group, log and stop handling, optional piped stdin and stdout, and no `ExecPermit`.
  - Stage budgets (safety caps, §4.3).
  - Resource limits, Linux only: repo-controlled commands are wrapped as `prlimit --as=<bytes> --nofile=<n> --nproc=<n> -- <cmd>` (util-linux, listed in `tools.toml`). Elsewhere no limits are applied, and the report notes it. `setrlimit` from a crate only limits substance itself, and `pre_exec` needs `unsafe`.
- **Outcome:** repo code can only run through `ExecPermit`, and every process starts from `crates/runner`.
- **Checks:** C-RUST, C-GUARD (its process-spawning guard from M0-T08 covers this).

#### M2-T02 StackAdapter, TypeScript stacks, boot ladder (ADR-0017)

- **Depends:** M2-T01, M1-T04 · **Read:** "Language, framework and database adapters"
- **Do:**
  - `crates/adapters/src/stack.rs`:

```rust
pub trait StackAdapter: Send + Sync {
    fn id(&self) -> &'static str;                                     // "ts-next", "ts-vite", "ts-node-server"
    fn detect(&self, repo: &RepoView) -> Option<Detection>;           // with evidence
    fn boot_plan(&self, repo: &RepoView, cfg: &ProjectConfig) -> BootPlan; // install/build/migrate/seed/start, env, port wiring
    fn migrations(&self, repo: &RepoView) -> Option<MigrationTool>;
    fn route_table(&self, repo: &RepoView) -> Measured<RouteTable>;   // filled in M6
    fn test_command(&self, repo: &RepoView) -> Option<CommandSpec>;  // filled in M10
}
```

  - Package manager comes from the lockfile, and installs use the plan's locked-install commands.
  - Production mode first:

    | Stack | Boot |
    | --- | --- |
    | Next.js | `next build`, then `next start -p $PORT` |
    | Vite | `vite build`, then `vite preview --port $PORT --strictPort`; Supabase when `supabase/config.toml` exists |
    | Express/Hono | the build script if present, then `npm start` with `PORT` |

  - Migration tool detection: Prisma, Drizzle, TypeORM and Knex, using the plan's commands.
  - Boot ladder, replacing the container ladder (ADR-0017):
    1. **README rung.** Extract the shell blocks under install, setup, getting-started or run headings, and run them in order in a fresh working copy. This rung *measures O02 only*.
    2. **`substance.toml [app]` commands.**
    3. **Adapter-detected commands.**
    - Probes boot from rung 2, or rung 3 when there is no config. When only the README rung works, probes use it, and B28 reports not_measured because it may be running a dev server.
    - Every failed rung, and every fix substance had to apply, is logged as friction evidence.
- **Outcome:** TS stacks are detected, and boot plans are produced.
- **Checks:** C-RUST.

#### M2-T03 DbAdapter: Postgres, SQLite, Supabase (ADR-0018)

- **Depends:** M2-T01 · **Read:** "Language, framework and database adapters" (database adapters), "Determinism engineering" (reset)
- **Do:**
  - `crates/adapters/src/db.rs`: an async `DbAdapter` trait (`async-trait`) with:
    - `preflight` → capabilities;
    - `provision(run_id)` → `DbHandle` plus the app's connection URL;
    - `snapshot(name)`;
    - `reset(snapshot)`, callable only while the app is stopped. Enforce this by taking a `&StoppedApp` token;
    - `begin_capture` / `end_capture`, filled in M3;
    - `query_count`, filled in M3;
    - `catalog`, filled in M9;
    - `teardown`.
  - Postgres, using `tokio-postgres`:
    - Databases are named `substance_<first 12 chars of run_id>` and `…_seeded`.
    - Reset sequence: drop the logical slot → `DROP DATABASE … WITH (FORCE)` → `CREATE DATABASE … TEMPLATE …_seeded` → recreate the slot.
    - Preflight checks `wal_level = logical`, then tries a temporary slot with `wal2json` and falls back to `test_decoding`, which ships with Postgres. It also checks `pg_stat_statements` in `shared_preload_libraries` and the CREATEDB privilege.
    - Anything missing makes that channel not_measured.
    - Teardown drops slots and databases, including on Ctrl-C through a drop guard.
  - SQLite (`rusqlite` with the `backup` feature): snapshot = an online backup while the app is stopped. Reset = copy the file back and remove `-wal` and `-shm`.
  - Supabase: `supabase start` and `supabase status -o json` from PATH. Reset = `supabase db reset`, then reseed through M2-T04. ADR-0018 records that this is slower.
- **Outcome:** per-run databases with snapshot and reset.
- **Checks:** C-RUST.

#### M2-T04 Seeding and test accounts (ADR-0019)

- **Depends:** M2-T02, M2-T03 · **Read:** "Determinism engineering"; the B10, B13, B17 and B25 rows
- **Do:**
  - Order: migrate → the repo's seed command (from config, or detected: `prisma db seed`, `npm run seed`, `supabase/seed.sql`, `manage.py loaddata`) → accounts → snapshot `seeded`.
  - Accounts come from `[[users]]`, at least two per role.
  - Account creation methods:
    - `api`: the `[signup]` request template;
    - `sql`;
    - `supabase`: the GoTrue admin API with the local service key;
    - `existing`: the account must already exist after seeding; M4-T01 verifies it by logging in;
    - `ui`: implemented in M4.
  - Verification emails go through Mailpit once M3 exists.
  - Generated data uses seeded generators. Canary values for B13 are derived from the seed and written to `canaries.json`.
  - Bulk seeding (B17) is M8's job.
- **Outcome:** a deterministic `seeded` snapshot with test accounts.
- **Checks:** C-RUST, C-SCHEMA.

#### M2-T05 Operability checks O01 (exec), O02–O05 (ADR-0020)

- **Depends:** M2-T04 · **Read:** the O table
- **Do:**

| ID | Implementation |
| --- | --- |
| O01 | Exit status of the locked install in a fresh working copy |
| O02 | Outcome of the README rung, plus the friction log |
| O03 | SIGTERM to the app's group. Pass when every process in the group has exited within the grace period and the group leader's exit code is 0 or 143. A wrapper such as `npm start` exiting while its child lives on fails |
| O04 | Run the migrate command a second time; fail on a non-zero exit |
| O05 | No container, so a static check (ADR-0020): fail when the final stage of a repo Dockerfile has no `USER`, or runs as `root` or `0`. No Dockerfile → not_measured |

- **Outcome:** O01–O05 are implemented.
- **Checks:** C-RUST. `checks list --status implemented` includes O01–O05.

#### M2-T06 `substance doctor`

- **Depends:** M2-T03 · **Read:** §4.1 of this file
- **Do:** report on the host:
  - git version; Node and npm; whether Playwright Chromium is installed;
  - Postgres: admin URL reachable, `wal_level`, logical decoding plugin, `pg_stat_statements`, CREATEDB, leftover `substance_%` databases;
  - `sqldiff`; libfaketime (Linux only); `GITHUB_TOKEN` present;
  - every tool in `tools.toml`: found, version, minimum version.
  Print a table, or JSON with `--json`. Exit 0 always; doctor is informational.
- **Outcome:** a one-command preflight check.
- **Checks:** C-RUST. `cargo run -q -p substance-cli -- doctor --json | jq 'type'` prints `"object"`.

### M3 Recorder and driver

#### M3-T01 TypeScript driver

- **Depends:** M0-T07 · **Read:** "Driver protocol and effect recorder", "Determinism engineering" (settle)
- **Do:**
  - `driver/package.json`: `"type": "module"`, exact versions of `playwright`, `@axe-core/playwright`, `typescript`, `eslint` and `typescript-eslint`. Scripts: `build`, `typecheck` (`tsc --noEmit`) and `lint`.
  - `tsconfig.json`: `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`.
  - `eslint.config.js`: `no-floating-promises`, `no-misused-promises` and `no-explicit-any` are errors. Ignore `src/generated/`.
  - Commit `driver/package-lock.json`; C-TS uses `npm ci`, which needs it.
  - Protocol types are imported from `./generated/`, written by `schema export` (M0-T07). No hand-written copies.
  - `src/main.ts`: read JSON lines from stdin and handle one command at a time. Write JSON-line events to stdout and nothing else; logs go to stderr.
  - Commands follow the plan's table. `open_context` also receives the normalization rules, the blob directory, success and error selectors, and idle patterns.
  - **Settle:** an in-page MutationObserver plus an in-flight request counter that ignores idle patterns.
    - Settled = nothing in flight and no DOM mutation for `settle_quiet_ms`.
    - Cap: `settle_timeout_ms`; hitting it sets `timed_out: true`.
  - **Signals:**
    - `role=status` → success; `role=alert` → error;
    - `aria-live` polite → success; assertive → error;
    - toast containers and their type attributes from `rules/signals.toml` (sonner, react-toastify, radix, chakra);
    - `[signals]` config selectors.
    - No English word lists.
  - **Bodies:** hashed with `node:crypto` sha256 and written once to `<blob_dir>/sha256/<2>/<64>`; events return the hash.
  - **Storage:** diff localStorage, sessionStorage, cookies (`context.cookies()`) and IndexedDB (database and store names plus record counts).
  - **WebSocket** frames (direction and payload hash). Dialogs are auto-dismissed and recorded; downloads are saved as blobs.
  - **`route`:** `context.route` with the modes `fail_500`, `abort` and `delay`.
  - **`clock`:** `context.clock.install({ time })`, which keeps advancing, and `fastForward`.
  - **`login`** (add the variant to `DriverCommand` in core first, then run C-SCHEMA):
    - `{ mode: "ui", path, identifier, password, submit, email, password_value }` fills and submits the form;
    - `{ mode: "api", method, path, body }` sends the request with `page.request`, so cookies land in the context.
    - It returns the auth cookies and storage keys that changed.
    - The password never appears in events or logs. The login request body is stored as a blob whose password value is replaced with `<redacted>`.
  - **`snapshot_dom`:** the normalized body HTML → sha256, plus a visible-text map.
  - **`axe`, `layout`, `trace`** as in the plan.
- **Outcome:** the driver builds and type-checks against the generated types.
- **Checks:**
  - C-TS.
  - `rg -n --glob '!**/generated/**' ':\s*any\b|as any' driver/src` prints nothing.
  - `rg -n --glob '!**/generated/**' '(interface|type) (DriverCommand|DriverEvent|RecordPart)\b' driver/src` prints nothing.
  - Add a `driver` job running C-TS to `ci.yml`.

#### M3-T02 Driver host in Rust

- **Depends:** M3-T01, M2-T01 · **Read:** "Driver protocol and effect recorder"
- **Do:** in `crates/recorder/src/driver.rs`:
  - spawn `node driver/dist/main.js` with `ServiceProcess` (M2-T01), stdin and stdout piped;
  - line framing with a timeout per command;
  - on a crash, restart once; after that, browser channels report not_measured;
  - driver stderr goes to `logs/driver.log`.
- **Outcome:** a typed async client for the driver.
- **Checks:** C-RUST.

#### M3-T03 Recorder channels and record assembly

- **Depends:** M3-T02, M2-T03 · **Read:** "Driver protocol and effect recorder" (the channel table)
- **Do:**

| Channel | Implementation |
| --- | --- |
| DB rows | Postgres: `pg_logical_slot_get_changes` after settle; parse wal2json v2 or test_decoding output into per-table insert, update and delete counts, plus a canonical rows blob. SQLite: online backup before and after, then `sqldiff` from PATH, parsed into counts |
| Query count | `sum(calls)` from `pg_stat_statements`, filtered by the app database's `dbid`, read before the action and after settle; the count is the difference. No reset: a reset is server-wide and would break parallel runs. If `pg_stat_statements_info.dealloc` changed between the two reads (entries were evicted), the channel is `not_measured("pg_stat_statements evicted")`. substance's own queries run over an admin connection to the `postgres` database, so they aren't counted. SQLite → `not_measured("sqlite: v1")` |
| Server logs | Slice the log files by byte offsets between action start and settle. Error lines are counted with `rules/log-errors.toml`: level fields, `Traceback`, `panicked at`, unhandled rejections |
| Egress, email | From M3-T04 |

  - The recorder merges these channels with the driver's `RecordPart`, normalizes the result (M3-T05) and writes `records/<rec_id>.json` as canonical JSON.
- **Outcome:** complete EffectRecords, with not_measured where a channel is unavailable.
- **Checks:** C-RUST, C-SCHEMA.

#### M3-T04 Egress proxy and Mailpit (ADR-0021)

- **Depends:** M2-T01 · **Read:** "Determinism engineering" (close the outside world)
- **Do:**
  - **Proxy:** a forward proxy in `crates/runner/src/egress/`, built on `hudsucker` or an equivalent MITM crate, with a per-run CA from `rcgen`.
    - It logs host, method, path, status, bytes, and request and response body blobs.
    - It denies by default (502 with `denied: true`). `[egress] allow` lists exceptions.
  - **App environment:** `HTTP_PROXY`, `HTTPS_PROXY` and their lowercase forms; `NO_PROXY=localhost,127.0.0.1,::1` plus the DB host; `NODE_EXTRA_CA_CERTS`, `NODE_USE_ENV_PROXY=1`, `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE`.
  - The manifest records `egress_enforced: false`. The ADR names the clients that ignore these variables, so their traffic is invisible.
  - **Mailpit** (binary from PATH): start it on free SMTP and HTTP ports. The app's SMTP variables come from `[mail] env`. Add a client for `/api/v1/messages` and message details.
  - Stubbed responses are added in M8.
- **Outcome:** the egress and email channels exist from M3 on.
- **Checks:** C-RUST.

#### M3-T05 Normalization and idle baseline

- **Depends:** M3-T03 · **Read:** "Determinism engineering" (normalize; subtract the idle baseline)
- **Do:**
  - `rules/normalize.toml` default patterns:
    - ISO-8601 timestamps; epoch numbers in fields named `*_at`, `*time*` or `ts`;
    - UUID v1–v7, ULID, and nanoid-shaped values in id-named fields;
    - CSRF fields and meta tags; `nonce=` values;
    - hashed asset names; Next.js build IDs.
  - Each match is replaced with a fixed token (`<ts>`, `<uuid>`). Project overrides come from `[normalize]`.
  - Rust is the source of truth for the rules and sends them to the driver. Raw bodies are stored as blobs, with their normalized hashes next to them.
  - Idle baseline: for each screen, settle, then watch for `idle_baseline_ms` with no action. Record request patterns (method plus a path template with IDs replaced) and socket pings as `idle_patterns`. These are excluded from records and from the settle in-flight count.
- **Outcome:** normalized records that are stable across runs.
- **Checks:** C-RUST.

#### M3-T06 Clock, timezone and locale (ADR-0022)

- **Depends:** M3-T02 · **Read:** "Determinism engineering" (freeze the inputs)
- **Do:**
  - Browser: install the clock at `as_of`; context `timezoneId: "UTC"`, `locale: "en-US"`.
  - Server, on Linux with libfaketime: `LD_PRELOAD=<path>/libfaketime.so.1`, `FAKETIME="@<as_of>"` and `FAKETIME_DONT_RESET=1`. The `@` (start-at) form keeps time advancing, and `FAKETIME_DONT_RESET` stops each child process (npm → node) from restarting the clock at `as_of`. Then `server_clock: fixed_start`.
  - Elsewhere: `server_clock: real`, and server-clock parts such as B24's report not_measured.
  - Always: `TZ=UTC`, `LANG=en_US.UTF-8`, `LC_ALL=en_US.UTF-8`.
- **Outcome:** inputs are fixed and recorded in the manifest.
- **Checks:** C-RUST, C-SCHEMA.

#### M3-T07 Replay

- **Depends:** M3-T05, M3-T06 · **Read:** "Driver protocol and effect recorder" (replay)
- **Do:**
  - `substance replay <id> --run <dir>` loads the record, or a finding's first record, and reruns it:
    - restore the snapshot: DB reset plus the working copy's data directories;
    - boot, open a context with the record's seed and options;
    - run `setup`, then `action`.
  - It writes a new record and compares the normalized bytes. It prints a unified diff and exits 0 if identical, 1 if different.
- **Outcome:** any record can be replayed.
- **Checks:** C-RUST.

### M4 Crawler and first oracles

#### M4-T01 Deterministic crawler

- **Depends:** M3-T07, M2-T04 · **Read:** "Core data model" (AppModel), "Determinism engineering" (stable selectors)
- **Do:**
  - BFS from `/`, and from each role's post-login landing page.
  - **Login** (used here and by B10, B14, B24, B33 and the `existing` account mode):
    - From `[login]` when configured. Otherwise, the first screen in frontier order with exactly one visible password input and one submit control in the same form becomes the `ui` login.
    - Success is judged by effect, not by words: an auth cookie or storage key appears, no error signal shows, and the landing page has no password input.
    - Without a working login, every check that needs a session reports `not_measured("no working login")`.
  - Frontier order: (depth, normalized route, control order).
  - Route templates: numeric, UUID and digit-bearing segments become `:id`.
  - Per screen, `list_controls`. Actions are sorted by (role, accessible name, selector).
  - Forms are filled by seeded generators according to type, name, pattern, min, max and required.
  - Logout is detected by effect, not by words. When an action clears auth cookies or storage, mark the control as logout, log in again, and skip it from then on.
  - External links, `mailto:` and downloads are recorded, never followed.
  - Budgets come from `[budgets]`.
  - Every crawl action produces an EffectRecord.
  - Selectors: role plus accessible name, then test ID, then CSS. A selector that stops matching triggers re-exploration of that screen, never a failure.
  - Outputs: `app_model.json` (screens, controls, forms, request map, entities from JSON arrays of objects with IDs, tables touched) and `crawl.har`. The AppModel is cached per (commit SHA, `config_hash`, `master_seed`, `rule_pack_hash`, driver package version, Playwright version).
  - The `ui` signup method from M2-T04 is implemented here.
- **Outcome:** an AppModel and crawl records.
- **Checks:** C-RUST, C-SCHEMA.

#### M4-T02 Oracle framework and the 3-of-3 rule

- **Depends:** M4-T01 · **Read:** "Design principles" 3–6, "Determinism engineering" (verdict rule)
- **Do:**
  - `crates/oracles`:
    - `pub trait Oracle { fn check_id(&self) -> CheckId; fn judge(&self, input: &OracleInput<'_>) -> Vec<Judgment>; }`
    - `OracleInput` borrows records, the AppModel, the manifest and the config.
    - The crate depends only on `substance-core`, `serde`, `serde_json` and `regex`.
  - `crates/probes`:
    - `#[async_trait] pub trait Probe { fn check_ids(&self) -> &'static [CheckId]; fn level(&self) -> Level; async fn run(&self, ctx: &mut ProbeCtx) -> Result<Vec<RecordId>, ProbeError>; }`
    - `ProbeCtx` gives a fresh snapshot, the driver, the recorder, a seeded RNG and a budget.
  - The orchestrator runs each probe target 3 times on fresh snapshots and judges each repetition. It then aggregates:
    - 3 fails → `fail` (confirmed);
    - 0 fails → `pass`;
    - mixed → `flaky` (reported separately);
    - any not_measured → `not_measured`.
  - Crawl-judged checks (`OracleOnly`, such as B01 and B31) see each target only once, in the crawl. When a crawl record fails, the orchestrator replays its `setup` and `action` 2 more times on fresh snapshots (M3-T07's replay path) and applies the same aggregation. A target that passes in the crawl needs no replay.
  - Add the `Probe(&'static dyn Probe)` and `OracleOnly(&'static dyn Oracle)` variants to `Handler` (M1-T12).
  - Static hints on the same file or route attach as `cause`, for example S03 on a B02 or B03 finding.
- **Outcome:** a pure judging layer and the reproduction rule.
- **Checks:** C-RUST. `cargo tree -p substance-oracles --depth 1 -e normal` lists only `substance-core`, `serde`, `serde_json` and `regex`.

#### M4-T03 B01–B04

- **Depends:** M4-T02 · **Read:** the B01–B04 rows
- **Do:**

| ID | Implementation |
| --- | --- |
| B01 | From crawl records: fail when a control changes nothing in ui (URL, DOM hash, signals, dialog, download), net (minus idle traffic), ws or storage |
| B02 | For each control that made a same-origin non-GET request, rerun it with `route` returning 500, then with `abort`. Fail when a success signal appears with no error signal |
| B03 | Non-GET 2xx while both the DB diff and the storage diff are empty. Needs DB measured; otherwise not_measured |
| B04 | A write with status ≥ 400 (including the generator's invalid inputs) while the DB diff shows inserts or updates. Server-side fault injection comes in M8 |

- **Outcome:** B01–B04 are implemented.
- **Checks:** C-RUST. `checks list --status implemented` includes B01–B04.

#### M4-T04 B18, B19, B28–B31; L05 and S11 bundle parts

- **Depends:** M4-T02 · **Read:** the B18, B19, B28–B31, L05 and S11 rows
- **Do:**

| ID | Implementation |
| --- | --- |
| B18 | Unknown routes (`/__substance_404_<seed>`) and unknown IDs on `:id` routes. Fail on 5xx, a blank page (empty visible text) or a page error |
| B19 | Load each discovered route directly in a fresh context, logged in where needed. Fail on 5xx, blank or page error |
| B28 | Structural markers: the Django debug page, the Werkzeug debugger (`__debugger__`, `/console`), tracebacks in bodies, `/@vite/client` reachable, Next.js dev markers (`/_next/webpack-hmr`) |
| B29 | `href="#"` and `javascript:void(0)`; social links to a platform's bare homepage; placeholder image hosts and template text from `rules/placeholders.toml`; Privacy and Terms links that 404 on an app with email or password fields |
| B30 | Cookie HttpOnly and SameSite (Secure and HSTS are not_measured over http); CSP, X-Content-Type-Options, frame-ancestors or X-Frame-Options, Referrer-Policy; reachable `.map` files; axe violations with serious or critical impact |
| B31 | Framework warning signatures (React's missing-key and hydration messages) and uncaught page errors during the crawl |
| L05, S11 (bundle) | Scan the build output and the served JS for provider keys (`sk-`, `sk-ant-`, `AIza`), Supabase JWTs whose payload `role` is `service_role`, `sb_secret_`, and inlined `VITE_*KEY` values. Also run `gitleaks dir` on the build output. Confirmed; gates |

- **Outcome:** M4's check list is complete.
- **Checks:** C-RUST. `checks list --status implemented` includes B18, B19 and B28–B31.

### M5 Benchmark v1 (code only; the owner runs it)

#### M5-T01 Bench layout, injections and base apps (ADR-0023)

- **Depends:** M4-T04 · **Read:** "Evaluating the evaluator"
- **Do:**
  - `bench/apps/<name>/app.toml`: git URL, pinned commit SHA, stack, an embedded `substance.toml`, and a link showing the app passes RealWorld's API suite.
    - Choose two RealWorld full-stack TS apps, one Vite + Supabase app and one Next.js + Prisma app. ADR-0023 records the choices and criteria: maintained, passes the RealWorld API suite at the pinned SHA, boots with the TS adapters.
    - Never vendor app sources into this repo.
  - Add an `Injection` type to core (`id`, `description`, `language`, `applies_to`, `must_catch: Vec<CheckId>`, `max_matches`, plus the ast-grep `rule` and `fix`), and add it to the schema export.
  - Write `bench/injections/ts/*.yml` for the TypeScript-applicable rows of the plan's starting set:
    - remove `await` before a DB write;
    - swap an error toast for a success toast;
    - remove an ownership check;
    - in-memory store instead of the DB;
    - remove a pagination limit;
    - log the request body in the login handler;
    - pass the request body straight to the ORM;
    - `/health` always returns 200.
- **Outcome:** bench data that validates against the schema.
- **Checks:**
  - C-SCHEMA.
  - `for f in bench/injections/ts/*.yml; do cargo run -q -p substance-cli -- schema validate --kind injection "$f" || exit 1; done` exits 0.

#### M5-T02 `substance bench`

- **Depends:** M5-T01 · **Read:** "Evaluating the evaluator" (metrics, regression gate)
- **Do:**
  - For each base app and each variant (the base itself, plus each applicable injection):
    - clone at the pinned SHA into `bench-runs/<app>/<variant>/work`;
    - apply the rewrite with ast-grep-core. Zero matches, or more than `max_matches`, makes the variant `invalid_injection`;
    - run the pipeline as a library call `--runs` times (default 5).
  - Metrics:
    - recall per check;
    - false positives on base variants;
    - score spread per axis (max − min, ‰);
    - agreement with engineer picks from `bench/picks.toml` (the owner fills it).
  - Output: `bench-report.json` (with a schema) and `bench-report.md`.
  - `--baseline <file>` exits 1 when any recall drops or false positives rise. This is the plan's regression gate.
- **Outcome:** a working bench command, never run by the implementer.
- **Checks:** C-RUST, C-SCHEMA.

### Conventions for M6–M11

Every behavioral check task follows these rules. Tasks don't repeat them.

- **Files.**
  - Probe: `crates/probes/src/<family>/<id>.rs`, for example `crates/probes/src/b/b05.rs`. It implements `Probe` (M4-T02).
  - Oracle: `crates/oracles/src/<family>/<id>.rs`, implementing `Oracle`.
  - Register both in the handler table (M6-T00), and flip the registry entry to `implemented`.
- **Targets.**
  - Choose candidates from the AppModel and the crawl records.
  - Sort them by (route template, control selector, method, path).
  - Keep the first `[budgets] probe_max_targets` (default 20). The skipped count goes into `CheckVerdict.metrics.targets_skipped`.
- **Repetitions.** Every target runs 3 times on a fresh `seeded` snapshot, through the 3-of-3 rule (M4-T02).
- **Missing inputs.** A missing input gives `not_measured` with a reason. Examples: `"db: sqlite"`, `"needs 2 users of one role"`, `"no create form found"`.
- **Evidence.** Each finding carries the record IDs, plus a `note` evidence item stating the failed condition in one sentence.
- **New driver commands.**
  - Add the variant to `DriverCommand`/`DriverEvent` in `crates/core/src/model/protocol.rs`, then run C-SCHEMA, then implement it in `driver/src/`, then run C-TS.
  - The checks for any task that changes the driver include C-TS.
- **Writes.** Probes write only to the per-run database and to the working copy.

Every task's checks: C-RUST, C-GUARD, C-SCHEMA and `C-IMPL <IDs>`. Task-specific extras are listed per task.

### M6 Data and trust probes

#### M6-T00 Handler table and probe budgets

- **Depends:** M4-T04 · **Read:** §4.2 of this file
- **Do:**
  - `crates/orchestrator/src/handlers.rs`:
    - `handler_for` and the `handler` field already exist (M1-T12, M4-T02). Confirm that every check implemented in M1–M4 has an arm, and add any that are missing.
  - Add to `[budgets]` in `ProjectConfig` and `docs/substance.example.toml`: `probe_max_targets = 20`.
  - Add `pub fn select_targets<T: Ord>(candidates: Vec<T>, cap: u32) -> (Vec<T>, u32)` to `crates/probes/src/targets.rs`. It sorts, truncates and returns the skipped count.
  - C-IMPL (defined in §1) uses the `handler` field.
- **Outcome:** no check can be marked implemented without code wired to it.
- **Checks:** C-RUST, C-SCHEMA. Run C-IMPL with the IDs implemented so far (H01–H22, S01–S12, D04, O01–O05, O10, T04–T06, P01–P07, V01, V03, L05, B01–B04, B18, B19, B28–B31).

#### M6-T01 Route tables for TypeScript

- **Depends:** M6-T00 · **Read:** "Language, framework and database adapters"; the B32 row
- **Do:**
  - Core types, with schema export:
    - `RouteTable { routes: Vec<RouteEntry> }`, sorted by (path, method);
    - `RouteEntry { method, path, file, line, guards: Vec<String>, source }`.
  - `crates/adapters/src/ts/routes/` with `next.rs`, `express.rs`, `hono.rs` and `nest.rs`, using `tree-sitter` and `tree-sitter-typescript`. Confirm the grammar API on docs.rs.

    | Stack | What to extract |
    | --- | --- |
    | Next.js app router | `app/**/route.{ts,js}`, exported functions named `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD` or `OPTIONS`. `[id]` becomes `:id`, `[...slug]` becomes `*slug`, `(group)` segments are dropped |
    | Next.js pages router | `pages/api/**` with method `*` |
    | Next.js server actions | Exports of files starting with `"use server"`: method `ACTION`, path `action:<file>#<export>` |
    | Next.js middleware | `middleware.ts` `matcher` patterns add guard `middleware` to the routes they match |
    | Express and Hono | `<ident>.(get\|post\|put\|patch\|delete\|all)("<literal>", …handlers)`. Guards are every handler argument except the last, as written. `<ident>.use("<prefix>", <router>)` resolves prefixes within the file and through one level of imports; an unresolved prefix becomes `?prefix?` in the path |
    | NestJS | `@Controller('x')` plus `@Get(':id')` and siblings. Guards come from `@UseGuards(...)` on the class and the method |

  - `StackAdapter::route_table` returns `Measured` for detected stacks, and `NotMeasured("no route extractor for <stack>")` otherwise.
  - The orchestrator writes `spec/routes.json` to the run directory.
- **Outcome:** route tables for the TS stacks.
- **Checks:** C-RUST, C-SCHEMA.

#### M6-T02 Persistence ladder (B05)

- **Depends:** M6-T01 · **Read:** the B05 row
- **Do:**
  - **Targets:** forms whose crawl submission inserted DB rows.
  - **Create:** fill the form with the seeded generator. The first text field gets the marker `subst-<first 8 hex chars of the target seed>`. Submit and settle.
  - **Find the marker:** after the create, locate the screen whose visible-text map contains the marker. Check the current screen first, then screens within crawl depth 2. If the marker is never visible, the verdict is `unverifiable("entity not visible after create")`; that case belongs to B01 or B03.
  - **Rungs, in order:**
    1. `navigate`: leave the screen, then return to it.
    2. `reload`
    3. `new_context`: same user, fresh browser context.
    4. `restart`: runner stop, then start the same build.
    5. `redeploy`: fresh working copy, then install, build, migrate and start against the same database. No seed or account step.
  - A rung passes when the marker is visible again on that screen. Stop at the first failure.
  - **Oracle:** fail with `sub_check` set to the first failing rung. The registry's `GateRule` makes `restart` and `redeploy` gates.
- **Outcome:** B05 is implemented.
- **Checks:** standard, C-IMPL B05.

#### M6-T03 Round-trip fidelity (B06)

- **Depends:** M6-T02 · **Read:** the B06 row
- **Do:**
  - `rules/roundtrip.toml`: entries `{ id, value }`:
    - `emoji_zwj` (👩‍👩‍👧), `rtl` (שלום), `combining` (é as e plus U+0301), `long_10k` (10,000 × `a`);
    - `quotes` (`'"\`;--`), `zero` (`0`), `negative` (`-1`);
    - `markup`: `<b data-substance-canary="SEED">x</b>`, where `SEED` is replaced at runtime.
  - **Targets:** text and number inputs in create and edit forms.
  - **Each value:** save it through the form, then read it back from:
    - (a) the display screen's visible-text map;
    - (b) the JSON GET response that carried the entity, from the request map.
  - Compare after Unicode NFC normalization and trimming.
  - **Sub-checks** (each is the value ID plus a suffix):
    - `mismatch`: the value read back differs;
    - `truncated`: stored shorter without an error signal;
    - `markup_rendered`: the driver's `query_count {selector}` (a new command) finds `[data-substance-canary]` in the DOM.
- **Outcome:** B06 is implemented.
- **Checks:** standard, C-TS, C-IMPL B06.

#### M6-T04 Double submit (B09)

- **Depends:** M6-T02 · **Read:** the B09 row
- **Do:**
  - New driver command `click_burst { selector, count, interval_ms }`, called with `count: 5, interval_ms: 10`.
  - **Targets:** the submit controls of create forms (as in M6-T02).
  - **Oracle:** fail when the DB diff shows more than one insert into the table that the single-click crawl record inserted into.
  - Needs the DB channel measured; otherwise not_measured.
- **Outcome:** B09 is implemented.
- **Checks:** standard, C-TS, C-IMPL B09.

#### M6-T05 Cross-user access and logout (B10, B14)

- **Depends:** M6-T02 · **Read:** the B10 and B14 rows; "Determinism engineering" (seeding)
- **Do:**
  - **Users:** this check needs two `[[users]]` of the same role, A and B. With fewer, the verdict is `not_measured("needs 2 users of one role")`.
  - **Session export:** new driver command `export_session`, which returns cookies, storage tokens and any `Authorization` header the app sent.
  - **A-owned requests:** first log in as A and create entities through the M6-T02 flow. Then collect same-origin requests made as A that returned 2xx and whose URL or body contains:
    - the primary key of a row A's actions inserted, or
    - A's canary values (M2-T04).
  - **Replay:** use `reqwest` in `crates/probes/src/http.rs` (a shared client). Replay each A-owned request with B's session, and again with no session, on a fresh snapshot each time.
  - **B10 fails when:**

    | Sub-check | Condition |
    | --- | --- |
    | `cross_user_read` / `anonymous_read` | A GET returns 2xx, and the body contains A's canaries or A's row values |
    | `cross_user_write` / `anonymous_write` | A write returns 2xx, and the DB diff changes or deletes A's rows |

  - **B14:** log in as A, export the session, then log out using the control the crawl marked as logout. Replay an A-owned GET with the old session. A 2xx response containing A's data fails.
- **Outcome:** B10 and B14 are implemented.
- **Checks:** standard, C-TS, C-IMPL B10 B14.

#### M6-T06 Mass assignment (B11)

- **Depends:** M6-T05 · **Read:** the B11 row
- **Do:**
  - **Targets:** recorded JSON write requests made by B against B's own resources.
  - **Mutations,** one at a time, each on a fresh snapshot (the mutation name is the `sub_check`):

    | Mutation | Change to the request body |
    | --- | --- |
    | `role` | add `"role": "admin"` |
    | `is_admin` | add both `"isAdmin": true` and `"is_admin": true` |
    | `owner` | set every key matching `(?i)^(user\|owner\|author\|account)_?id$` to A's ID |
    | `price` | multiply numbers in keys matching `(?i)price\|amount\|total\|cost\|balance\|credit` by 0.01; integers become `1` |

  - **Oracle:** fail when the DB diff's after-image has a column whose name, compared case-insensitively with `_` removed, matches the injected key and holds the injected value.
- **Outcome:** B11 is implemented.
- **Checks:** standard, C-IMPL B11.

#### M6-T07 Over-exposure (B12)

- **Depends:** M6-T02 · **Read:** the B12 row
- **Do:**
  - For each JSON response in the crawl records, flatten it to (path, value) pairs.
  - **Unused fields:** a field is unused when its string or number value (strings of 3+ chars, numbers other than 0 and 1) appears neither in the requesting screen's visible-text map nor in any later request URL or body of the same crawl branch.
  - **Fail when** either:
    - unused fields make up at least the `b12_unused_permille` share in `thresholds.toml` (default 600‰) — confidence `hint`;
    - any field is `sensitive` — confidence `confirmed`. Sensitive means the key matches `(?i)password|passwd|hash|salt|token|secret|api_?key`, or the value equals another `[[users]]` entry's email.
- **Outcome:** B12 is implemented.
- **Checks:** standard, C-IMPL B12.

#### M6-T08 Canary tracing (B13)

- **Depends:** M6-T05, M3-T04 · **Read:** the B13 row
- **Do:**
  - Canaries come from `canaries.json` (M2-T04). Each is tagged with its owning user.
  - Search every record channel for canary substrings, and also their URL-encoded and base64 forms. Each channel is a `sub_check`:

    | Sub-check | Where |
    | --- | --- |
    | `server_log` | server log slices |
    | `url` | request URLs and query strings |
    | `storage` | storage values that aren't the owner's own session |
    | `egress` | egress URLs and bodies |
    | `other_user` | responses to another user's session, from M6-T05 records |
    | `email` | emails to any other recipient |

  - Any hit fails, and B13 is a gate.
- **Outcome:** B13 is implemented.
- **Checks:** standard, C-IMPL B13.

#### M6-T09 Account deletion (B25)

- **Depends:** M6-T02 · **Read:** the B25 row
- **Do:**
  - **Config:** add `[account] delete = { ui = "<selector>" }` or `{ api = { method, path } }` to `ProjectConfig` and the example file.
  - **Detecting deletion:** without config, use a crawl record whose DB diff deleted the row holding the user's email. Otherwise the verdict is `not_measured("no account deletion found or configured")`.
  - **After deleting user A:**
    - FK columns referencing the user table's primary key that still hold A's ID → `sub_check: fk_rows`.
    - Text columns equal to A's email or handle → `sub_check: text_rows`. On Postgres, use one `SELECT 1 … WHERE col = $1 LIMIT 1` per text column of each table, capped at 500 columns.
    - Supabase only: rows left in `auth.users`.
    - Tables in `retained_tables` are excluded.
- **Outcome:** B25 is implemented.
- **Checks:** standard, C-SCHEMA, C-IMPL B25.

#### M6-T10 Contract between UI and server (B32)

- **Depends:** M6-T01 · **Read:** the B32 row
- **Do:**

  | Sub-check | Condition | Confidence |
  | --- | --- | --- |
  | `ui_call_missing` | a UI request got 404 or 405, and its path matches no route-table entry | confirmed |
  | `unreached` | a route-table entry that no crawl request matched | hint |
  | `missing_guard` | among routes sharing their first two path segments, at least ⅔ have a non-empty `guards` list and this route has none; confirmed only when an anonymous request to it returns 2xx | confirmed or hint |

- **Outcome:** B32 is implemented.
- **Checks:** standard, C-IMPL B32.

#### M6-T11 Supabase policies (B33)

- **Depends:** M6-T05, M4-T04 · **Read:** the B33 row
- **Do:** runs only when `[db] engine = "supabase"`; otherwise not_measured.

  | Sub-check | How |
  | --- | --- |
  | `rls_disabled` | `SELECT relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relkind = 'r' AND NOT c.relrowsecurity` |
  | `permissive_policy` | `pg_policies` rows in `public` where `roles` includes `anon` or `authenticated`, and `qual` or `with_check` is `true` |
  | `service_key_exposed` | reuse M4-T04's bundle finding |
  | `cross_user_rows` | sign in as B through GoTrue (`/auth/v1/token?grant_type=password`), then `GET /rest/v1/<table>?select=*` for each public table, with headers `apikey: <anon key>` and `Authorization: Bearer <B's access_token>`. Fail when rows contain A's canaries |

  The anon key comes from `supabase status -o json`.
- **Outcome:** B33 is implemented.
- **Checks:** standard, C-IMPL B33.

### M7 API layer

The owner's acceptance runs for M7 use TS apps only (review issue 21).

#### M7-T01 Spec stage (I03)

- **Depends:** M6-T01 · **Read:** "System overview" (the Spec stage), the I03 row
- **Do:**
  - **Shipped spec.** Look for these files first:
    - `openapi.{json,yaml,yml}` and `swagger.{json,yaml,yml}` anywhere, `docs/openapi.*`, `api/openapi.*`;
    - otherwise, the running app's `/openapi.json`, `/swagger.json`, `/api-docs` and `/v3/api-docs`, in that order.
  - **Inferred spec.** First confirm the flags with `mitmproxy2swagger --help`. Then:
    1. Run `mitmproxy2swagger -i crawl.har -o spec/inferred.yml -p <base_url> -f har`.
    2. Remove the `ignore:` prefixes from the generated path list in code.
    3. Run the command again.
    4. Convert the YAML to JSON.
  - **Merge.** Add route-table entries that are missing from the spec as path items with no schemas.
  - **Redocly.** Run `redocly --help` with the pinned version. If `generate-spec` exists, record that in ADR-0025; mitmproxy2swagger is still the default. If neither tool works, stop and report.
  - **Output:** `spec/openapi.json` plus `spec/provenance.json` (`shipped` or `inferred`, and the source path or URL).
  - I03 stays `support`, so it gets no verdict.
- **Outcome:** every run with an app has a spec or a stated reason for not having one.
- **Checks:** C-RUST, C-SCHEMA.

#### M7-T02 Schemathesis (I01)

- **Depends:** M7-T01 · **Read:** the I01 row
- **Do:**
  - Confirm the CLI with `schemathesis run --help` for the pinned version. Stop and report if the flags below don't exist.
  - Run `schemathesis run spec/openapi.json --url <base_url> --checks all --seed <derived seed>`, with a header carrying user A's session and the version's machine-readable report option (JUnit or NDJSON).
  - Parse the report. `sub_check` = the Schemathesis check name; `ignored_auth` is critical and a gate.
  - Each finding's evidence includes the reproduction from the report (a curl line, with session values replaced by `<session>`).
- **Outcome:** I01 is implemented.
- **Checks:** standard, C-IMPL I01.

#### M7-T03 RESTler (I02)

- **Depends:** M7-T01 · **Read:** the I02 row
- **Do:**
  - Runs only when `restler` is on PATH; otherwise not_measured.
  - Run `restler compile --api_spec spec/openapi.json`, then `restler fuzz-lean` with the compiled grammar, dictionary and settings, target host and port, and the time budget. Confirm the flags with `--help`.
  - Each bug bucket in `RestlerResults/**/bug_buckets/` becomes a `sub_check`.
- **Outcome:** I02 is implemented.
- **Checks:** standard, C-IMPL I02.

#### M7-T04 Spec drift (I04)

- **Depends:** M7-T01 · **Read:** the I04 row
- **Do:**
  - Runs only for a shipped spec; otherwise `not_measured("spec inferred")`.
  - Match each recorded request to an operation through the path templates.
  - Validate request and response bodies with the `jsonschema` crate, resolving `$ref` against the spec document.

  | Sub-check | Condition |
  | --- | --- |
  | `undocumented_endpoint` | a recorded request matches no operation |
  | `status_not_documented` | the response status isn't listed for the operation |
  | `request_schema_mismatch` | the request body fails the operation's schema |
  | `response_schema_mismatch` | the response body fails the operation's schema |

- **Outcome:** I04 is implemented.
- **Checks:** standard, C-IMPL I04.

#### M7-T05 Server-side validation (B07)

- **Depends:** M7-T01 · **Read:** the B07 row
- **Do:**
  - For each write operation with a request schema, the seeded generator builds invalid payloads, one per class:
    - `wrong_type`
    - `missing_required`
    - `overlong` (10,000 characters, or 10× `maxLength`)
    - `negative` (−1 where `minimum ≥ 0`, or for count-, price- or amount-named fields)
  - Send each payload straight to the API with A's session, on a fresh snapshot.
  - **Fail when:**
    - the response is 2xx and the DB diff shows the invalid value persisted → `sub_check` = the class;
    - any response is 5xx → `server_error`.
- **Outcome:** B07 is implemented.
- **Checks:** standard, C-IMPL B07.

#### M7-T06 CRUD laws at the API level (B08)

- **Depends:** M7-T01 · **Read:** the B08 row
- **Do:**
  - **Collections:** spec paths with `GET /x` plus `POST /x`, and `GET` or `DELETE /x/{id}`.
  - **Parameters,** detected by name:
    - pagination: `page`, `limit`, `offset`, `cursor`, `per_page`, `size`;
    - sorting: `sort`, `order`, `orderBy`, `order_by`;
    - search: `q`, `search`, `query`;
    - filters: any other query parameter that appears in the spec.
  - **Laws** (each is a `sub_check`):

    | Sub-check | Law |
    | --- | --- |
    | `create_listed` | a created item appears in the list, and in search results by its marker |
    | `delete_gone` | a deleted item disappears from the list, the item GET (404) and search |
    | `filter_subset` | filtered results ⊆ unfiltered results |
    | `pages_partition` | pages are disjoint, and their union equals the full list |
    | `count_matches` | a `total` or `count` field equals the row count |
    | `sort_reverse` | ascending = reverse of descending, after breaking ties by ID |

  - UI-level B08 stays `not_measured("needs agent labels (M12)")`. Its verdict carries target `ui`.
- **Outcome:** B08 is implemented at the API level.
- **Checks:** standard, C-IMPL B08.

#### M7-T07 Finding deduplication

- **Depends:** M7-T02, M7-T05, M6-T05 · **Read:** "Scoring and report"
- **Do:**
  - In the orchestrator's judge stage, an I01 finding and a B07, B10 or B11 finding merge into one when they share:
    - the method;
    - the path template;
    - the failure class. The mapping is a table in code: `ignored_auth` ↔ B10, `negative_data_rejection` ↔ B07, and so on. Each entry has a comment naming the Schemathesis check.
  - Keep the B finding. Add the I01 finding's ID to `related`, and union the evidence sorted by kind.
  - I01's verdict still counts.
- **Outcome:** no duplicate findings across the API and behavioral layers.
- **Checks:** C-RUST.

### M8 Resilience, operations and channels

#### M8-T01 Toxiproxy control

- **Depends:** M3-T04 · **Read:** "Determinism engineering" (fault injection)
- **Do:**
  - The runner starts `toxiproxy-server -port <free>` from PATH as a `ServiceProcess`.
  - `crates/runner/src/faults.rs`: a REST client for `/proxies` and `/proxies/{name}/toxics` with:
    - `latency(ms)`, `timeout(ms)`, `reset_peer`;
    - `down`, which sets `enabled: false`;
    - `clear()`.
  - From boot on, the app's DB URL always points at a Toxiproxy listener, so the URL never changes between faults. When Toxiproxy is missing, checks that need faults report not_measured, and the app connects to the DB directly.
- **Outcome:** faults can be switched per action.
- **Checks:** C-RUST.

#### M8-T02 Egress stubs for LLM providers

- **Depends:** M3-T04 · **Read:** "Determinism engineering" (close the outside world); the L rows
- **Do:**
  - `stubs/llm/providers.toml`: provider → hosts (`api.openai.com`, `api.anthropic.com`, `generativelanguage.googleapis.com`) and response shapes.
  - The proxy intercepts those hosts and answers from `stubs/llm/<provider>/<sha256 of normalized request body>.json`. With no matching file, it uses the provider's default reply.
  - When the request asks for streaming (`"stream": true` or the Gemini stream endpoint), the reply is SSE in the provider's format.
  - Modes are set per probe over a control channel in the runner: `ok`, `rate_limited` (429 with `retry-after`), `error` (500) and `slow` (60 s delay).
  - Every intercepted call is counted as a paid call, and the egress record entry gets `paid_provider`.
- **Outcome:** LLM features run offline and deterministically.
- **Checks:** C-RUST, C-SCHEMA.

#### M8-T03 Faults (B15, B16, O06)

- **Depends:** M8-T01 · **Read:** the B15, B16 and O06 rows
- **Do:**
  - **Targets:** crawl actions with DB writes.
  - **For each fault** (`down`, `latency 5000`, `reset_peer`):
    1. Apply the fault.
    2. Run the action and settle, with the cap raised to 35 s.
    3. Clear the fault.
    4. Run the action again to test recovery.

  | Check / sub-check | Fails when |
  | --- | --- |
  | B15 `white_screen` | the visible text is empty |
  | B15 `false_success` | a success signal appears during the fault |
  | B15 `partial_write` | during the fault, only some of the tables the clean record wrote to were written |
  | B15 `no_recovery` | after clearing, the action's DB diff and signals don't match the clean record |
  | B16 | a request is still pending after 30 s, or `aria-busy="true"` or `role=progressbar` remains at the settle cap |
  | O06 | with the DB `down`, `GET health_path` returns 200. No `health_path` → not_measured |

- **Outcome:** B15, B16 and O06 are implemented.
- **Checks:** standard, C-IMPL B15 B16 O06.

#### M8-T04 Scaling counts (B17, ADR-0024)

- **Depends:** M6-T02 · **Read:** the B17 row
- **Do:**
  - **Tables:** those the crawl inserted into.
  - **Cloning** is Postgres only (SQLite → not_measured):
    - Clone seeded rows with `INSERT … SELECT`, in topological FK order.
    - Primary keys come from the column default (serial or `gen_random_uuid()`).
    - Unique text columns get the suffix `-c<n>`.
    - Raise each table to 10 rows, then to 1,000.
  - **List endpoints:** recorded GETs returning a JSON array, or an object with an array field.
  - At each size, measure query count (`pg_stat_statements`) and response bytes.

  | Sub-check | Fails when |
  | --- | --- |
  | `n_plus_one` | queries at 1,000 ÷ queries at 10 > `b17_query_ratio` (default 5) |
  | `unbounded` | bytes at 1,000 ÷ bytes at 10 > `b17_bytes_ratio` (default 20) |

  - Both ratios go in `thresholds.toml`. ADR-0024 records the cloning method.
- **Outcome:** B17 is implemented.
- **Checks:** standard, C-IMPL B17.

#### M8-T05 Time travel (B24)

- **Depends:** M3-T06 · **Read:** the B24 row
- **Do:**
  - **Instants,** each a `sub_check`:
    - `token_expiry`: a JWT `exp` found in cookies or storage, plus 1 minute; skipped when no JWT is found;
    - `new_year`: the next 1 January at 00:00:30 UTC;
    - `dst`: the next DST switch for `America/Los_Angeles`, plus 1 hour. This instant runs in a context with `timezoneId: "America/Los_Angeles"`, and with `TZ=America/Los_Angeles` for the server part;
    - `leap_day`: 2028-02-29T12:00:00Z.
  - **Steps:** log in, open the screens visited after login, fast-forward the browser clock with `clock` `fastForward`, and reload each screen.
  - **Server clock:** only when `server_clock = fixed_start`. Restart the app with `FAKETIME="@<instant>"` and `FAKETIME_DONT_RESET=1`; otherwise the server part is not_measured.
  - **Fails on:** 5xx, an empty visible text, a page error, or landing on a login screen. Exception: for `token_expiry`, a redirect to login with no error is a pass.
- **Outcome:** B24 is implemented.
- **Checks:** standard, C-IMPL B24.

#### M8-T06 Trackers before consent (B26)

- **Depends:** M3-T04 · **Read:** the B26 row
- **Do:**
  - `rules/trackers.txt`: a domain list from a named source, with a pinned version recorded in its header comment. The owner approves the source in ADR-0026.
  - Load `/` and the first 5 crawl screens in a fresh context, with no clicks.
  - Fail on any request, in the browser or through egress, to a listed domain or its subdomains. Evidence is the domain and the initiating URL.
- **Outcome:** B26 is implemented.
- **Checks:** standard, C-IMPL B26.

#### M8-T07 Email delivery claims (B27)

- **Depends:** M3-T04, M2-T04 · **Read:** the B27 row
- **Do:**
  - **Config:** `[mail] flows = [{ name, ui = "<selector>" | api = { method, path, body }, expect = "link" | "code" }]`.
  - **Detected flow:** sign-up, meaning the form whose submission inserted the row holding the new email. It's used only when the repo depends on a mail library, listed in `rules/mail-libs.toml` (nodemailer, resend, @sendgrid/mail, postmark, smtplib, lettre, among others).
  - **Run:** trigger each flow, then poll Mailpit for 10 s.

  | Sub-check | Fails when |
  | --- | --- |
  | `not_sent` | no message arrives |
  | `wrong_recipient` | the recipient isn't the expected one |
  | `no_link` | `expect = "link"`, and no URL points to the app's origin |
  | `no_code` | `expect = "code"`, and the body has no 4–8 digit code |

  - With no flows, the verdict is not_measured.
- **Outcome:** B27 is implemented.
- **Checks:** standard, C-SCHEMA, C-IMPL B27.

#### M8-T08 Resource leaks (O07)

- **Depends:** M6-T00 · **Read:** the O07 row
- **Do:**
  - **Action:** the first GET page in the crawl's sorted order. Repeat it 1,000 times with the shared HTTP client, not the browser.
  - **Sample every 100 repetitions:**
    - `pg_stat_activity` connections to the app database;
    - open file descriptors summed over every process in the app's process group (PIDs from `pgrep -g <pgid>`): `/proc/<pid>/fd` on Linux, `lsof -p <pid>` via `run_tool` elsewhere;
    - RSS summed over the same PIDs, via `ps -o rss= -p <pid,…>`. Measuring only the leader would measure the `npm start` wrapper.
  - **Fail when** a metric rises strictly across the last 5 samples and the last sample is over 1.5× the first. Each metric is a `sub_check`.
- **Outcome:** O07 is implemented.
- **Checks:** standard, C-IMPL O07.

#### M8-T09 Deploy configuration (O08, O09)

- **Depends:** M6-T01 · **Read:** the O08 and O09 rows; ADR-0020
- **Do:**
  - **O08:** run `hadolint --format json` and `trivy config --format json` on Dockerfiles. Map severities: error/CRITICAL/HIGH → fail; others → hint. Image scans are `not_measured("no containers, ADR-0020")`.
  - **O09:**
    - Find Kubernetes manifests: YAML files with top-level `apiVersion` and `kind`.
    - Run `kube-linter lint --format json` on them.
    - Paths from liveness and readiness probes and from compose `healthcheck` URLs that match no route-table entry → `probe_path_missing`.
    - No deploy manifests → not_measured.
- **Outcome:** O08 and O09 are implemented.
- **Checks:** standard, C-IMPL O08 O09.

#### M8-T10 Upgrade migration (O11)

- **Depends:** M6-T02 · **Read:** the O11 row
- **Do:**
  - **Previous version:** the newest tag reachable from HEAD that doesn't point at HEAD. With no such tag, the verdict is not_measured.
  - **Old side:**
    - Create a second working copy at the tag and a database `…_prev`.
    - Run the old boot plan: install, migrate, seed, start.
    - Run a crawl capped at 50 actions.
    - Record the row count of every table, then stop the app.
  - **New side:** on the same database, run HEAD's migrate, then start HEAD.

  | Sub-check | Fails when |
  | --- | --- |
  | `migrate_failed` | HEAD's migrate exits non-zero |
  | `data_lost` | a table that still exists has fewer rows than before |
  | `broken_after` | `ready_path` or the first 10 crawl screens return 5xx or render blank |

- **Outcome:** O11 is implemented.
- **Checks:** standard, C-IMPL O11.

#### M8-T11 AI features (L01–L04)

- **Depends:** M8-T02 · **Read:** the L rows
- **Do:**
  - **Actions with paid calls:** crawl actions whose egress has entries with `paid_provider`.

  | Check | Sub-check | Fails when |
  | --- | --- | --- |
  | L01 | `anonymous_trigger` | replaying those actions' requests with no session still causes paid egress |
  | L01 | `anonymous_ui` | an anonymous crawl of public screens causes paid egress |
  | L02 | `fan_out` | one action makes more than 3 paid calls |
  | L02 | `per_keystroke` | typing 10 characters one by one (driver `type` with `delay_ms: 50`) into the action's input makes 5 or more paid calls |
  | L03 | `false_success` | under stub mode `rate_limited`, `error` or `slow`, a success signal appears |
  | L03 | `hang` | under those modes, loading persists at the settle cap with no error signal |
  | L04 | — | stub replies containing `<img src=x onerror="window.__substance_l04=1">` and `[x](javascript:window.__substance_l04=1)` set the flag. Read it with the new driver command `read_global { name }` |

- **Outcome:** L01–L04 are implemented.
- **Checks:** standard, C-TS, C-IMPL L01 L02 L03 L04.

### M9 Front-end completeness and data model

#### M9-T01 Five states (B20)

- **Depends:** M6-T02 · **Read:** the B20 row
- **Do:**
  - **Targets:** screens whose data requests return arrays.
  - New driver command option `observe_at_ms` on `action` and `navigate`: snapshot the DOM at that time, before settling.

  | State | How it's produced | Fails when |
  | --- | --- | --- |
  | `loading` | data routes delayed 3,000 ms, observed at 500 ms | the main region (`main`, else `body`) has empty visible text and no `aria-busy`, `role=progressbar` or `role=status` |
  | `empty` | a fresh account via the sign-up flow (not_measured without one) | the main region is blank, or a spinner shows at settle |
  | `failed` | `route fail_500` on all data requests | no error signal, or a success signal, or a blank main region |
  | `partial` | the first data request fails, in sorted order | a page error, or a blank main region |
  | `full` | the base crawl record | any of the above |

- **Outcome:** B20 is implemented.
- **Checks:** standard, C-TS, C-IMPL B20.

#### M9-T02 Keyboard only (B21)

- **Depends:** M6-T02 · **Read:** the B21 row
- **Do:**
  - **Target:** the sign-up form, else the first create form.
  - New driver command `keyboard_fill { form_selector, values }`:
    - press Tab up to 60 times;
    - type each value when its field is focused;
    - press Enter or Space on the submit control.
    - It returns the focus order and whether submit was reached.

  | Sub-check | Fails when | Confidence |
  | --- | --- | --- |
  | `unreachable_field` | a required field never gets focus | confirmed |
  | `focus_trap` | the focus order repeats before submit is reached | confirmed |
  | `effect_differs` | the DB diff differs from the mouse-driven record | confirmed |
  | `no_focus_indicator` | the computed `outline` and `box-shadow` are identical with and without focus | hint |

- **Outcome:** B21 is implemented.
- **Checks:** standard, C-TS, C-IMPL B21.

#### M9-T03 Layout (B22)

- **Depends:** M4-T01 · **Read:** the B22 row
- **Do:**
  - For each crawl screen (capped), at viewport widths 360, 768 and 1280 px (height 800), call the driver's `layout` command, which returns these as sub-checks:
    - `overflow_x`: `scrollWidth > clientWidth`;
    - `overlap`: `elementFromPoint` at a control's center isn't the control or one of its descendants;
    - `offscreen`: a control's box lies outside the document;
    - `small_target`: a control smaller than 24×24 px, at width 360 only.
  - The `sub_check` is `<kind>@<width>`.
- **Outcome:** B22 is implemented.
- **Checks:** standard, C-TS, C-IMPL B22.

#### M9-T04 Timezone and locale (B23)

- **Depends:** M6-T02 · **Read:** the B23 row
- **Do:**
  - **`input_locale`:**
    - Context with locale `de-DE`.
    - In number inputs of create forms, enter `12,50`, or `12.50` when the input's `type` is `number`.
    - Fail when the DB stores a value other than 12.5 and no error signal appeared.
  - **`date_shift`:**
    - Run a create form with a date input set to `2026-03-15` in two contexts: timezone `Pacific/Kiritimati` and timezone `America/Los_Angeles`.
    - `date` columns: the stored value must be `2026-03-15`.
    - `timestamptz` columns: the value converted to the context's TZ must fall on 2026-03-15.
    - Also fail when the display screen shows a different day across the two contexts for the same row.
- **Outcome:** B23 is implemented.
- **Checks:** standard, C-IMPL B23.

#### M9-T05 Schema quality (D01, D02)

- **Depends:** M2-T03 · **Read:** the D01 and D02 rows
- **Do:**
  - Add `DbAdapter::catalog()` returning tables, columns (with type and nullability), FKs, indexes and unique constraints.
    - Postgres: `information_schema` plus `pg_index`.
    - SQLite: `pragma table_info`, `foreign_key_list`, `index_list` and `index_info`.
  - **D01 sub-checks:**

    | Sub-check | Condition | Confidence |
    | --- | --- | --- |
    | `unindexed_fk` | an FK whose columns aren't the leading columns of any index | confirmed |
    | `missing_fk` | a column named `*_id` or `*Id` with no FK, while a table with the matching name exists | hint |
    | `nullable_required` | a nullable column fed by a form input marked `required` | hint |
    | `email_not_unique` | an email-named column with no unique index | confirmed |
    | `naive_timestamp` | `timestamp without time zone` (Postgres only) | confirmed |

  - **D02:** `squawk --reporter json` over `prisma/migrations/*/migration.sql`, `drizzle/**/*.sql`, `supabase/migrations/*.sql` and `migrations/**/*.sql`. Postgres only; confirm the flags with `squawk --help`.
- **Outcome:** D01 and D02 are implemented.
- **Checks:** standard, C-IMPL D01 D02.

#### M9-T06 Money types (D03)

- **Depends:** M9-T05 · **Read:** the D03 row
- **Do:**
  - **Static,** matching names against `(?i)price|amount|total|cost|balance|fee|salary|money`:
    - DB columns of type `real`, `double precision` or `float`;
    - Prisma `Float` fields;
    - rules in `rules/ts/money-types.yml` (Drizzle `real()`/`doublePrecision()`, TypeORM `type: "float"`).
  - **Behavioral:**
    - Save `19.99` through a form's money field. The DB value read as text must be `19.99`, and the API and the UI must show `19.99`.
    - When the same form also has a quantity-named field, set it to 3. If a total-named field appears in the response, it must equal `59.97`.
  - Sub-checks: `float_type`, `stored_inexact`, `total_inexact`.
- **Outcome:** D03 is implemented.
- **Checks:** standard, C-IMPL D03.

### M10 Test truthfulness

This milestone makes substance run *the evaluated app's* tests. It adds no tests to substance.

#### M10-T01 Server coverage map

- **Depends:** M6-T00 · **Read:** "Check catalog: static, test and dependency checks" (T rows)
- **Do:**
  - **TS apps:** start with `NODE_V8_COVERAGE=<run>/coverage/raw`. After a graceful stop (needed to flush the data), run `c8 report --temp-directory <raw> --reporter=json --report-dir <run>/coverage` from PATH; confirm the flags.
  - Map paths to the working copy and drop files that aren't authored.
  - **Core files** are found by differential coverage, because `NODE_V8_COVERAGE` covers a whole process, not one action:
    - run (a): boot, `wait_ready`, graceful stop;
    - run (b): boot, replay the crawl records that wrote to the DB (`setup` plus `action`), graceful stop;
    - core files are authored files with at least one function whose hit count in (b) exceeds (a), using c8's JSON (istanbul `f` counts).
    - Write them to `coverage/core_files.json`.
  - **Report coverage:** `coverage.server_code` = covered ÷ authored server lines, in permille.
  - Python is added in M11-T01. Rust is `not_measured("rust coverage: v1")`.
- **Outcome:** a coverage map for T02 and the report.
- **Checks:** C-RUST, C-SCHEMA.

#### M10-T02 Run the app's suite (T01)

- **Depends:** M10-T01, M2-T04 · **Read:** the T01 row
- **Do:**
  - **Test command:**
    - from `[app] test` in config, else the adapter's `test_command`;
    - for TS: the `test` script, unless it's npm's default `echo "Error: no test specified"`;
    - for Python: pytest; for Rust: `cargo test --locked`.
  - **Environment:**
    - needs `ExecPermit`;
    - a fresh working copy;
    - the DB URL points at a database `…_tests` created from the seeded template;
    - `CI=true`;
    - the egress proxy with no allow list.
  - **Machine-readable output:**
    - jest: `--json --outputFile`;
    - vitest: `--reporter=json --outputFile`;
    - pytest: `--junitxml`;
    - cargo: parse the `test result:` lines.
  - **Sub-checks:**

    | Sub-check | Fails when |
    | --- | --- |
    | `failed` | the suite fails |
    | `no_tests` | 0 tests ran |
    | `timeout` | the budget ran out |
    | `network_blocked` | failures whose logs show connections refused by the proxy; reported separately, as a hint |

  - The per-test results are saved to `tests/baseline.json` in the run directory.
- **Outcome:** T01 is implemented.
- **Checks:** standard, C-IMPL T01.

#### M10-T03 Mutation score (T02)

- **Depends:** M10-T02 · **Read:** the T02 row
- **Do:**
  - **TS:** write `stryker.config.json` into the working copy:
    - `mutate` = the core files;
    - `testRunner` = jest or vitest (confirm the runner plugins);
    - `reporters: ["json"]`, `concurrency: 2`;
    - `timeoutMS` from the budget.
  - Run `stryker run` from PATH: `@stryker-mutator/core` plus the jest or vitest runner plugin, at the versions pinned in `tools.toml`. Never `npx`, which can download at run time. Needs `ExecPermit`.
  - Parse `reports/mutation/mutation.json` (the mutation-testing-elements schema). Score = killed ÷ (killed + survived + timeout), in permille.
  - Fail when the score is below `t02_min_score_permille` (default 600).
  - Out of budget → not_measured, with the partial score as a metric.
  - Python and Rust are added in M11-T03.
- **Outcome:** T02 is implemented.
- **Checks:** standard, C-IMPL T02.

#### M10-T04 Isolation (T03)

- **Depends:** M10-T02 · **Read:** the T03 row
- **Do:**
  - **Shuffled run:**
    - jest: `--randomize --seed=<n>`;
    - vitest: `--sequence.shuffle --sequence.seed=<n>`;
    - pytest: `-p randomly --randomly-seed=<n>`;
    - cargo: not_measured.
  - **Repeat run:** run the suite a second time, with no reset.
  - **Fail** with the test names when a test passed in the baseline but fails in either run. Sub-checks: `order_dependent` and `state_leak`.
- **Outcome:** T03 is implemented.
- **Checks:** standard, C-IMPL T03.

### M11 Python and Rust packs

#### M11-T01 Python stacks

- **Depends:** M2-T02, M6-T01 · **Read:** "Language, framework and database adapters"
- **Do:**
  - **Detection:** `pyproject.toml` or requirements dependencies on fastapi, django, flask or streamlit.
  - **Install:**
    - `uv sync --locked` when `uv.lock` exists (the plan's locked install, which O01 measures);
    - `poetry sync` when `poetry.lock` exists, or `poetry install --sync` on Poetry 1.x, chosen by `poetry --version`;
    - otherwise `python -m venv .venv`, then `.venv/bin/pip install -r requirements.txt`.
  - **Start:**

    | Stack | Command |
    | --- | --- |
    | FastAPI | `uvicorn <module>:<var> --port $PORT`; the module and variable come from tree-sitter-python finding `<var> = FastAPI(` |
    | Django | `manage.py migrate`, then `gunicorn <project>.wsgi --bind 127.0.0.1:$PORT` when gunicorn is a dependency; else `manage.py runserver --noreload`, flagged as a dev server so B28 reports not_measured |
    | Flask | gunicorn if present, else `flask --app <module> run --port $PORT` |
    | Streamlit | `streamlit run <file> --server.port $PORT --server.headless true`; its faults go through Toxiproxy |

  - **Migrations:** `alembic upgrade head`, and Django `migrate`.
  - **Route tables:**
    - FastAPI: GET `/openapi.json` at runtime.
    - Flask and Django: introspection scripts in `crates/adapters/scripts/{flask_routes,django_routes}.py`, embedded with `include_str!`, run with the venv's Python under `ExecPermit`. They print RouteTable JSON.
  - **Coverage:** `coverage run` wrapping the start command, then `coverage json`.
  - **Test command:** pytest.
- **Outcome:** Python apps boot, and their routes and coverage are extracted.
- **Checks:** C-RUST.

#### M11-T02 Rust stacks

- **Depends:** M2-T02, M6-T01 · **Read:** "Language, framework and database adapters"
- **Do:**
  - **Detection:** Cargo dependencies on `axum` or `actix-web`.
  - **Build:** `cargo build --release --locked`. The binary is the only `bin` target from `cargo metadata --format-version 1 --no-deps`; `[app] start` is required when there are several.
  - **Port:** `PORT`. When the code doesn't read it, `[app] start` must give the port. Detect `env::var("PORT")` with tree-sitter-rust.
  - **Migrations:** `sqlx migrate run`, `diesel migration run`, `sea-orm-cli migrate up`.
  - **Route tables,** with tree-sitter-rust:
    - Axum: `.route("<path>", get(h).post(h2))` and `.nest("<prefix>", …)` within a file. Guards are the identifiers inside `.layer(…)`/`.route_layer(…)`, plus the extractor type names in the handler's parameters.
    - Actix: `#[get("<path>")]`-style attributes, and `web::resource("<path>").route(web::get().to(h))`.
  - **Test command:** `cargo test --locked`. Coverage is not_measured.
- **Outcome:** Rust apps boot, and their routes are extracted.
- **Checks:** C-RUST.

#### M11-T03 Python and Rust rules and mutation

- **Depends:** M11-T01, M11-T02 · **Read:** the S02, S03, D04 and T04 rows
- **Do:**
  - Complete `rules/python/*.yml` and `rules/rust/*.yml` for S02, S03, D04, D03 (money types) and T04.
  - Mutation for T02:
    - Rust: `cargo mutants --file <core file>` (one flag per file); parse `mutants.out/outcomes.json`.
    - Python: `mutmut run --paths-to-mutate <core files>`, then `mutmut results`.
    - Confirm the flags for both. Coverage-based core files exist only for Python; Rust uses files under `src/` that contain handlers.
- **Outcome:** checks work equally across the three languages.
- **Checks:** standard, C-IMPL with the same IDs (they must stay implemented).

#### M11-T04 Bench for Python and Rust

- **Depends:** M11-T03, M5-T02 · **Read:** "Evaluating the evaluator"
- **Do:**
  - Add RealWorld Python (FastAPI) and Rust (Axum) apps to `bench/apps/`. The owner approves them in ADR-0023.
  - Add `bench/injections/python/*.yml` and `bench/injections/rust/*.yml`:
    - the plan's Python and Rust rows: remove `Depends(get_current_user)`; `.await?` → `.await.ok()`;
    - Python and Rust equivalents of the TS set where a pattern exists.
- **Outcome:** bench data for every language.
- **Checks:** C-SCHEMA, and the injection validation loop from M5-T01 over the new folders.

### M12 Agentic layer: deferred (ADR-0006)

Nothing to build beyond M0's stub crate. UI-level B08 reports `not_measured("needs agent labels (M12)")`. The `[signals]` config can supply success selectors instead.

### M13 Scoring v1 and report

#### M13-T01 Scoring

- **Depends:** M11-T04 (all checks exist) · **Read:** "Scoring and report"
- **Do:**
  - **Core types,** with schema export:
    - `AxisScore { axis, grade: A|B|C|D|F, coverage_permille, low_confidence, worst_finding: Option<FindingId> }`;
    - `Summary { works_now: bool, history: maintained | abandoned_or_one_shot | unknown, slop: bool, gates_hit: Vec<CheckId>, cell: String }`;
    - `Report` gains `scores: Vec<AxisScore>` and `summary: Summary`.
  - **`config/weights.toml`:** severity → grade (`critical = "F"`, `high = "D"`, `medium = "C"`, `low = "B"`) and `low_confidence_below_permille = 500`. Its sha256 becomes `scoring_weights_version`.
  - **Grades:** each axis grade is the worst grade among its confirmed failures; A when there are none.
    - Flaky findings, hints and `reported` verdicts never lower a grade. `reported` counts as measured for coverage.
    - Coverage = measured ÷ v1 checks of the axis.
    - Provenance is never scored, and Generic isn't scored in v1.
  - **Gates:** from `GateRule`. `--fail-on gate` exits 1 when `gates_hit` isn't empty.
  - **Summary matrix cells:**

    | | Maintained | Abandoned or one-shot | History unknown |
    | --- | --- | --- | --- |
    | Works now | "Solid" | "Works, unmaintained" | "Works, history unknown" |
    | Broken now | "Broken, maintained" | "Slop" when ADR-0012's two-family rule holds, else "Broken, abandoned" | "Broken, history unknown" |

    - **Works now** = no gate hit and works_now grade C or better.
    - **Maintained** = archetype Young and iterating, Solo maintained or Team maintained.
    - **Abandoned or one-shot** = One-shot dump, or Burst then abandoned.
- **Outcome:** a deterministic score and summary.
- **Checks:** C-RUST, C-SCHEMA.

#### M13-T02 SARIF output

- **Depends:** M13-T01 · **Read:** "Scoring and report" (outputs)
- **Do:**
  - `report.sarif`, built with `serde-sarif` (version 2.1.0).
  - One rule per check ID: `id`, `name` = title, `properties.family`, `properties.axes`.
  - Level: critical and high → `error`; medium → `warning`; low and info → `note`.
  - Static findings get physical locations (file, line). Behavioral findings get logical locations (`route` or `screen`).
  - `properties.replay` holds the replay command.
  - `fingerprints.substance/v1` = the finding ID.
  - Vendor the official SARIF 2.1.0 JSON schema at `schemas/vendor/sarif-2.1.0.json`, recording its source URL in a sibling `README`. Add `--kind sarif` to `schema validate`.
- **Outcome:** SARIF that code-scanning UIs accept.
- **Checks:** C-RUST, C-SCHEMA.

#### M13-T03 Full HTML report and `substance report`

- **Depends:** M13-T01 · **Read:** "Scoring and report"
- **Do:**
  - Templates in `crates/report/templates/` cover these sections, in order:
    1. the summary cell;
    2. gates hit;
    3. axis grades with coverage and a low-confidence mark;
    4. findings grouped by axis, with severity, confidence, evidence links (relative paths into the run directory) and the replay command;
    5. the flaky list;
    6. the not_measured and unverifiable list with reasons;
    7. the archetype panel;
    8. the provenance panel ("never scored");
    9. the coverage block;
    10. the manifest, including `egress_enforced` and the thresholds-uncalibrated banner.
  - `substance report <RUN_DIR> [--format …]` re-renders from stored data, without network or app.
- **Outcome:** the full report.
- **Checks:** C-RUST. `rg -n '<script|https?://' crates/report/templates` prints nothing.

#### M13-T04 Replay for every finding

- **Depends:** M13-T01, M3-T07 · **Read:** "Driver protocol and effect recorder" (replay)
- **Do:**
  - Every finding's `replay` = `substance replay <fnd_id> --run <dir>`.
  - **Behavioral findings:** replay the first record (M3-T07).
  - **Static, history and dependency findings:** re-run that check's handler (`handler_for`) at the same commit, with `--snapshot-from <dir>`. Exit 0 when the same finding ID is produced again, 1 otherwise.
  - `replay` also accepts `--all`, which replays every finding and prints a table.
- **Outcome:** every finding is reproducible.
- **Checks:** C-RUST.

---

## 7. Deferred and out of v1 scope

**By owner instruction (no tests, ADR-0008).** Not built and not run:

- round-trip tests for core types (M0's acceptance), replaced by schema export and validate plus compile-time assertions;
- golden-file oracle tests (`crates/oracles/tests/fixtures/`);
- per-probe integration tests against benchmark apps;
- the double-run determinism diff of the M4 suite in CI;
- the benchmark recall gate in CI (the `bench` command exists; CI doesn't run it);
- "tests alongside the code" from "Engineering conventions".

**By owner decision:**

- sandbox isolation: Docker, gVisor, Kubernetes, container build-ladder rungs, image-digest pinning, Trivy image scans (ADR-0003, ADR-0020);
- URL and repo+url modes, and `--i-own-this` (ADR-0004);
- GitLab and other forges (ADR-0005);
- A01–A04, M12, UI-level B08 and build-ladder rung 4 (ADR-0006).

**Later, per the plan:** V02, A05, G01, G02, learned weights, Wasm oracle plugins, per-PR incremental runs, the payments channel, more databases and interfaces, service mode, fix tasks for coding agents.

---

## 8. Owner acceptance runs (not for the implementing model)

These are the plan's acceptance criteria, as commands the owner runs after a milestone's tasks are done.

| M | Run | Expect |
| --- | --- | --- |
| M0 | `substance schema validate --kind effect-record schemas/examples/effect-record.json`, then run C-SCHEMA twice | Valid; no diff |
| M1 (before) | `scripts/scaffold-hashes.sh`, then commit `rules/scaffolds.toml` | S04's hash part and H17's scaffold match become measured |
| M1 | `time substance run --triage-only --as-of 2026-10-01T00:00:00Z <public GitHub repo>`, then rerun with `--snapshot-from <first run dir>` and `diff` the two `report.json` | Under 2 min after cloning; identical reports, provided no time cap fired |
| M2 | `SUBSTANCE_PG_ADMIN_URL=… substance run --allow-exec <RealWorld TS clone>` | Boots from a clean clone; O01–O05 verdicts present; reset works per probe |
| M3 | `substance replay <rec_id> --run <dir>`, twice | Exit 0; byte-identical normalized records |
| M4 | Full run on unmodified RealWorld TS | Zero findings for B01–B04, B18, B19, B28–B31 and L05 |
| M5 | `substance bench --suite bench --runs 5` | Recall of at least 90% for each M4 check (target) |
| M6 | Bench with the M6 injections | Injections caught; no false positives on base apps |
| M7 | A TS app with a shipped spec, and a TS app with an inferred spec | Both fuzzed; findings deduplicated against B checks |
| M8 | Bench | Every new check catches at least one injected defect |
| M9 | Bench | Each check verified on a base app and an injected variant |
| M10 | Run on a base app | Mutation stays within the configured time budget |
| M11 | Bench per language, plus the FastAPI fuzz from M7 | Recall within 10 points of TypeScript |
| M13 | Full run | Every finding has evidence and a replay command that reproduces it |

---

## 9. Progress

Tick each task in the same commit that completes it.

- [x] M0-T01 Workspace scaffold
- [x] M0-T02 ADRs
- [x] M0-T03 Core IDs and registry
- [x] M0-T04 Core data model
- [ ] M0-T05 Project config
- [ ] M0-T06 CLI skeleton
- [ ] M0-T07 Schema and TS generation
- [ ] M0-T08 Orchestrator, run dir, report v0
- [ ] M0-T09 CI workflow
- [ ] M1-T01 Tool runner and external snapshots
- [ ] M1-T02 Git engine
- [ ] M1-T03 GitHub forge
- [ ] M1-T04 Registries and lockfiles
- [ ] M1-T05 H01–H10
- [ ] M1-T06 H11–H22, archetypes, thresholds
- [ ] M1-T07 Static rule engine
- [ ] M1-T08 S01–S12
- [ ] M1-T09 D04, O01, O10, T04–T06, L05
- [ ] M1-T10 P01–P07
- [ ] M1-T11 V01, V03
- [ ] M1-T12 Report v0 and triage end to end
- [ ] M2-T01 Working copy and supervisor
- [ ] M2-T02 StackAdapter and TS stacks
- [ ] M2-T03 DbAdapter
- [ ] M2-T04 Seeding and accounts
- [ ] M2-T05 O01–O05
- [ ] M2-T06 doctor
- [ ] M3-T01 TS driver
- [ ] M3-T02 Driver host
- [ ] M3-T03 Recorder channels
- [ ] M3-T04 Egress proxy and Mailpit
- [ ] M3-T05 Normalization and idle baseline
- [ ] M3-T06 Clock, TZ, locale
- [ ] M3-T07 Replay
- [ ] M4-T01 Crawler
- [ ] M4-T02 Oracle framework, 3-of-3
- [ ] M4-T03 B01–B04
- [ ] M4-T04 B18, B19, B28–B31, bundle scans
- [ ] M5-T01 Bench layout and injections
- [ ] M5-T02 bench command
- [ ] M6-T00 Handler table and probe budgets
- [ ] M6-T01 Route tables for TypeScript
- [ ] M6-T02 Persistence ladder (B05)
- [ ] M6-T03 Round-trip fidelity (B06)
- [ ] M6-T04 Double submit (B09)
- [ ] M6-T05 Cross-user access and logout (B10, B14)
- [ ] M6-T06 Mass assignment (B11)
- [ ] M6-T07 Over-exposure (B12)
- [ ] M6-T08 Canary tracing (B13)
- [ ] M6-T09 Account deletion (B25)
- [ ] M6-T10 Contract between UI and server (B32)
- [ ] M6-T11 Supabase policies (B33)
- [ ] M7-T01 Spec stage (I03)
- [ ] M7-T02 Schemathesis (I01)
- [ ] M7-T03 RESTler (I02)
- [ ] M7-T04 Spec drift (I04)
- [ ] M7-T05 Server-side validation (B07)
- [ ] M7-T06 CRUD laws at the API level (B08)
- [ ] M7-T07 Finding deduplication
- [ ] M8-T01 Toxiproxy control
- [ ] M8-T02 Egress stubs for LLM providers
- [ ] M8-T03 Faults (B15, B16, O06)
- [ ] M8-T04 Scaling counts (B17, ADR-0024)
- [ ] M8-T05 Time travel (B24)
- [ ] M8-T06 Trackers before consent (B26)
- [ ] M8-T07 Email delivery claims (B27)
- [ ] M8-T08 Resource leaks (O07)
- [ ] M8-T09 Deploy configuration (O08, O09)
- [ ] M8-T10 Upgrade migration (O11)
- [ ] M8-T11 AI features (L01–L04)
- [ ] M9-T01 Five states (B20)
- [ ] M9-T02 Keyboard only (B21)
- [ ] M9-T03 Layout (B22)
- [ ] M9-T04 Timezone and locale (B23)
- [ ] M9-T05 Schema quality (D01, D02)
- [ ] M9-T06 Money types (D03)
- [ ] M10-T01 Server coverage map
- [ ] M10-T02 Run the app's suite (T01)
- [ ] M10-T03 Mutation score (T02)
- [ ] M10-T04 Isolation (T03)
- [ ] M11-T01 Python stacks
- [ ] M11-T02 Rust stacks
- [ ] M11-T03 Python and Rust rules and mutation
- [ ] M11-T04 Bench for Python and Rust
- [ ] M13-T01 Scoring
- [ ] M13-T02 SARIF output
- [ ] M13-T03 Full HTML report and `substance report`
- [ ] M13-T04 Replay for every finding
