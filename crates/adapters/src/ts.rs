//! The three TypeScript stacks of M2-T02: Next.js, Vite and a plain Node server (Express/Hono).
//!
//! ## Package manager from the lockfile
//!
//! The lockfile is the only honest source: `packageManager` in `package.json` is advisory, and a
//! repository with a `pnpm-lock.yaml` and no `packageManager` field still wants pnpm. Each
//! manager's **locked** install is used, because an unlocked install lets a dependency resolve
//! differently on two runs of the same commit, which breaks "same input, same verdict" (ADR-0009).
//!
//! Two lockfiles are a real ambiguity, and the plan does not resolve it, so this module picks by a
//! stated rule: if more than one lockfile is present, the one whose manager is named in
//! `package.json`'s `packageManager` wins; without that field, npm wins, because `npm ci` is the
//! only command every Node host can run. A reader who disagrees can pin `[app.install]` in
//! `substance.toml`, which is rung 2 and outranks this.
//!
//! ## Production mode first
//!
//! The boot table's commands are production ones (`next start`, `vite preview`) rather than the
//! dev servers the READMEs use, because a dev server carries development markers: B28 looks for
//! exactly those, and a dev server would make it fire on every app. `vite preview` also needs
//! `--strictPort`, because it otherwise silently moves to another port when the chosen one is
//! taken, and substance would then poll a port nothing is listening on.

use substance_core::config::ProjectConfig;
use substance_core::model::{CommandSpec, Measured, RouteTable};

use crate::stack::{
    BootPlan, Confidence, Detection, MigrationTool, PackageJson, ROUTES_LATER, RepoView,
    StackAdapter,
};

/// The placeholder an argument uses for the allocated port.
///
/// The same string the runner fills (M2-T01), so a plan can put the port on a command line as well
/// as in the environment; `vite preview --port {port}` needs both spellings to agree.
pub const PORT_PLACEHOLDER: &str = "{port}";

/// The Node package managers this build knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackageManager {
    /// `npm ci`.
    Npm,
    /// `pnpm install --frozen-lockfile`.
    Pnpm,
    /// `bun install --frozen-lockfile`.
    Bun,
    /// `yarn install --immutable` on Yarn 2+, `yarn install --frozen-lockfile` on Yarn 1.
    Yarn,
}

impl PackageManager {
    /// The manager's name, as evidence and friction logs spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Yarn => "yarn",
        }
    }

    /// The locked install command, for the manager without a version-specific flag choice.
    ///
    /// Only yarn has two spellings, so this returns the Yarn **1** command; a caller holding a
    /// [`ManagerChoice`] uses [`ManagerChoice::install`] instead, which knows the version. The
    /// signature is kept for the non-yarn managers, where there is nothing to know.
    #[must_use]
    pub fn install_plain(self) -> CommandSpec {
        self.install_for(None)
    }

    /// The locked install command, with the yarn major when it is known.
    ///
    /// Every arm fails rather than resolving a new version, which is the property that makes a run
    /// reproducible.
    #[must_use]
    fn install_for(self, yarn_major: Option<u8>) -> CommandSpec {
        match self {
            Self::Npm => CommandSpec::new("npm", &["ci"]),
            Self::Pnpm => CommandSpec::new("pnpm", &["install", "--frozen-lockfile"]),
            Self::Bun => CommandSpec::new("bun", &["install", "--frozen-lockfile"]),
            Self::Yarn => match yarn_major {
                // Yarn 2 and later renamed the flag; Berry rejects `--frozen-lockfile`.
                Some(major) if major >= 2 => CommandSpec::new("yarn", &["install", "--immutable"]),
                _ => CommandSpec::new("yarn", &["install", "--frozen-lockfile"]),
            },
        }
    }

    /// The command that runs a `package.json` script, for this manager.
    #[must_use]
    pub fn run_script(self, script: &str) -> CommandSpec {
        match self {
            Self::Npm => CommandSpec::new("npm", &["run", script]),
            // pnpm and bun both accept `run`; yarn's `run` is optional but harmless and keeps all
            // four spellings the same shape.
            Self::Pnpm => CommandSpec::new("pnpm", &["run", script]),
            Self::Bun => CommandSpec::new("bun", &["run", script]),
            Self::Yarn => CommandSpec::new("yarn", &["run", script]),
        }
    }

    /// The command that runs a `node_modules/.bin` program, never installing one.
    ///
    /// ## Why not `npx`
    ///
    /// `npx next build` does not fail when `next` is missing from `node_modules`: it fetches the
    /// package from the registry and runs it. That is wrong twice over. The boot would depend on
    /// the network, so the same commit could boot on one host and not another (ADR-0009), and the
    /// egress would be invisible -- the plan wants network use *measured*, not hidden inside a
    /// command that looks local (§1 forbids `npx` for exactly this reason). A missing binary must
    /// be friction, which is a fact about the repository, not a download.
    ///
    /// ## The local-only form, per manager
    ///
    /// Each of these was run on 2026-10-21 in a directory holding `node_modules/.bin/hello` and a
    /// `package.json` that names no dependencies: the local case printed the marker, and the
    /// missing-binary case failed without fetching.
    ///
    /// | manager | command | with a missing binary |
    /// | --- | --- | --- |
    /// | pnpm | `pnpm exec <bin> <args>` | `ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL`, no fetch |
    /// | bun | `bun x --no-install <bin> <args>` | "Stopping because --no-install was passed" |
    /// | npm | `npm exec --no -- <bin> <args>` | **still fetched**, so npm does not use it |
    /// | yarn | `yarn exec` (1 only) | removed in Yarn 2, so yarn does not use it either |
    ///
    /// ## npm invokes the binary directly
    ///
    /// npm has no local-only exec form. `npm exec --no` contacted the registry and reported a 404
    /// for a package that does not exist, and `npm exec --offline` failed with `ENOTCACHED`
    /// instead of running the local binary -- one of those is a download that reached the network.
    /// So npm is invoked straight from `node_modules/.bin`, resolved against the work copy that is
    /// the step's `cwd`. The trade is that a binary npm would have found in a *parent*
    /// `node_modules` is now missed, which is the behaviour we want: the run installed exactly one
    /// dependency tree, and a tool from outside it is not the tool we measured.
    ///
    /// A repository whose install produced no such binary fails the step with "No such file or
    /// directory", and that failure is friction -- the honest outcome for an install that did not
    /// produce the tool its own build needs.
    #[must_use]
    pub fn exec(self, program: &str, args: &[&str]) -> CommandSpec {
        let mut all = vec![program];
        all.extend_from_slice(args);
        match self {
            // npm and yarn invoke the binary directly. Neither has a local-only exec form: npm's
            // `exec` fetches (see above), and Yarn 2+ removed `yarn exec` altogether, while Yarn
            // 1's `yarn <bin>` prefers a `package.json` script of the same name. The direct path
            // needs no version knowledge and no flag that one major accepts and another rejects.
            //
            // Relative, and the step's `cwd` is the work copy, so it resolves inside the tree
            // under test and nowhere else.
            Self::Npm | Self::Yarn => {
                let path = format!("node_modules/.bin/{program}");
                CommandSpec::new(&path, args)
            }
            Self::Pnpm => CommandSpec::new("pnpm", &exec_args("exec", &all)),
            // bun's `x` *does* install unless told not to, and `--no-install` is its documented
            // opt-out, so this is the one manager that keeps an exec verb.
            Self::Bun => CommandSpec::new("bun", &exec_args("x --no-install", &all)),
        }
    }
}

/// A local-exec argument list: the verb (and its flags), then the program and its arguments.
///
/// One function so that `--no-install` travels with the verb it modifies, rather than being a
/// flag a caller could forget to pass.
fn exec_args<'a>(verb: &'a str, all: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&'a str> = verb.split(' ').collect();
    out.extend_from_slice(all);
    out
}

/// Which manager and lockfile the repository uses, or why none was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagerChoice {
    /// A lockfile was found.
    Found {
        /// The manager.
        manager: PackageManager,
        /// The lockfile, as evidence.
        lockfile: String,
        /// The yarn major version, when the manager is yarn and the version was readable.
        yarn_major: Option<u8>,
    },
    /// No lockfile was found, so no locked install is possible.
    None,
}

impl ManagerChoice {
    /// The locked install command, or `None` when there is no lockfile.
    ///
    /// On the choice rather than on the manager, because the yarn flag depends on the version the
    /// choice carries: a caller cannot ask for the command and leave the version behind.
    #[must_use]
    pub fn install(&self) -> Option<CommandSpec> {
        match self {
            Self::Found {
                manager,
                yarn_major,
                ..
            } => Some(manager.install_for(*yarn_major)),
            Self::None => None,
        }
    }

    /// The manager, when one was found.
    #[must_use]
    pub fn manager(&self) -> Option<PackageManager> {
        match self {
            Self::Found { manager, .. } => Some(*manager),
            Self::None => None,
        }
    }
}

/// Reads the package manager out of the lockfiles present.
///
/// The rule for more than one lockfile is documented at the top of this module: the one named in
/// `packageManager` wins, and npm wins otherwise.
#[must_use]
pub fn package_manager(repo: &RepoView) -> ManagerChoice {
    // Checked in a fixed order so the choice never depends on directory iteration order.
    let candidates = [
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("bun.lockb", PackageManager::Bun),
        ("bun.lock", PackageManager::Bun),
        ("yarn.lock", PackageManager::Yarn),
        ("package-lock.json", PackageManager::Npm),
    ];
    let present: Vec<(&str, PackageManager)> = candidates
        .iter()
        .copied()
        .filter(|(name, _)| repo.exists(name))
        .collect();
    if present.is_empty() {
        return ManagerChoice::None;
    }
    if present.len() == 1 {
        let (name, manager) = present[0];
        return ManagerChoice::Found {
            manager,
            lockfile: name.to_owned(),
            yarn_major: yarn_major(repo, manager),
        };
    }
    let declared = declared_manager(repo);
    let chosen = declared
        .and_then(|wanted| {
            present
                .iter()
                .find(|(_, manager)| *manager == wanted)
                .copied()
        })
        .or_else(|| {
            present
                .iter()
                .find(|(_, manager)| *manager == PackageManager::Npm)
                .copied()
        })
        .unwrap_or(present[0]);
    ManagerChoice::Found {
        manager: chosen.1,
        lockfile: chosen.0.to_owned(),
        yarn_major: yarn_major(repo, chosen.1),
    }
}

/// The manager `package.json`'s `packageManager` field names, when it does.
fn declared_manager(repo: &RepoView) -> Option<PackageManager> {
    let text = repo.read("package.json")?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let declared = value.get("packageManager")?.as_str()?;
    // The field is `name@version`, sometimes with a hash suffix (`pnpm@9.1.0+sha256.…`).
    let name = declared.split('@').next().unwrap_or(declared);
    match name {
        "npm" => Some(PackageManager::Npm),
        "pnpm" => Some(PackageManager::Pnpm),
        "bun" => Some(PackageManager::Bun),
        "yarn" => Some(PackageManager::Yarn),
        _ => None,
    }
}

/// Yarn's major version, when this is a yarn repository and the version is readable.
///
/// Read from `.yarnrc.yml`, which Yarn 2+ requires and Yarn 1 never has. That is enough to pick
/// `--immutable` over `--frozen-lockfile`, and it needs no process run: `yarn --version` would
/// execute repo-controlled code, which triage must not do.
fn yarn_major(repo: &RepoView, manager: PackageManager) -> Option<u8> {
    if manager != PackageManager::Yarn {
        return None;
    }
    if repo.exists(".yarnrc.yml") {
        return Some(2);
    }
    // Yarn 1 writes a `# yarn lockfile v1` header.
    match repo.read("yarn.lock") {
        Some(text) if text.contains("yarn lockfile v1") => Some(1),
        Some(_) => Some(2),
        None => None,
    }
}

/// The parts of a boot plan all three stacks share: install, the port, the ready path.
///
/// Written once so the three adapters cannot disagree about, say, which variable carries the port.
fn common(repo: &RepoView, cfg: &ProjectConfig, id: &str) -> BootPlan {
    let mut plan = BootPlan {
        source: id.to_owned(),
        install: None,
        build: None,
        migrate: None,
        seed: cfg.app.seed.as_ref().map(|line| shell_command(line)),
        start: None,
        env: cfg.app.env.clone(),
        port_env: cfg.app.port_env.clone(),
        ready_path: cfg.app.ready_path.clone(),
    };
    plan.install = package_manager(repo).install();
    plan
}

/// A `substance.toml` command line as a `CommandSpec`.
///
/// The config holds one line per command (`npm ci`, `next start -p $PORT`), so it is split on
/// whitespace. `$PORT` and `${PORT}` are left to the runner, which fills the port variable; the
/// split only has to avoid mangling them, and whitespace splitting does not.
#[must_use]
pub fn shell_command(line: &str) -> CommandSpec {
    let mut parts = line.split_whitespace();
    let program = parts.next().unwrap_or_default().to_owned();
    CommandSpec {
        program,
        args: parts.map(str::to_owned).collect(),
        env: std::collections::BTreeMap::new(),
    }
}

/// The manager a repository uses, falling back to npm when it has no lockfile.
///
/// Used only where a plan needs *some* command; the friction log records the fallback, because an
/// unlocked install is a fact O02 and a reader both want to know.
fn manager_or_npm(repo: &RepoView) -> PackageManager {
    package_manager(repo)
        .manager()
        .unwrap_or(PackageManager::Npm)
}

/// Next.js.
#[derive(Debug, Clone, Copy, Default)]
pub struct NextJs;

impl StackAdapter for NextJs {
    fn id(&self) -> &'static str {
        "ts-next"
    }

    fn detect(&self, repo: &RepoView) -> Option<Detection> {
        let package = repo.package_json()?;
        let mut evidence = Vec::new();
        let mut confidence = Confidence::Weak;
        if package.depends_on("next") {
            evidence.push("package.json: next".to_owned());
            confidence = Confidence::Strong;
        }
        for name in ["next.config.js", "next.config.mjs", "next.config.ts"] {
            if repo.exists(name) {
                evidence.push(name.to_owned());
                confidence = confidence.max(Confidence::Weak);
            }
        }
        if evidence.is_empty() {
            return None;
        }
        evidence.sort();
        Some(Detection {
            adapter: self.id(),
            evidence,
            confidence,
        })
    }

    fn boot_plan(&self, repo: &RepoView, cfg: &ProjectConfig) -> BootPlan {
        let mut plan = common(repo, cfg, self.id());
        let manager = manager_or_npm(repo);
        // Production mode: build, then serve the build. `next start` refuses to run without one.
        plan.build = Some(manager.exec("next", &["build"]));
        plan.start = Some(manager.exec("next", &["start", "-p", PORT_PLACEHOLDER]));
        // The default Next port is 3000 and `-p` overrides it, but the environment variable is
        // exported as well: a custom `server.js` may read it instead of the flag.
        plan.env
            .insert(cfg.app.port_env.clone(), PORT_PLACEHOLDER.to_owned());
        plan
    }

    fn migrations(&self, repo: &RepoView) -> Option<MigrationTool> {
        // Next is a front end; its database is whatever the repo put behind it.
        detect_migration_tool(repo)
    }

    fn route_table(&self, _repo: &RepoView) -> Measured<RouteTable> {
        // `app/**/route.ts` is readable statically, but the shape M6 needs (methods, parameters)
        // is not settled yet, so this says so rather than guessing.
        Measured::not_measured(ROUTES_LATER)
    }

    fn test_command(&self, _repo: &RepoView) -> Option<CommandSpec> {
        None
    }
}

/// A Vite single-page app, with Supabase when the repository carries a local Supabase config.
#[derive(Debug, Clone, Copy, Default)]
pub struct Vite;

impl Vite {
    /// Whether the repository runs a local Supabase stack.
    #[must_use]
    pub fn uses_supabase(repo: &RepoView) -> bool {
        repo.exists("supabase/config.toml")
    }
}

impl StackAdapter for Vite {
    fn id(&self) -> &'static str {
        "ts-vite"
    }

    fn detect(&self, repo: &RepoView) -> Option<Detection> {
        let package = repo.package_json()?;
        let mut evidence = Vec::new();
        let mut confidence = Confidence::Weak;
        if package.depends_on("vite") {
            evidence.push("package.json: vite".to_owned());
            confidence = Confidence::Strong;
        }
        for name in ["vite.config.ts", "vite.config.js", "vite.config.mts"] {
            if repo.exists(name) {
                evidence.push(name.to_owned());
            }
        }
        if evidence.is_empty() {
            return None;
        }
        if Self::uses_supabase(repo) {
            evidence.push("supabase/config.toml".to_owned());
        }
        evidence.sort();
        Some(Detection {
            adapter: self.id(),
            evidence,
            confidence,
        })
    }

    fn boot_plan(&self, repo: &RepoView, cfg: &ProjectConfig) -> BootPlan {
        let mut plan = common(repo, cfg, self.id());
        let manager = manager_or_npm(repo);
        plan.build = Some(manager.exec("vite", &["build"]));
        // `--strictPort` matters: without it `vite preview` moves to the next free port when the
        // one it was given is taken, and substance would poll a port nothing is listening on.
        plan.start = Some(manager.exec(
            "vite",
            &["preview", "--port", PORT_PLACEHOLDER, "--strictPort"],
        ));
        plan.env
            .insert(cfg.app.port_env.clone(), PORT_PLACEHOLDER.to_owned());
        if Self::uses_supabase(repo) {
            // The local stack is started by M2-T03's DbAdapter; the plan only records that the app
            // expects it, so a boot without it fails with a message about the database rather than
            // about a missing environment variable.
            plan.env.insert(
                "SUPABASE_URL".to_owned(),
                "http://127.0.0.1:54321".to_owned(),
            );
        }
        plan
    }

    fn migrations(&self, repo: &RepoView) -> Option<MigrationTool> {
        detect_migration_tool(repo)
    }

    fn route_table(&self, _repo: &RepoView) -> Measured<RouteTable> {
        Measured::not_measured(ROUTES_LATER)
    }

    fn test_command(&self, _repo: &RepoView) -> Option<CommandSpec> {
        None
    }
}

/// A Node HTTP server: Express, Hono, Fastify or bare `node`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeServer;

impl NodeServer {
    /// The frameworks this adapter claims.
    const FRAMEWORKS: &'static [&'static str] = &["express", "hono", "fastify", "koa"];
}

impl StackAdapter for NodeServer {
    fn id(&self) -> &'static str {
        "ts-node-server"
    }

    fn detect(&self, repo: &RepoView) -> Option<Detection> {
        let package = repo.package_json()?;
        let mut evidence = Vec::new();
        for framework in Self::FRAMEWORKS {
            if package.depends_on(framework) {
                evidence.push(format!("package.json: {framework}"));
            }
        }
        // A server needs a start script; without one there is nothing to boot even if it is a
        // server library user.
        let has_start = package.has_script("start");
        if evidence.is_empty() {
            return None;
        }
        if !has_start {
            return None;
        }
        evidence.push("package.json: scripts.start".to_owned());
        evidence.sort();
        Some(Detection {
            adapter: self.id(),
            evidence,
            confidence: Confidence::Strong,
        })
    }

    fn boot_plan(&self, repo: &RepoView, cfg: &ProjectConfig) -> BootPlan {
        let mut plan = common(repo, cfg, self.id());
        let manager = manager_or_npm(repo);
        // The table's rule: the build script if present, then `npm start` with PORT.
        if repo
            .package_json()
            .is_some_and(|package| package.has_script("build"))
        {
            plan.build = Some(manager.run_script("build"));
        }
        plan.start = Some(manager.run_script("start"));
        plan.env
            .insert(cfg.app.port_env.clone(), PORT_PLACEHOLDER.to_owned());
        plan
    }

    fn migrations(&self, repo: &RepoView) -> Option<MigrationTool> {
        detect_migration_tool(repo)
    }

    fn route_table(&self, _repo: &RepoView) -> Measured<RouteTable> {
        // Express and Hono routes come from an AST walk in M6.
        Measured::not_measured(ROUTES_LATER)
    }

    fn test_command(&self, _repo: &RepoView) -> Option<CommandSpec> {
        None
    }
}

/// The migration tool a repository uses, from the plan's four.
///
/// Detected from dependencies rather than config files: `prisma/schema.prisma` can exist without
/// Prisma being a dependency, and the dependency is what decides whether the command will run.
#[must_use]
pub fn detect_migration_tool(repo: &RepoView) -> Option<MigrationTool> {
    let package = repo.package_json()?;
    let manager = manager_or_npm(repo);
    // Order matters: a repository can carry both Drizzle and Prisma while migrating from one, and
    // the pinned order makes the answer the same on every run.
    if package.depends_on("prisma") || repo.exists("prisma/schema.prisma") {
        return Some(MigrationTool {
            tool: "prisma",
            command: manager.exec("prisma", &["migrate", "deploy"]),
        });
    }
    if package.depends_on("drizzle-kit") || repo.exists("drizzle.config.ts") {
        return Some(MigrationTool {
            tool: "drizzle",
            command: manager.exec("drizzle-kit", &["migrate"]),
        });
    }
    if package.depends_on("typeorm") {
        return Some(MigrationTool {
            tool: "typeorm",
            command: manager.exec("typeorm", &["migration:run"]),
        });
    }
    if package.depends_on("knex") {
        return Some(MigrationTool {
            tool: "knex",
            command: manager.exec("knex", &["migrate:latest"]),
        });
    }
    None
}

/// Every TypeScript adapter, in the order the ladder tries them.
///
/// Next before Vite before a plain server: a Next app also lists `vite` in rare setups (a docs
/// sub-app), while a Vite app never has `next`, and a plain server is the weakest claim of the
/// three. Detection returns the first match at the highest confidence.
#[must_use]
pub fn adapters() -> Vec<Box<dyn StackAdapter>> {
    vec![Box::new(NextJs), Box::new(Vite), Box::new(NodeServer)]
}

/// The adapter that best matches a repository.
///
/// Strong beats weak, and among equals the order in [`adapters`] decides.
#[must_use]
pub fn detect(repo: &RepoView) -> Option<(Box<dyn StackAdapter>, Detection)> {
    let mut best: Option<(Box<dyn StackAdapter>, Detection)> = None;
    for adapter in adapters() {
        let Some(detection) = adapter.detect(repo) else {
            continue;
        };
        let better = match &best {
            None => true,
            Some((_, current)) => detection.confidence > current.confidence,
        };
        if better {
            best = Some((adapter, detection));
        }
    }
    best
}

/// A `PackageJson` parsed from text, for a caller that already has the document.
#[must_use]
pub fn parse_package_json(text: &str) -> Option<PackageJson> {
    PackageJson::parse(text)
}
