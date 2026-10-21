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
