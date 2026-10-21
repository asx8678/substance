# Substance — architecture plan for a deterministic app evaluator

Oct 21, 2026 · @Adam

## Read this first

Build a Rust CLI, working name `substance`, that judges whether an app is solid or slop using deterministic, evidence-backed checks. Everything here comes from design discussions with the owner; this doc is the handover to the building agent.

How to use this plan:

- Work milestone by milestone (see Milestones). Each one ends with its acceptance criteria green in CI before the next starts.
- Treat the check catalog as the backlog. Check IDs (B01, H03, …) are stable: use them in code, tests, reports and commit messages, and never reuse one.
- Where the plan is silent, choose the more deterministic option and record it as a short ADR in `docs/decisions/`.
- Numbers marked as targets or defaults are starting points. Calibrate them with the benchmark, never by guessing.

Ask the owner these before writing code:

1. Name: keep `substance` or rename it? It appears in the binary, crates and report titles.
2. Main use case first: judging other people's apps, or gating code from the owner's own coding agents in CI? It sets defaults and time budgets.
3. Sandbox runtime for v1: local Docker only, or Kubernetes Jobs with gVisor or Kata as well?
4. Forges in v1: GitHub only, or GitLab too?
5. LLM provider and model for the small agentic layer, and is that layer allowed in v1 at all?
6. URL mode in v1, or repo mode only?
7. License, and whether the repo is public.

## Purpose and scope

The tool answers one question with evidence: does this app do what it claims, survive failure, and show real development, or is it surface without substance?

Slop means claims exceeding evidence: a polished UI, a confident README and lots of code, but features that don't work, fail silently, or were dumped in one shot. The failure mode is called a "Potemkin app".

v1 scope:

- **Apps:** web apps whose code is TypeScript, Python or Rust.
- **Stacks first:** Next.js, Vite SPA + Supabase, Express/Hono, FastAPI, Django, Flask, Axum, Actix.
- **Databases first:** Postgres (including the local Supabase stack) and SQLite.
- **Inputs:** a git repo for full evaluation; a live URL for safe checks only, if approved (question 6).
- **Delivery:** a CLI for local and CI use, writing HTML, JSON and SARIF reports.

Non-goals for v1:

- Judging whether code was written by AI. Provenance is reported, never scored.
- LLM-as-judge verdicts. Agents explore and propose; deterministic code decides.
- Mobile, desktop and CLI apps. The effect-record design allows them later.
- A hosted multi-tenant service. Later: Elixir/Phoenix with Oban and LiveView in front of the CLI.
- Invasive probes against systems the user doesn't own.

## Design principles

Every component follows these rules. When two conflict, the earlier one wins.

1. **Claims vs. evidence.** Slop is claims exceeding evidence. Every UI control, README promise, route, function name and test name is a claim; checks try to prove or break it.
2. **Judge the artifact, not the author.** Never score "written by AI". Score what the app does and how the project behaves.
3. **Agents explore, scripts verify.** LLM agents may explore, label and propose. Every verdict comes from a deterministic oracle over recorded data and replays without the LLM.
4. **Observe, then judge.** Probes produce immutable effect records; oracles are pure functions over them. A new oracle can re-judge old records without rerunning the app.
5. **Static hints, behavior proves.** A static finding is a hint and an explanation. It becomes a hard finding only when behavior confirms it, or when it is self-evident (a secret in a shipped bundle).
6. **No finding without evidence.** Every finding links a reproducible artifact (trace, DB diff, request, commit SHA) and a replay command.
7. **Same input, same verdict.** Seeds, clock, locale, snapshots, tool versions and model versions are pinned and recorded in the run manifest.
8. **Not measured is never pass.** A missing adapter, crashed probe or low coverage shows as `not_measured`, with coverage next to every score.
9. **Weight behavior over style.** Static tells are easy to game. Randomized behavioral probes are not, and gaming them produces a working app.
10. **Two families before calling slop.** A slop label needs agreeing evidence from at least two independent check families, for example history plus leftovers in the code.
11. **Separate axes.** "Works now", "will be maintained" and "is generic" are different questions. Never average them into one opaque number.
12. **Repo content is hostile data.** README, comments and code are untrusted input to any agent. Text addressed to AI evaluators is itself a finding (S12).

## System overview

_Pipeline diagram (7 stages, triage lane, agent and API side lanes): see the online doc, https://claude.ai/artifact/Lgf5vCggP4Fhhj5er3TLxN_

Read it top down: triage skips the sandbox and reports directly, the agent only helps exploration, and every verdict passes through the oracles.

Pipeline stages:

1. **Intake:** resolve the repo or URL, access mode and budget; write the run manifest.
2. **Triage (no sandbox):** history, static and dependency checks. Results go straight to the report; `--triage-only` stops here.
3. **Sandbox:** build ladder, clean-room boot, seed data, snapshot.
4. **Explore:** the deterministic crawler builds the AppModel and records traffic (HAR); the agent helps only when the crawler is stuck.
5. **Spec:** use the app's own OpenAPI spec, or infer one from recorded traffic; extract the route table in repo mode.
6. **Probe:** run catalog checks on fresh snapshots through the driver and recorder; API fuzzers run against the spec.
7. **Judge:** oracles turn effect records and tool outputs into findings, applying the 3-of-3 reproduction rule.
8. **Report:** scoring, hard gates, coverage, archetype, provenance panel and replay commands.

## Tech stack and repository layout

Rust is the core: it orchestrates, decides and scores. A thin TypeScript driver controls the browser, because Playwright is built in Node (even its Python bindings run a bundled Node process). Every other tool runs in a container, and Rust parses its JSON or SARIF output.

| Part | Choice | Notes |
| --- | --- | --- |
| CLI and orchestration | Rust, tokio, clap | One static binary for local and CI use |
| Sandbox control | bollard (Docker API); kube-rs later | One sandbox per evaluated app |
| Git history | gix (gitoxide) or git2 | Full clones only; refuse shallow clones |
| Static analysis | tree-sitter, ast-grep-core | Rules are YAML data under `rules/` |
| Data model | serde, schemars | JSON Schemas generated from Rust types |
| Driver types | ts-rs | TypeScript types generated from Rust, so the two sides can't drift |
| Browser driver | TypeScript, Playwright | Hands and eyes only; makes no decisions |
| Report | minijinja, serde-sarif | HTML, JSON and SARIF |
| Later | wasmtime, ort | Oracle plugins; ONNX attribution classifier |

External tools, pinned by image digest:

| Area | Tools |
| --- | --- |
| API fuzzing | Schemathesis; RESTler (optional) |
| Spec inference and drift | Redocly CLI (`generate-spec`, `drift`), mitmproxy2swagger |
| Fault injection | Toxiproxy |
| Mail sink | Mailpit |
| Mutation testing | StrykerJS, cargo-mutants, mutmut |
| Vulnerabilities and secrets | osv-scanner, npm audit, cargo-audit, pip-audit, gitleaks |
| Unused dependencies and licenses | knip, cargo-machete, deptry, cargo-deny, license-checker, pip-licenses |
| Lint hints | typescript-eslint, tsc, Clippy, Ruff, pyright |
| Slop pattern scanners (reused) | aislop, antislop, AI-SLOP Detector |
| Infra and migrations | hadolint, Trivy, kube-linter, squawk |
| Repo health | OpenSSF Scorecard, deps.dev API |
| Accessibility | axe-core, injected by the driver |

Repository layout:

```text
substance/
  crates/
    cli/           # binary: substance run | replay | bench | report
    core/          # EffectRecord, Finding, Verdict, Evidence, AppModel, RunManifest, CheckId
    orchestrator/  # pipeline stages, budgets, scheduling, retries
    sandbox/       # Docker via bollard, build ladder, snapshots, Toxiproxy, egress proxy, Mailpit
    recorder/      # runs the driver, adds DB/log/egress/email channels, normalizes, stores records
    crawler/       # deterministic state-graph crawler that builds the AppModel
    probes/        # active checks that drive the app and emit records
    oracles/       # pure functions over effect records, one module per check family
    history/       # git, forge and registry analysis, archetypes
    statics/       # tree-sitter and ast-grep rule runner
    adapters/      # stack adapters (ts, python, rust) and database adapters (postgres, sqlite)
    agent/         # optional LLM layer behind a cargo feature
    report/        # scoring, HTML/JSON/SARIF, replay commands
  driver/          # TypeScript Playwright driver (JSON-lines protocol)
  rules/           # ast-grep YAML: slop tells, escape hatches, leftovers, defect injections
  schemas/         # generated JSON Schemas and TypeScript types
  bench/           # defect-injection benchmark, corpora manifests, expected findings
  docs/decisions/  # ADRs
```

## Core data model

Five types carry everything. Define them once in `core`, generate JSON Schemas and TypeScript types from them, and never hand-write them twice.

**EffectRecord:** everything one action changed, across every channel. Immutable once written; large payloads are stored as content-addressed blobs (sha256) under the run directory.

```json
{
  "id": "rec_01H...",
  "run_id": "run_...",
  "snapshot": "snap_seeded_v1",
  "seed": 4211,
  "action": {"kind": "click", "target": "role=button[name='Save']", "screen": "/invoices/new", "input": {}},
  "injected": {"failure": null},
  "ui": {"url_before": "/invoices/new", "url_after": "/invoices/42", "dom_changed": true, "signals": ["success"], "download": null, "dialog": null},
  "net": [{"method": "POST", "path": "/api/invoices", "status": 201, "req_body": "blob:...", "res_body": "blob:..."}],
  "ws": [],
  "storage": {"local": [], "session": [], "indexeddb": [], "cookies": []},
  "db": {"insert": {"invoices": 1, "invoice_lines": 3}, "update": {}, "delete": {}, "rows": "blob:..."},
  "logs": {"console_errors": 0, "page_errors": 0, "server_errors": 0, "server_lines": "blob:..."},
  "queries": 7,
  "egress": [{"host": "api.openai.com", "method": "POST", "bytes": 812, "body": "blob:..."}],
  "email": [],
  "timing": {"settle_ms": 640, "timed_out": false}
}
```

**Finding:** `check_id`, `severity` (critical, high, medium, low, info), `confidence` (confirmed, flaky, hint), `title`, `evidence[]` (record IDs, blobs, commits, file and line ranges), `replay` (a CLI command) and `cause` (the static explanation, when one exists).

**Verdict** per check and target: `pass`, `fail`, `flaky`, `not_measured` (with a reason) or `unverifiable`.

**AppModel:** what exploration found, versioned by commit SHA.

- Screens (routes), controls (role, accessible name, selector candidates), forms (fields, types, constraints).
- Request map: which control triggers which requests.
- Entities inferred from responses and DB tables; test users per role.
- Labels from the agent layer: success-signal selector, entity list and detail screens, search field.

**RunManifest:** `as_of` timestamp, commit SHA, access mode, seeds, frozen clock value, timezone, locale, tool image digests, model IDs, rule-pack hash and scoring-weights version. Two runs with equal manifests must produce equal reports.

## Driver protocol and effect recorder

Rust sends one JSON command per line on the driver's stdin; the driver answers with one JSON event per line on stdout. The driver executes and observes the browser, nothing more.

Commands for v1:

| Command | Purpose |
| --- | --- |
| `open_context {viewport, locale, tz, storage_state?}` | New isolated browser context |
| `goto {url}` | Navigate, then settle |
| `list_controls {}` | Accessibility-tree walk: role, accessible name, selector candidates |
| `act {kind, target, value?}` | One action (click, fill, select, press or submit), then settle |
| `route {pattern, mode, ms?}` | Fail (500 or abort) or delay matching requests inside the browser |
| `clock {op, value}` | Set or advance the Playwright clock |
| `snapshot_dom {}` | Normalized DOM hash plus visible-text map |
| `axe {}` | Run axe-core on the page |
| `layout {}` | Element boxes for overflow, overlap and tap-target checks |
| `trace {op, path}` | Start or stop a Playwright trace for evidence |
| `close_context {}` | Dispose of the context |

Events: `ready`, `record_part` (the browser channels of one action), `error`, `done`.

The recorder merges the driver's part with the channels it captures itself into one EffectRecord:

| Channel | Captured by | How |
| --- | --- | --- |
| UI: DOM change, URL, dialogs, downloads | Driver | Normalized DOM hash before and after |
| Success and error signals | Driver | `role=status`, `role=alert`, `aria-live` regions, toast containers, agent-labeled selectors; never English word lists |
| HTTP | Driver | Request and response events; bodies to blobs |
| WebSocket | Driver | `framesent` and `framereceived` events |
| Browser storage | Driver | Diff of localStorage, sessionStorage, IndexedDB and cookies |
| Console and page errors | Driver | `console` and `pageerror` events |
| Database rows | Recorder | Postgres logical decoding (wal2json); SQLite `sqldiff` between file snapshots |
| Query count | Recorder | `pg_stat_statements` reset before the action, read after (SQLite: `not_measured` in v1) |
| Server logs | Recorder | Container stdout and stderr between action start and settle |
| Egress | Recorder | Egress proxy log: host, method, size, body |
| Email | Recorder | Mailpit API |

Replay: `substance replay <record-or-finding-id>` restores the record's snapshot, reruns its action with the same seed and diffs the new normalized record against the stored one.

## Determinism engineering

The same snapshot, seed and manifest must give byte-identical normalized records and the same verdict. Every rule below serves that.

- **Settle, don't sleep.** An action ends when no requests are in flight and the DOM hasn't changed for 500 ms (watched with a MutationObserver), capped by a timeout (default 10 s). Hitting the cap is recorded, not retried blindly.
- **Subtract the idle baseline.** Before probing a screen, record 5 s with nobody touching it (polling, analytics, websocket pings). Filter that traffic out of later records.
- **Normalize before hashing or diffing.** Strip timestamps, generated IDs, CSRF tokens, nonces and build hashes from DOM and payloads, using configurable rules.
- **Freeze the inputs.** One stored seed per random-input generator; a frozen clock in the browser (Playwright clock) and on the server (libfaketime); timezone UTC and locale en-US, except in checks that vary them on purpose.
- **Close the outside world.** Egress is blocked by default. Third-party APIs, including LLM APIs, are served from recordings or scripted stubs; email goes to Mailpit.
- **Reset state per probe.** Boot and seed once, snapshot, and give every probe a fresh copy: Postgres `CREATE DATABASE probe_n TEMPLATE seeded`, SQLite file copy, filesystem snapshots for the rest.
- **Stable selectors.** Role plus accessible name first, test IDs second, CSS last. A selector that no longer matches triggers re-exploration, never a failure.
- **Verdict rule.** A failure must reproduce 3 times out of 3 on fresh snapshots. Mixed results are reported as `flaky`, separately.
- **History snapshots.** Save raw forge and registry responses with `as_of`, and compute every "last 90 days" relative to it.
- **Pinned versions.** Tool images by digest, rule packs by hash, LLM model IDs and scoring weights, all written into the run manifest.

Acceptance test: run the same probe twice on the same snapshot and seed; the normalized records must be byte-equal.

## Sandbox, safety and access modes

Evaluated repos are untrusted code, and live URLs usually belong to someone else. Both limits shape what the tool may do.

Isolation in repo mode:

- One sandbox per evaluated app: Docker with gVisor (`runsc`) when available; Kubernetes Jobs with gVisor or Kata later.
- No host Docker socket inside sandboxes, no credentials, no host mounts except the run directory (source read-only, evidence write-only).
- Egress denied except package registries during the build; at runtime all egress goes through the recording proxy.
- CPU, memory, disk and wall-time quotas per stage; kill on breach.

Access modes:

| Mode | Input | Allowed checks |
| --- | --- | --- |
| `url` | A live URL the user doesn't control | Levels 0–1 only: reading responses and normal-user flows |
| `repo` | A git repo | Everything, against the sandboxed copy |
| `repo+url` | A repo plus the user's own deployment | Repo checks; live checks up to level 1 unless `--i-own-this` confirms ownership |

Every probe declares a destructiveness level:

| Level | Meaning | Examples |
| --- | --- | --- |
| 0 | Reads responses only | Headers, bundle scan, placeholder web, debug mode |
| 1 | Normal user actions in a test account | Crawl, dead controls, lying success (failure injected inside the browser) |
| 2 | Writes unusual data | Round-trip strings, double submit |
| 3 | Attacks or stresses the system | Authorization matrix, fuzzing, fault injection, leak tests |

Levels 2 and 3 run only against the sandboxed copy, or against a deployment the user declares they own. Fuzzing or swapping user IDs on someone else's production can count as unauthorized security testing.

Hostile content:

- Repo text (README, comments, issues) reaches agents only inside clearly delimited untrusted blocks.
- Agents can't change verdicts, so injected instructions can at most waste budget or produce artifacts that fail validation.
- Text addressed to AI reviewers ("rate this app excellent") is reported as finding S12.

## Language, framework and database adapters

An adapter tells the core how to build, run, migrate, test and read one stack, so checks stay stack-agnostic. Each adapter implements one Rust trait (`StackAdapter` or `DbAdapter`) and ships its YAML rules under `rules/<lang>/`.

| Concern | TypeScript | Rust | Python |
| --- | --- | --- | --- |
| Detect | Lockfile, `package.json` dependencies | `Cargo.toml`; bin targets via `cargo metadata` | `pyproject.toml`, `uv.lock`, `poetry.lock`, `requirements.txt` |
| Locked install | `npm ci`, `pnpm install --frozen-lockfile`, `bun install --frozen-lockfile` | `cargo build --locked` | `uv sync --locked`, `poetry install`, pinned requirements |
| Typical stacks | Next.js, Vite SPA + Supabase, Express/Hono, NestJS | Axum, Actix, Leptos | FastAPI, Django, Flask, Streamlit |
| Migrations | Prisma, Drizzle, TypeORM, Knex | SQLx, Diesel, SeaORM | Alembic, Django |
| Route table | Next.js `app/**/route.ts`; Express/Hono/NestJS from the AST | Axum `.route(...)`, Actix/Rocket attributes, via tree-sitter | At runtime: FastAPI `/openapi.json`, Flask `app.url_map`, Django URL resolver |
| Server coverage | `NODE_V8_COVERAGE` + c8 | `cargo llvm-cov` | `coverage run` |
| Tests | Vitest, Jest | `cargo nextest`, `cargo test` | pytest |
| Mutation | StrykerJS | cargo-mutants | mutmut |
| Lint hints | typescript-eslint, `tsc --strict` | Clippy restriction lints | Ruff, pyright |
| Dependencies | osv-scanner, knip | cargo-deny, cargo-machete | pip-audit, deptry |

Build ladder, first success wins; every failed rung is logged as boot friction:

1. The repo's `Dockerfile` or compose file.
2. `devcontainer.json`.
3. Cloud Native Buildpacks.
4. The agent follows the README (A01, budgeted).

Migration commands for O04: `prisma migrate deploy`, `drizzle-kit migrate`, `typeorm migration:run`, `knex migrate:latest`, `sqlx migrate run`, `diesel migration run`, `sea-orm-cli migrate up`, `alembic upgrade head`, `python manage.py migrate`.

Database adapters:

| Engine | Row diff | Query count | Snapshot and reset |
| --- | --- | --- | --- |
| Postgres | Logical decoding (wal2json) | `pg_stat_statements` | `CREATE DATABASE ... TEMPLATE` |
| SQLite | `sqldiff` between file copies | `not_measured` in v1 | File copy |
| Supabase | Postgres adapter on the local stack (`supabase start` with the repo's migrations) | `pg_stat_statements` | `CREATE DATABASE ... TEMPLATE` |

MySQL, MongoDB and the Firestore emulator come later. A missing adapter makes the checks that need it report `not_measured`, never `pass`.

Websocket-driven UIs (Phoenix LiveView, Blazor Server, Streamlit) send writes over a websocket. For them, inject failures between the app and its database with Toxiproxy instead of failing HTTP requests in the browser.

## Check catalog: behavioral checks

These checks drive the running app and judge its effect records; they carry most of the evaluator's weight. Level is the destructiveness level; M is the milestone that delivers the check.

| ID | Check | Fails when | Level | Needs | M |
| --- | --- | --- | --- | --- | --- |
| B01 | Dead controls | A control causes no effect in any channel | 1 | Browser | M4 |
| B02 | Lying success | With writes failed inside the browser, a success signal still appears | 1 | Browser | M4 |
| B03 | Phantom write | A write returns 2xx but the DB and storage diffs are empty | 1 | DB | M4 |
| B04 | Partial write | A failed request leaves rows behind | 1 | DB | M4 |
| B05 | Persistence ladder | Data is lost after navigation, reload, a new browser context, a server restart or a redeploy (the failing rung is reported) | 1 | Browser; DB for the last two rungs | M6 |
| B06 | Round-trip fidelity | Tricky values (emoji, RTL, combining marks, 10k characters, quotes, zero, negatives) don't come back identical, or markup in them renders as HTML | 2 | Browser, DB | M6 |
| B07 | Server-side validation | Invalid payloads sent straight to the API get 2xx or 5xx instead of 4xx | 3 | API | M7 |
| B08 | CRUD laws | Created items missing from list or search; deleted items still visible anywhere; filtered results not a subset; pages overlapping or incomplete; counts not matching rows; ascending sort not the reverse of descending | 2 | API (M7); UI labels (M12) | M7 |
| B09 | Double submit | A burst of clicks creates more than one record | 2 | DB | M6 |
| B10 | Authorization matrix | User B, or an anonymous client, replaying user A's recorded requests gets anything but 401, 403 or 404 | 3 | API | M6 |
| B11 | Mass assignment and client-trusted values | Extra fields (`role`, `isAdmin`, another user's `userId`) or a changed `price` or `total` get persisted | 3 | API, DB | M6 |
| B12 | Over-exposure | Responses carry fields the UI never displays; worst when named like `password`, `hash`, `token` or `email` | 1 | Browser | M6 |
| B13 | Canary tracing | Planted canary values (passwords, tokens, user A's email) appear in logs, URLs, browser storage, third-party requests or user B's responses | 2 | All channels | M6 |
| B14 | Logout replay | The old session cookie or token still works after logout | 3 | API | M6 |
| B15 | Fault recovery | A DB or third-party outage gives a white screen or partial writes, or the app needs a restart to recover | 3 | Toxiproxy | M8 |
| B16 | Hang test | With a dependency stalled, a request neither finishes nor fails within N seconds (default 30) | 3 | Toxiproxy | M8 |
| B17 | Scaling counts | Query count or response size grows with the number of rows (10 vs. 1,000 seeded): one query per row, or no pagination | 3 | DB | M8 |
| B18 | Bad routes and IDs | Unknown routes or IDs crash or go blank instead of showing a not-found page | 0 | Browser | M4 |
| B19 | Deep links | A discovered route fails when loaded directly in a fresh browser context | 0 | Browser | M4 |
| B20 | Five states | A view goes blank or spins forever when loading, empty (fresh account), failed (forced 500), partial or full | 2 | Browser | M9 |
| B21 | Keyboard-only flows | Sign-up or the core action can't be completed with Tab and Enter | 1 | Browser | M9 |
| B22 | Layout sanity | At 360, 768 or 1280 px: horizontal overflow, overlapping clickable elements, controls off-screen, or tap targets under 24×24 px | 0 | Browser | M9 |
| B23 | Timezone and locale | The same data shows a different instant or number under another timezone or locale | 2 | Clock, locale | M9 |
| B24 | Time travel | Moving the clock forward breaks token refresh, New Year, a daylight-saving switch, Feb 29 or scheduled jobs | 3 | Clock | M8 |
| B25 | Account deletion | Rows still reference a deleted user, beyond tables configured as legally retained | 2 | DB | M6 |
| B26 | Trackers before consent | Known tracker domains load before the cookie banner is answered | 0 | Egress, browser | M8 |
| B27 | Email claims | A flow that claims to send email (verification, reset, receipt) sends nothing, or sends the wrong content | 2 | Mailpit | M8 |
| B28 | Debug mode exposed | Django's DEBUG page, the Werkzeug debugger, tracebacks in responses, or Next.js or Vite dev servers serving real traffic (`/@vite/client`) | 0 | Browser | M4 |
| B29 | Placeholder web | `href="#"` links, social icons linking to a platform's homepage, placeholder image hosts, "© Your Company", or Privacy and Terms links that 404 on an app that collects data | 0 | Browser | M4 |
| B30 | Passive hygiene | Missing cookie flags or security headers, source maps in production, axe-core violations | 0 | Browser | M4 |
| B31 | Console warnings | Hydration mismatches, missing-key warnings, or uncaught errors during normal use | 1 | Browser | M4 |
| B32 | Frontend–backend contract | The UI calls a route that returns 404 or 405; a route nothing reaches (repo mode); one route lacking the auth guard its siblings have | 1 | Browser, repo | M6 |
| B33 | Supabase policies | Public tables without row-level security (`pg_class.relrowsecurity`), policies that allow everything (`true` in `pg_policies`), or a `service_role` JWT or `sb_secret_` key in the bundle | 0–3 | Repo, local stack | M6 |

## Check catalog: API, data model, operability and AI features

API checks reuse mature fuzzers instead of reimplementing them; the rest cover the database schema, production readiness and AI features inside the app.

API (I):

| ID | Check | Fails when | Level | M |
| --- | --- | --- | --- | --- |
| I01 | Schemathesis suite | Any built-in check fails: `not_a_server_error`, `status_code_conformance`, `negative_data_rejection`, `positive_data_acceptance`, `use_after_free`, `ensure_resource_availability`, `ignored_auth` | 3 | M7 |
| I02 | RESTler checkers (optional) | UseAfterFree, NameSpaceRule, ResourceHierarchy, LeakageRule, InvalidDynamicObjects or PayloadBody violations | 3 | M7 |
| I03 | Spec inference | Not a check: when no spec exists, infer OpenAPI from the crawl's HAR (Redocly `generate-spec` or mitmproxy2swagger) to feed I01 and I02 | — | M7 |
| I04 | Spec drift | Recorded traffic contradicts the OpenAPI spec the app ships (Redocly `drift`) | 0 | M7 |

Data model (D):

| ID | Check | Fails when | M |
| --- | --- | --- | --- |
| D01 | Schema quality | After migrations, the catalog shows missing foreign keys, nullable required fields, unindexed foreign keys, email columns without a unique constraint, or `timestamp` instead of `timestamptz` | M9 |
| D02 | Migration lint | squawk flags unsafe Postgres migrations | M9 |
| D03 | Money as float | Money-named columns or types (`price`, `amount`, `total`, `balance`) use float, real, `number` or `f64`; three items at 19.99 don't store exactly 59.97 | M9 |
| D04 | Startup schema traps | `prisma db push` in the start script, TypeORM `synchronize: true`, Sequelize `sync({ force: true })`, SQLAlchemy `drop_all()` at boot, raw `DROP TABLE` on startup, or SQLite under `/tmp`; a hint that B05 confirms | M1 |

Operability (O):

| ID | Check | Fails when | M |
| --- | --- | --- | --- |
| O01 | Locked install | Lockfile missing or inconsistent (M1), or the locked install fails (M2) | M1, M2 |
| O02 | Clean-room boot | Following only the README in a fresh sandbox doesn't produce a running app; every manual fix is logged as friction | M2 |
| O03 | Graceful shutdown | SIGTERM doesn't make the app exit cleanly within the grace period (default 30 s) | M2 |
| O04 | Idempotent migrations | Running migrations a second time fails | M2 |
| O05 | Non-root | The container runs as root | M2 |
| O06 | Lying health check | `/health`, or the configured probe path, returns 200 with the database stopped | M8 |
| O07 | Leak test | After 1,000 repetitions of one action, DB connections (`pg_stat_activity`), file handles or memory keep growing | M8 |
| O08 | Container hygiene | hadolint errors, or critical vulnerabilities in the image (Trivy) | M8 |
| O09 | Kubernetes manifests | Probe paths missing from the route table, or no resource limits (kube-linter) | M8 |
| O10 | Fake CI | Test steps that can't fail: `\|\| true`, `continue-on-error: true`, `--passWithNoTests` | M1 |
| O11 | Upgrade test | The previous release with seeded data fails to upgrade to HEAD, or data is lost afterwards (needs tags) | M8 |

AI features inside the app (L). The egress proxy stubs LLM APIs with scripted responses keyed by request hash, so these checks are deterministic:

| ID | Check | Fails when | Level | M |
| --- | --- | --- | --- | --- |
| L01 | Open paid-API proxy | An anonymous request triggers a call to a paid LLM API | 3 | M8 |
| L02 | Cost amplification | One user action triggers more than N paid API calls (default 3), or one call per keystroke | 1 | M8 |
| L03 | LLM failure fallback | With the stub returning 429 or 500, the app hangs or shows a fake answer | 3 | M8 |
| L04 | Model output as HTML | Stubbed model output `<img src=x onerror=...>` executes in the page | 3 | M8 |
| L05 | LLM keys in the client | Provider keys in the shipped bundle, for example `VITE_OPENAI_API_KEY` (Vite ships every `VITE_` variable to the browser) | 0 | M1, M4 |

## Check catalog: static, test and dependency checks

Static checks are cheap hints that explain behavior; dependency checks are cheap and often decisive. All of these run in M1 without a sandbox, except T01–T03.

Static (S), hints unless marked confirmed:

| ID | Check | Looks for |
| --- | --- | --- |
| S01 | Slop pattern scan | Reused scanners (aislop, antislop, AI-SLOP Detector): narrating comments, swallowed exceptions, invented imports, TODO stubs |
| S02 | Escape hatches | TypeScript: `any`, `as unknown as`, `@ts-ignore`, non-null `!`. Rust: `unwrap()` or `expect()` in handlers, `todo!()`, `#[allow(dead_code)]`, `let _ =`, `.ok()`. Python: bare `except: pass`, `# type: ignore`, `print` debugging, SQL built with f-strings |
| S03 | Swallowed writes | Unawaited DB writes (TypeScript), `let _ = query.execute(...)` (Rust), `except: pass` around a commit (Python); hint for B02 and B03 |
| S04 | Scaffold leftovers | Default package name (`vite-project`), uv's "Add your description here", page title "Vite + React + TS", the create-next-app README, default `vite.svg`, LICENSE with `[year] [fullname]`; matched by hash against known scaffolds |
| S05 | Agent leftovers | Report-style markdown in the root (`IMPLEMENTATION_SUMMARY.md`, `FIXES_APPLIED.md`, `*_COMPLETE.md`), one-off scripts (`test_fix.py`, `debug_*.js`, `verify_*.sh`), duplicate versions (`App_old.tsx`, `main_v2.py`, `utils_fixed.rs`, `*.backup`) |
| S06 | Committed junk | `node_modules`, `target/`, `__pycache__`, `dist/`, `.env` or `.DS_Store` tracked in git |
| S07 | Config contradictions | Several configs for one tool, `strict: false`, `#![allow(warnings)]` at the crate root, `pyproject.toml`, `setup.py` and `requirements.txt` disagreeing |
| S08 | Docs honesty | README commands that don't exist; environment variables the code reads but the docs never mention |
| S09 | Over-abstraction | Interfaces or traits with exactly one implementation; functions that only forward calls |
| S10 | Data-access sprawl | Number of files importing the database client directly |
| S11 | Secrets (confirmed) | Secrets in the tree and the full history (gitleaks); keys in built bundles |
| S12 | Evaluator manipulation | Text addressed to AI reviewers or evaluators |

Tests (T):

| ID | Check | Fails when | M |
| --- | --- | --- | --- |
| T01 | Tests pass | The suite fails or doesn't run | M10 |
| T02 | Mutation score | Most mutants survive in files the core flows touch (chosen with the exploration coverage map) | M10 |
| T03 | Test isolation | Failures under random order (pytest-randomly, `jest --randomize`), on a second run, or with the network blocked | M10 |
| T04 | Skipped and weak tests | A high share of skipped tests; few assertions per test; mostly `toBeTruthy` or `toBeDefined` | M1 |
| T05 | Test weakening | Commits that delete assertions, change expected values or add skip markers next to code changes, without adding tests | M1 |
| T06 | Late test batches | Tests arrive in one late batch instead of growing with the code | M1 |

Dependencies (P):

| ID | Check | Fails when | M |
| --- | --- | --- | --- |
| P01 | Known vulnerabilities | osv-scanner, npm audit, cargo-audit or pip-audit report issues | M1 |
| P02 | Dependency reputation | The package doesn't exist, was first published days before the project added it, has no source repo, or has few downloads and versions (slopsquatting risk) | M1 |
| P03 | Born-stale versions | Most dependencies were already outdated on the day they were added, judged by registry publish dates | M1 |
| P04 | Deprecated or yanked | Recently added packages that npm marks deprecated, or yanked releases on PyPI or crates.io | M1 |
| P05 | Bloat and overlap | Dependencies per thousand authored lines; several libraries for one job; unused dependencies (knip, cargo-machete, deptry) | M1 |
| P06 | Licenses | Dependency licenses incompatible with the project license (cargo-deny, license-checker, pip-licenses) | M1 |
| P07 | Dependency vitality | Direct dependencies that the history checks classify as abandoned or one-shot | M1 |

## Repo history and vitality

History answers "was this built or dumped, and will anyone fix it?" It needs no sandbox, runs in seconds, and goes first as triage.

Two rules for every metric:

- **Measure what is expensive to fake.** Commit counts, commit dates and stars are cheap to fake: agents commit at every step, commit dates are set by the client, and stars can be bought. Prefer server-side timestamps, time spread and other people.
- **Count authored lines only.** Exclude lockfiles, generated code (`linguist-generated`, `@generated`), vendored folders, minified files, snapshots, fixtures, i18n JSON and assets. Detect renames and moves, and ignore whitespace-only changes.

| ID | Signal | How it is computed | Cost to fake |
| --- | --- | --- | --- |
| H01 | Chronology | Server-side dates (repo creation, first PR, issue, CI run, registry release) vs. commit dates; commits older than the repo mean imported or backdated history | High |
| H02 | Active weeks | Distinct weeks with human commits or merged PRs, last 90 and 365 days | Medium |
| H03 | Big-bang ratio | Share of today's lines (via blame) that come from the project's first day | High |
| H04 | Commit size | Median and 90th percentile of authored lines; share of today's code from commits over 1,000 authored lines; giant commits after day one weigh more | Medium |
| H05 | Machine-speed commits | Authored lines divided by minutes since the same author's previous commit | Medium |
| H06 | Upload dumps | Commits with GitHub's browser-upload message "Add files via upload", weighted by size | Low |
| H07 | Fix chains | Commits minutes apart on the same files with "fix" messages and failing CI between them | Medium |
| H08 | Refactoring signals | GitClear's definitions: share of moved lines, copy/paste within one commit, age of the lines being changed | High |
| H09 | Short-lived code | Share of added lines deleted within 7 days | Medium |
| H10 | Message vs. diff | Generic messages ("update") on large diffs; share of generic messages overall | Low |
| H11 | Maintainers and bus factor | Distinct humans who merged PRs or wrote surviving lines (deduplicated with `.mailmap` and account IDs, bots excluded); bus factor = fewest people who wrote half of today's code | Medium |
| H12 | Review | Share of merged PRs approved by someone other than the author; rubber-stamp merges (under 1 minute, no comments) | Medium |
| H13 | Maintenance | Time to first response on outsiders' issues and PRs, time to merge security updates, libyear, CI status on the default branch | Medium–high |
| H14 | Real usage | Outside issue authors whose accounts have real history; dependents (deps.dev); downloads from npm, PyPI or crates.io | Medium |
| H15 | Silent tracker | Only the author ever opens or comments on issues | Medium |
| H16 | Claims vs. history | CHANGELOG versions with no tag or release; badges pointing at missing workflows or static images; "production-ready" or "battle-tested" with few active weeks | Medium |
| H17 | Template divergence | How much changed since the GitHub template origin or a known scaffold | Medium |
| H18 | Star spikes | Star bursts from freshly created accounts; a red flag only, never a positive signal | — |
| H19 | Repo farms | One account creating many one-shot repos in a short span; context only, since the project is judged, not the person | — |
| H20 | Velocity | Authored lines per active person-day; thousands per day for weeks means generated, almost certainly unreviewed code | Medium |
| H21 | OpenSSF Scorecard | Run Scorecard and import its checks (Maintained, Code-Review, Contributors, Branch-Protection, CI-Tests, Dangerous-Workflow, Pinned-Dependencies) | — |
| H22 | Archetype | The deterministic classification below | — |

Archetypes are reported instead of a single history score, with the numbers that triggered them. Thresholds come from the benchmark corpora.

| Archetype | Rule |
| --- | --- |
| One-shot dump | Most of today's code is from day one; few active weeks; no outside issues |
| Burst, then abandoned | Activity in the first weeks; nothing in the last 90 days |
| Young and iterating | Under 90 days old, but many active days, mostly edits, tests changing with the code |
| Solo maintained | Recently active; bus factor 1; answers issues |
| Team maintained | Two or more people merging; most PRs reviewed |
| Unverifiable history | Commit dates predate the repo, so the timeline can't be trusted |

False positives to handle:

- **Squash merges:** few commits but healthy history; use PR history instead.
- **Forks:** count only commits that aren't upstream.
- **Private work, then a public release:** looks like a big bang; judge what happens after the first public commit.
- **Bus factor 1:** normal for small, good projects; a risk flag, not slop.

P07 runs the same checks on every direct dependency.

## Provenance

Provenance gets its own report panel and never enters any score; the code is judged on what it does.

**V01 Declared provenance (M1).** Read what tools sign before guessing anything:

- **Claude Code** adds a commit trailer by default, configurable or removable through the `attribution.commit` setting; left unchanged, it names the session's model. Commits from cloud and Remote Control sessions can also carry a claude.ai session link.
- **Codex** adds `Co-authored-by: Codex <noreply@openai.com>` by default (`commit_attribution`); the value is static and doesn't name the model.
- **Aider's** convention tags the author name with "(aider)" and names the model as co-author.
- **Bot authors and branches:** coding-agent PRs and app builders committing as bots.
- **Instruction files:** `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.cursor/rules`, `.github/copilot-instructions.md`, `.kiro/steering`. These name the tool, not the model.

All of these can be turned off or stripped, so their absence proves nothing. Map them onto today's lines with blame.

**V02 Statistical attribution (later).** Published studies report roughly 88–97% accuracy in lab conditions; real repos are harder (human edits, mixed models, formatters, unseen models). Rules when it is built:

- Classify per commit or per changed block, above a minimum size; aim at the model family first.
- Report top guesses with probabilities; default to "unknown" or "unseen model".
- Train in Python with labels from V01 trailers and the benchmark corpus; export to ONNX; run from Rust via `ort` with pinned weights.

**V03 Knowledge-cutoff estimate (M1).** From P03's dependency vintage and outdated API styles (for example the old OpenAI Python SDK's `openai.ChatCompletion.create`), estimate when the generating model's knowledge ends.

What the panel shows and what it is for:

- Share of today's lines by declared tool and model, then the statistical guess with its confidence, then unknown.
- Per-model risk: AI-written code merged without review and without test changes.
- Aggregate data across many repos: which models and app builders produce which defect classes.
- Never use the statistical guess to accuse a person of using AI.

Code watermarks exist only in research, and detection usually needs vendor-private access, so they are out of scope.

## The small agentic layer

Agents are optional, budgeted and never decide verdicts. Every agent output becomes a frozen artifact that is validated before use, then cached per commit.

| ID | Job | Output artifact | Validation before use | M |
| --- | --- | --- | --- | --- |
| A01 | Unblock the crawler; boot from the README when the build ladder fails | Playwright script; boot steps | The script replays deterministically on a fresh snapshot | M12 |
| A02 | Label the app model | Success-signal selector, entity list and detail screens, search field | The selector appears in the baseline success run; every label resolves to an element | M12 |
| A03 | Propose domain invariants | Executable predicates, such as "invoice total = sum of lines + tax" | Must hold on the known-good seeded state, or it is discarded | M12 |
| A04 | Modifiability probe | A patch that adds one field end to end | Measured deterministically: files touched, tests broken, evaluator results after the change | M12 |
| A05 | Claim extraction | Checkable claims from the README and UI | Each claim maps to a check, or is marked unverifiable | Later |

Rules:

- The deterministic crawler runs first; the agent is called only when the crawler is stuck or needs meaning.
- Per-app budget in tokens and minutes; artifacts cached by commit SHA, so re-runs cost no tokens.
- Repo text reaches the agent only as delimited, untrusted data.
- Model IDs and prompt versions go into the run manifest.
- With the agent turned off, every check that doesn't need it must produce the same verdicts.

## Scoring and report

The report is a profile, not one number: separate axes, hard gates, coverage beside every score, and evidence behind every finding.

| Axis | Fed by |
| --- | --- |
| Works now | B01–B04, B08, B18–B21, I01 |
| Honest | B02, B03, I04, O06, S08, H16 |
| Robust | B05–B07, B09, B15–B17, B23, B24, O07, L03 |
| Safe | B10–B14, B25, B26, B28, B30, B33, I01 (`ignored_auth`), S11, P01–P04, L01, L04, L05 |
| Data integrity | D01–D04, B04, B05, B09 |
| Operable | O01–O05, O08–O11, B16 |
| Maintainable | T01–T06, S02, S09, S10, A04 |
| Vitality | H01–H22, P07, shown as archetype plus metrics |
| Provenance | V01–V03, a panel, never scored |
| Generic | G01–G02, later |

Scoring rules:

- **Severity is fixed per check** in the catalog, so scores stay deterministic.
- **Hard gates:** any one fails the app regardless of other results. Gates: data loss on a server rung of B05; lying success (B02); authorization breach (B10, B11, I01 `ignored_auth`); a secret or service key in the client (S11, B33, L05); an open paid-API proxy (L01); a canary leaking to a third party or another user (B13).
- **Weakest link within an axis:** the score follows the worst critical flow, not the average.
- **Coverage beside every score:** routes reached, controls exercised, server code executed (repo mode), checks measured. Low coverage lowers confidence, never raises the score.
- **Slop label:** needs agreeing evidence from two check families (principle 10).
- **Weights:** hand-set and versioned in v1; learned later (Later roadmap).

The summary combines the present and the future:

|  | Maintained | Abandoned or one-shot |
| --- | --- | --- |
| Works now | Safe to adopt | Works until it breaks, and nobody will fix it: freeze or fork it |
| Broken now | Being fixed: check back later | Slop |

Outputs per run:

- `report.html`: static and self-contained.
- `report.json`: schema-versioned.
- `report.sarif`: for CI and code scanning.
- `evidence/`: blobs, traces, DB diffs.
- Every finding includes `substance replay <finding-id>`, which reruns its probe on a fresh snapshot.

## Evaluating the evaluator

The evaluator ships a check only when it catches known bugs, stays quiet on good apps and gives the same answer twice.

Defect-injection benchmark in `bench/`:

- **Base apps:** RealWorld (Conduit) implementations in TypeScript, Python and Rust that pass RealWorld's own API test suite, plus one Vite + Supabase app and one Next.js + Prisma app.
- **Injections:** ast-grep rewrite rules, the same engine as the static checks. One defect per variant, each naming the checks that must catch it.
- **Metrics:** recall per check, false positives on the untouched base apps, score spread over 5 runs of the same app, and agreement with a few engineers' "which of these two is more solid?" picks.
- **Regression gate:** no change merges if recall drops or false positives rise.

Starting set of injections:

| Injection | Language | Must be caught by |
| --- | --- | --- |
| Remove `await` before a DB write | TypeScript | B03, B02, S03 |
| Delete `Depends(get_current_user)` | Python | B10, I01 |
| Turn `.await?` into `.await.ok()` | Rust | B02, B04, S02 |
| Swap an error toast for a success toast | TypeScript | B02 |
| Remove an ownership check | All | B10 |
| Replace the DB with an in-memory store | All | B05 |
| Remove a pagination limit | All | B17 |
| Log the request body in the login handler | All | B13 |
| Pass the request body straight to the ORM | All | B11 |
| Make `/health` return 200 unconditionally | All | O06 |

Corpora for calibration:

- **Slop corpus:** apps generated from VIBE's 200 public task specs with several models and app builders.
- **Trusted corpus:** small, maintained open-source apps per language.
- **Thresholds** (H04 commit sizes, H20 velocity, archetype cut-offs) are set from these corpora per language, never guessed.

## Milestones

Build in this order; a milestone is done when its acceptance criteria pass in CI. Time and recall figures are starting targets, to be adjusted after the first benchmark run.

| # | Milestone | Deliverables | Acceptance criteria |
| --- | --- | --- | --- |
| M0 | Foundations | Cargo workspace, `core` types, JSON Schema and TypeScript type generation, CLI skeleton, run manifest, `report.json` v0 | Schemas regenerate in CI with no diff; round-trip tests for every core type |
| M1 | Triage without a sandbox | H01–H22, S01–S12, D04, O01 (static), O10, T04–T06, P01–P07, V01, V03, L05 (repo), HTML report v0 | Runs on a public repo in under 2 minutes after cloning; the same `as_of` snapshot gives an identical report |
| M2 | Sandbox and boot | Build ladder; TypeScript adapters (Next.js, Vite + Supabase, Express/Hono); Postgres and SQLite adapters; seeding; snapshot and reset; O01–O05 | RealWorld TypeScript boots from a clean clone; snapshot reset works per probe |
| M3 | Recorder and driver | TypeScript driver, JSON-lines protocol, all channels, normalization, idle baseline, settling, `replay` | The same action twice on the same snapshot gives byte-identical normalized records |
| M4 | Crawler and first oracles | Deterministic crawler, AppModel, B01–B04, B18, B19, B28–B31, L05 (bundle) | Zero findings on unmodified RealWorld TypeScript |
| M5 | Benchmark v1 | `bench/` with the TypeScript base apps and their injections | Each M4 check catches its own injections (target: recall at least 90%) |
| M6 | Data and trust probes | B05, B06, B09–B14, B25, B32, B33 | Injections caught; no false positives on base apps |
| M7 | API layer | Spec inference, Schemathesis, RESTler (optional), drift, B07, API-level B08 | A FastAPI app and an inferred-spec app are fuzzed; findings deduplicated against B checks |
| M8 | Resilience, operations and channels | Toxiproxy, egress proxy with LLM stub, Mailpit; B15–B17, B24, B26, B27, O06–O09, O11, L01–L04 | Every new check catches at least one injected defect |
| M9 | Front-end completeness and data model | B20–B23, D01–D03 | Each check verified on a base app and an injected variant |
| M10 | Test truthfulness | T01–T03, with mutation testing limited by the coverage map | Mutation runs stay within the configured time budget |
| M11 | Python and Rust packs | FastAPI, Django, Flask, Streamlit, Axum and Actix adapters; language rules; RealWorld Python and Rust in `bench/` | Benchmark recall per language within 10 points of TypeScript |
| M12 | Agentic layer | A01–A04; UI-level B08 using A02 labels | With the agent off, verdicts are unchanged for every check that doesn't need it |
| M13 | Scoring v1 and report | Axes, hard gates, coverage, archetypes, the summary matrix, provenance panel, SARIF, replay | Every finding has evidence and a replay command that reproduces it |

## Later roadmap

These were agreed as direction, not v1 scope; keep the v1 design open for them.

- **G01 Genericness, regenerate-and-diff:** infer the shortest prompt that would produce the app, regenerate 3–5 apps from it with different agents, and diff feature inventories. What the baselines don't reproduce is the app's real intent. Cache baselines per app category.
- **G02 Domain-rule density:** count domain rules, edge cases and constraints the code encodes beyond generic CRUD.
- **V02 Statistical attribution** (see Provenance).
- **Learned scoring weights:** logistic regression on the indicators from the labeled corpus, with each indicator's contribution shown ("giant commits after day one: odds ×6"); weights versioned.
- **Oracle plugins:** oracles are pure functions over records, so third parties can ship them as WebAssembly, run in wasmtime with fuel limits.
- **Per-PR incremental runs:** re-run only checks whose code changed (from the coverage map) and chart trends to show regressions.
- **Payments channel:** Stripe test mode for checkout flows.
- **More databases and interfaces:** MySQL, MongoDB, the Firestore emulator; mobile via Appium or Maestro; CLIs via a pseudo-terminal; desktop via OS accessibility APIs.
- **Service mode:** Elixir/Phoenix with Oban as the job queue and LiveView as the dashboard, running the CLI per job.
- **Fix tasks for coding agents:** export findings as SARIF plus a reproducer, then re-run to confirm the fix.
- **A05 Claim extraction** from the README and UI.

## Engineering conventions

The evaluator must pass its own checks, so build it the way it judges other projects.

- **Rust:** 2024 edition; `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::todo)]` in library crates; `thiserror` in libraries, `anyhow` only in the CLI; `tracing` for logs.
- **TypeScript driver:** `strict` tsconfig, no `any`, typescript-eslint `no-floating-promises` enabled.
- **Oracles:** pure functions with table-driven tests over recorded EffectRecords (golden files under `crates/oracles/tests/fixtures/`).
- **Probes:** every probe has an integration test against a benchmark app, with and without its injected defect.
- **Determinism in CI:** run the M4 suite twice on the same snapshot and diff the normalized outputs.
- **Versions:** tool images pinned by digest, rule packs versioned, and any schema change bumps the `report.json` version.
- **Decisions:** one ADR per decision in `docs/decisions/`; check IDs are stable and never reused.
- **Dogfooding:** run `substance` on its own repo in CI from M1 onward.
- **History hygiene:** small, descriptive commits with tests alongside the code. The history checks will judge this repo too.

## Glossary and references

| Term | Meaning |
| --- | --- |
| Effect record | Immutable record of everything one action changed, across all channels |
| Oracle | Pure function over effect records that returns a verdict |
| Probe | Active procedure that drives the app and produces effect records |
| Check | A probe and oracle pair with a stable ID, severity and destructiveness level |
| Finding | A failed check, with evidence and a replay command |
| AppModel | Screens, controls, forms, requests and entities found by exploration |
| Settle | The moment an action counts as finished |
| Idle baseline | Traffic a page produces with no user action |
| Destructiveness level | 0–3 rating of how invasive a probe is |
| Archetype | History classification, such as one-shot dump or team maintained |
| Potemkin app | An app whose surface promises features its substance doesn't deliver |

Sources:

- [Schemathesis checks reference](https://schemathesis.readthedocs.io/en/stable/reference/checks/)
- [Schemathesis CLI reference](https://schemathesis.readthedocs.io/en/stable/reference/cli/)
- [RESTler FAQ (checkers)](https://github.com/microsoft/restler-fuzzer/blob/0beec440c801aed6191b2d67c0a846a5bf9088ca/docs/user-guide/FAQ.md)
- [Checking Security Properties of Cloud Services REST APIs](https://www.microsoft.com/en-us/research/uploads/prod/2019/02/paper2.pdf)
- [Redocly CLI generate-spec and drift](https://redocly.com/docs/cli/commands/generate-spec)
- [mitmproxy2swagger](https://docsearch.algolia.com/mcp/docs/repo/alufers/mitmproxy2swagger)
- [WebGen-Bench (NeurIPS 2025)](https://neurips.cc/virtual/2025/poster/121451)
- [Vibe Code Bench v1.1](https://benchlm.ai/md/benchmarks/vibecodebench.md)
- [VIBE benchmark task specs](https://huggingface.co/datasets/MiniMaxAI/VIBE/commit/9f6120089eb2a9c6bb63cf2ea202d706007f3f70)
- [Package hallucination study](https://server-nexa.polito.it/hyperkitty/list/nexa@server-nexa.polito.it/thread/KRNUNFZXRASBF7XXFJP5ZYDIBW55AZM5/)
- [CSA research note on slopsquatting](https://labs.cloudsecurityalliance.org/research-rb/csa-research-note-slopsquatting-ai-coding-assistant-supply-c/)
- [GitClear report on AI and code quality (DevClass)](https://devclass.com/2025/02/20/ai-is-eroding-code-quality-states-new-in-depth-report/)
- [Claude Code settings reference (attribution)](https://code.claude.com/docs/en/settings-reference.md)
- [Claude Code attribution default (Classmethod)](https://dev.classmethod.jp/en/articles/claude-code-attribution-setting/)
- [Codex CLI commit attribution](https://codex.danielvaughan.com/2026/03/28/codex-cli-commit-attribution/)
- [LLM code stylometry for authorship attribution](https://deepfake-demo.aisec.fraunhofer.de/related_work/2506.17323)
- [Attribution of LLM-generated JavaScript](https://huggingface.co/papers/2510.10493)
- [aislop](https://pypi.org/project/aislop/)
- [AI-SLOP Detector](https://pypi.org/project/ai-slop-detector/)
- [antislop](https://docs.rs/antislop)
