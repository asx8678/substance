//! The project config, read from `substance.toml` (M0-T05).
//!
//! Every section and every key has a default, so a repository without a config file
//! still yields a complete [`ProjectConfig`]. Unknown keys are rejected, so a typo in
//! `substance.toml` fails loudly instead of being ignored.
//!
//! Parsing lives here; finding and reading the file lives in the orchestrator, because
//! this crate does no IO outside `clock.rs` (ADR-0009).

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::check::CheckId;
use crate::hash::{CanonicalJsonError, canonical_json};
use crate::ids::BlobRef;
use crate::model::NormalizePattern;

/// The whole project config, one field per `[section]` of `substance.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct ProjectConfig {
    /// How the app is installed, built, migrated, seeded and started.
    pub app: AppConfig,
    /// Which database the app uses and how it is reached.
    pub db: DbConfig,
    /// The test accounts, at least two per role (ADR-0019). Order is kept as written.
    pub users: Vec<UserConfig>,
    /// The request that creates an account when `create = "api"`.
    pub signup: SignupConfig,
    /// How to log in: a form or an API call.
    pub login: LoginConfig,
    /// The environment the app gets for sending mail.
    pub mail: MailConfig,
    /// Extra selectors that claim success or an error.
    pub signals: SignalsConfig,
    /// Project-specific normalization rules, applied in order.
    pub normalize: NormalizeConfig,
    /// Hosts the egress proxy lets through.
    pub egress: EgressConfig,
    /// Time and count limits.
    pub budgets: Budgets,
    /// Checks the project switched off.
    pub checks: ChecksConfig,
}

impl ProjectConfig {
    /// Parses a config document. The serde defaults of every section fill in absent keys.
    ///
    /// This checks the shape only. [`Self::validate`] lists inconsistencies such as two
    /// accounts with one handle; the caller decides whether to stop.
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    /// The content hash of this config, as `sha256:<hex>`.
    ///
    /// The manifest records it, and the app model cache is keyed on it. It hashes the
    /// canonical JSON of the config, so key order in the file never matters.
    pub fn config_hash(&self) -> Result<String, CanonicalJsonError> {
        let bytes = canonical_json(self)?;
        Ok(BlobRef::from_bytes(&bytes).as_str().to_owned())
    }

    /// Lists every inconsistency in this config, sorted.
    ///
    /// The list is advisory: a config that is merely incomplete still yields a useful
    /// run, so the CLI prints these and keeps going. Only unknown keys and malformed
    /// TOML are hard errors, and parsing catches them.
    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        self.app.validate(&mut problems);
        self.db.validate(&mut problems);
        self.users_validate(&mut problems);
        self.signup.validate(&mut problems);
        self.login.validate(&mut problems);
        self.signals.validate(&mut problems);
        self.budgets.validate(&mut problems);
        self.egress.validate(&mut problems);
        problems.sort();
        problems
    }

    /// Checks the accounts: at least two per role, unique handles and emails, and a
    /// signup request whenever an account is created through the API.
    fn users_validate(&self, problems: &mut Vec<String>) {
        let mut per_role: BTreeMap<&str, u32> = BTreeMap::new();
        let mut handles: BTreeMap<&str, u32> = BTreeMap::new();
        let mut emails: BTreeMap<&str, u32> = BTreeMap::new();
        let mut needs_signup = false;
        for (index, user) in self.users.iter().enumerate() {
            user.validate(index, problems);
            *per_role.entry(user.role.as_str()).or_default() += 1;
            *handles.entry(user.handle.as_str()).or_default() += 1;
            *emails.entry(user.email.as_str()).or_default() += 1;
            needs_signup = needs_signup || user.create == UserCreate::Api;
        }
        for (role, count) in per_role {
            if count < 2 {
                problems.push(format!(
                    "users: role {role:?} has {count} account(s), but ADR-0019 requires two per role"
                ));
            }
        }
        for (handle, count) in handles {
            if count > 1 {
                problems.push(format!(
                    "users: handle {handle:?} is used by {count} accounts, but handles identify accounts"
                ));
            }
        }
        for (email, count) in emails {
            if count > 1 {
                problems.push(format!(
                    "users: email {email:?} is used by {count} accounts, but a login needs a unique one"
                ));
            }
        }
        if needs_signup && self.signup.api.is_none() {
            problems.push(
                "signup: an account with create = \"api\" needs the [signup] api request"
                    .to_owned(),
            );
        }
    }
}

/// Why a config cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The text is not TOML, or not the TOML this schema describes.
    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),
}

/// How the app is installed, built, migrated, seeded and started (boot ladder rung 2, ADR-0017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct AppConfig {
    /// The install command, for example `npm ci`.
    pub install: Option<String>,
    /// The build command.
    pub build: Option<String>,
    /// The command that starts the long-running server; substance supplies the port.
    pub start: Option<String>,
    /// The migration command.
    pub migrate: Option<String>,
    /// The seed command.
    pub seed: Option<String>,
    /// The variable the server reads its port from.
    pub port_env: String,
    /// The path that answers once the server is up.
    pub ready_path: String,
    /// The path O06 probes with the database down; no path means O06 is not measured.
    pub health_path: Option<String>,
    /// Extra environment for every command, sorted by name.
    pub env: BTreeMap<String, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            install: None,
            build: None,
            start: None,
            migrate: None,
            seed: None,
            port_env: "PORT".to_owned(),
            ready_path: "/".to_owned(),
            health_path: None,
            env: BTreeMap::new(),
        }
    }
}

impl AppConfig {
    /// Checks that the paths are paths and that a set command is not blank.
    fn validate(&self, problems: &mut Vec<String>) {
        if self.port_env.trim().is_empty() {
            problems.push("app: port_env must not be empty".to_owned());
        }
        if !self.ready_path.starts_with('/') {
            problems.push(format!(
                "app: ready_path {:?} must start with a slash",
                self.ready_path
            ));
        }
        match &self.health_path {
            Some(path) if !path.starts_with('/') => {
                problems.push(format!("app: health_path {path:?} must start with a slash"))
            }
            _ => {}
        }
        let commands = [
            ("install", &self.install),
            ("build", &self.build),
            ("start", &self.start),
            ("migrate", &self.migrate),
            ("seed", &self.seed),
        ];
        for (key, value) in commands {
            match value {
                Some(command) if command.trim().is_empty() => {
                    problems.push(format!("app: {key} is set but empty"));
                }
                _ => {}
            }
        }
    }
}

/// Which database the app uses (ADR-0018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DbEngine {
    /// Postgres, with logical decoding for the database channel.
    #[default]
    Postgres,
    /// A SQLite file.
    Sqlite,
    /// A local Supabase stack.
    Supabase,
}

/// The database section of the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct DbConfig {
    /// The engine.
    pub engine: DbEngine,
    /// The variable the app reads its connection URL from.
    pub url_env: String,
    /// The SQLite file, relative to the repository root.
    pub sqlite_path: Option<String>,
    /// Tables B25 excludes from the deletion check, because the law keeps their rows.
    pub retained_tables: BTreeSet<String>,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            engine: DbEngine::Postgres,
            url_env: "DATABASE_URL".to_owned(),
            sqlite_path: None,
            retained_tables: BTreeSet::new(),
        }
    }
}

impl DbConfig {
    /// Checks that the connection settings can be used.
    fn validate(&self, problems: &mut Vec<String>) {
        if self.url_env.trim().is_empty() {
            problems.push("db: url_env must not be empty".to_owned());
        }
        if self.engine == DbEngine::Sqlite && self.sqlite_path.is_none() {
            problems.push("db: sqlite_path is required when engine = \"sqlite\"".to_owned());
        }
        match &self.sqlite_path {
            Some(path) if path.trim().is_empty() => {
                problems.push("db: sqlite_path is set but empty".to_owned());
            }
            _ => {}
        }
    }
}

/// How a `[[users]]` account is created (M2-T04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum UserCreate {
    /// Through the `[signup]` request template.
    #[default]
    Api,
    /// With SQL against the run database.
    Sql,
    /// Through the Supabase GoTrue admin API.
    Supabase,
    /// Through the app's own signup form.
    Ui,
    /// It already exists after seeding.
    Existing,
}

/// One test account. Secrets are named, never stored.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct UserConfig {
    /// The role this account proves, for example `user` or `admin`.
    pub role: String,
    /// The handle that identifies the account in reports.
    pub handle: String,
    /// The address to register and to read mail at.
    pub email: String,
    /// The name of the environment variable that holds the password.
    pub password_env: String,
    /// How the account is created.
    pub create: UserCreate,
}

impl UserConfig {
    /// Checks that every identifying field is set.
    fn validate(&self, index: usize, problems: &mut Vec<String>) {
        let fields = [
            ("role", &self.role),
            ("handle", &self.handle),
            ("email", &self.email),
            ("password_env", &self.password_env),
        ];
        for (key, value) in fields {
            if value.trim().is_empty() {
                problems.push(format!("users[{index}]: {key} must not be empty"));
            }
        }
    }
}

/// An HTTP request template the runner sends on the app's behalf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct ApiRequest {
    /// The method.
    pub method: String,
    /// The path, relative to the app's origin.
    pub path: String,
    /// The body template, with `{email}`, `{password}` and `{handle}` placeholders.
    pub body: Option<String>,
}

impl Default for ApiRequest {
    fn default() -> Self {
        Self {
            method: "POST".to_owned(),
            path: String::new(),
            body: None,
        }
    }
}

impl ApiRequest {
    /// Checks that the request has a method and an absolute path.
    fn validate(&self, section: &str, problems: &mut Vec<String>) {
        if self.method.trim().is_empty() {
            problems.push(format!("{section}: method must not be empty"));
        }
        if !self.path.starts_with('/') {
            problems.push(format!(
                "{section}: path {:?} must start with a slash",
                self.path
            ));
        }
    }
}

/// The account creation request.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct SignupConfig {
    /// The request that creates an account.
    pub api: Option<ApiRequest>,
}

impl SignupConfig {
    /// Checks the request template, when there is one.
    fn validate(&self, problems: &mut Vec<String>) {
        if let Some(api) = &self.api {
            api.validate("signup", problems);
        }
    }
}

/// A login form the crawl can fill.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct LoginUi {
    /// The path of the login screen.
    pub path: String,
    /// The selector of the identifier field.
    pub identifier: String,
    /// The selector of the password field.
    pub password: String,
    /// The selector of the submit control.
    pub submit: String,
}

impl LoginUi {
    /// Checks that the form has a path and three selectors.
    fn validate(&self, problems: &mut Vec<String>) {
        if !self.path.starts_with('/') {
            problems.push(format!(
                "login: ui path {:?} must start with a slash",
                self.path
            ));
        }
        let selectors = [
            ("identifier", &self.identifier),
            ("password", &self.password),
            ("submit", &self.submit),
        ];
        for (key, value) in selectors {
            if value.trim().is_empty() {
                problems.push(format!("login: ui {key} must not be empty"));
            }
        }
    }
}

/// How to log in (M4-T01). Set `ui` or `api`; set neither to let the crawl detect a
/// single password form, and set both to log in through the API and check the form.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct LoginConfig {
    /// The form to fill.
    pub ui: Option<LoginUi>,
    /// The request to send instead of filling the form.
    pub api: Option<ApiRequest>,
}

impl LoginConfig {
    /// Checks whichever form is configured.
    fn validate(&self, problems: &mut Vec<String>) {
        match (&self.ui, &self.api) {
            (Some(ui), Some(_)) => {
                problems.push("login: set either ui or api, not both".to_owned());
                ui.validate(problems);
            }
            (Some(ui), None) => ui.validate(problems),
            (None, Some(api)) => api.validate("login", problems),
            (None, None) => {}
        }
    }
}

/// The app's mail environment. Mailpit supplies the values (M3-T04).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct MailConfig {
    /// The environment the app gets, sorted by name, with `{smtp_host}` and `{smtp_port}`
    /// filled in by the runner.
    pub env: BTreeMap<String, String>,
}

/// Extra selectors that stand in for agent labels (M12).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct SignalsConfig {
    /// Selectors that claim success, in matching order.
    pub success: Vec<String>,
    /// Selectors that claim an error, in matching order.
    pub error: Vec<String>,
}

impl SignalsConfig {
    /// Checks that no selector is blank.
    fn validate(&self, problems: &mut Vec<String>) {
        let groups = [("success", &self.success), ("error", &self.error)];
        for (key, selectors) in groups {
            for selector in selectors {
                if selector.trim().is_empty() {
                    problems.push(format!("signals: {key} has an empty selector"));
                }
            }
        }
    }
}

/// Project normalization rules, applied before hashing and diffing.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct NormalizeConfig {
    /// The rules, in the order they are applied. The built-in rules come first.
    pub patterns: Vec<NormalizePattern>,
}

/// Hosts the egress proxy lets through (M3-T04).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct EgressConfig {
    /// Allowed hosts. The proxy denies everything else.
    pub allow: BTreeSet<String>,
}

impl EgressConfig {
    /// Checks that no host is blank.
    fn validate(&self, problems: &mut Vec<String>) {
        for host in &self.allow {
            if host.trim().is_empty() {
                problems.push("egress: allow has an empty host".to_owned());
            }
        }
    }
}

/// Time and count limits, in seconds, milliseconds or counts (ADR-0002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct Budgets {
    /// Triage time limit.
    pub triage_s: u32,
    /// Whole-run time limit.
    pub run_s: u32,
    /// How long the network must stay quiet before an action counts as settled.
    pub settle_quiet_ms: u32,
    /// The cap on settling one action.
    pub settle_timeout_ms: u32,
    /// How long the idle baseline is sampled.
    pub idle_baseline_ms: u32,
    /// How long a server gets to exit before it is killed.
    pub shutdown_grace_s: u32,
    /// Screens the crawl may visit.
    pub crawl_max_screens: u32,
    /// Actions the crawl may run.
    pub crawl_max_actions: u32,
    /// Crawl depth.
    pub crawl_max_depth: u32,
    /// Files the blame check may inspect.
    pub blame_max_files: u32,
    /// Requests the forge check may send.
    pub forge_max_requests: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            triage_s: 120,
            run_s: 1800,
            settle_quiet_ms: 500,
            settle_timeout_ms: 10_000,
            idle_baseline_ms: 5000,
            shutdown_grace_s: 30,
            crawl_max_screens: 200,
            crawl_max_actions: 2000,
            crawl_max_depth: 6,
            blame_max_files: 5000,
            forge_max_requests: 3000,
        }
    }
}

impl Budgets {
    /// Checks that the limits can be met: nonzero where zero would stop the run, and a
    /// quiet period no longer than its cap.
    fn validate(&self, problems: &mut Vec<String>) {
        let nonzero = [
            ("triage_s", self.triage_s),
            ("run_s", self.run_s),
            ("settle_timeout_ms", self.settle_timeout_ms),
            ("crawl_max_screens", self.crawl_max_screens),
            ("crawl_max_actions", self.crawl_max_actions),
            ("crawl_max_depth", self.crawl_max_depth),
        ];
        for (key, value) in nonzero {
            if value == 0 {
                problems.push(format!("budgets: {key} must be greater than zero"));
            }
        }
        if self.settle_quiet_ms > self.settle_timeout_ms {
            problems.push("budgets: settle_quiet_ms must not exceed settle_timeout_ms".to_owned());
        }
    }
}

/// Checks the project switched off. They are reported as `not_measured` (M0-T08).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct ChecksConfig {
    /// The checks to skip, in ID order and without duplicates.
    pub disable: BTreeSet<CheckId>,
}
