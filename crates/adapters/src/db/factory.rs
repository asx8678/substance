//! Picking the database adapter from the config (M2-T04).
//!
//! ## Why an enum and not a `Box<dyn DbAdapter>`
//!
//! The trait is object-safe and the runtime stage mostly wants exactly that, but two of Supabase's
//! methods cannot be reached through a trait object: [`crate::db::supabase::SupabaseAdapter::start`]
//! and [`crate::db::supabase::SupabaseAdapter::reset_stack`] take an [`ExecPermit`]
//! and are inherent methods precisely so that running the repository's migrations requires the
//! caller to have proven `--allow-exec`. A `Box<dyn DbAdapter>` would hide them, and the obvious
//! "fix" -- putting them on the trait -- would let any caller with a trait object run repo code with
//! no permit at all.
//!
//! [`Adapter`] therefore names the engine, and the caller matches on it for the two permit-gated
//! operations while using the trait for everything else. The match is four arms and each one says
//! which engine it is; that is cheaper than a trait method that erases the proof.

use std::path::{Path, PathBuf};

use substance_core::config::{DbConfig, DbEngine, ProjectConfig};
use substance_core::ids::RunId;
use substance_core::tools::ToolSpec;

use super::postgres::PostgresAdapter;
use super::sqlite::SqliteAdapter;
use super::supabase::{self, SupabaseAdapter};
use super::{DbAdapter, DbError};

/// The adapter a run uses, named by engine.
///
/// `Postgres` and `Sqlite` wrap the one adapter each; `Supabase` keeps the concrete type because it
/// has methods the trait cannot carry. `Box<dyn DbAdapter>` is available through [`Self::as_trait`]
/// for the stages that need nothing more.
///
/// The Postgres adapter is boxed because it is four times the size of the others (it holds a
/// `tokio_postgres::Config`, a mutex and a drop guard), and an enum is as large as its largest
/// variant: without the box every `Adapter` on the stack, and every one moved into an await's
/// state, would carry that footprint whichever engine the run actually uses.
#[derive(Debug)]
pub enum Adapter {
    /// A Postgres server, one database pair per run.
    Postgres(Box<PostgresAdapter>),
    /// A SQLite file inside the work copy.
    Sqlite(SqliteAdapter),
    /// A local Supabase stack.
    Supabase(SupabaseAdapter),
}

/// Why an adapter could not be built.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// The engine cannot be used with this config.
    #[error("db: {0}")]
    Config(String),
    /// The adapter refused its settings.
    #[error(transparent)]
    Adapter(#[from] DbError),
}

impl Adapter {
    /// The underlying trait object, for a stage that needs no permit-gated method.
    #[must_use]
    pub fn as_trait(&self) -> &dyn DbAdapter {
        match self {
            Self::Postgres(adapter) => adapter.as_ref(),
            Self::Sqlite(adapter) => adapter,
            Self::Supabase(adapter) => adapter,
        }
    }

    /// The engine's name, as the report spells it.
    #[must_use]
    pub fn engine(&self) -> &'static str {
        self.as_trait().engine()
    }

    /// The Supabase adapter, when this run uses one.
    ///
    /// The only way to reach `start` and `reset_stack`, and so the only way to start a stack. `None`
    /// for the other two engines, which need no permit to provision.
    #[must_use]
    pub fn supabase(&self) -> Option<&SupabaseAdapter> {
        match self {
            Self::Supabase(adapter) => Some(adapter),
            _ => None,
        }
    }

    /// Picks the engine the config names and builds its adapter.
    ///
    /// `work` is the work copy: SQLite's path and Supabase's `supabase/` are inside it. `run_dir` is
    /// where SQLite snapshots go. `tools` is the pinned tool table, needed only by Supabase.
    ///
    /// # Errors
    ///
    /// Returns [`FactoryError::Config`] when the engine needs a setting the config does not have --
    /// a SQLite run with no `sqlite_path`, a Postgres run with no admin URL in the environment --
    /// and [`FactoryError::Adapter`] when an adapter rejects what it was given.
    pub fn build(
        cfg: &ProjectConfig,
        run_id: &RunId,
        work: &Path,
        run_dir: &Path,
        tools: &[(String, ToolSpec)],
    ) -> Result<Self, FactoryError> {
        match cfg.db.engine {
            DbEngine::Postgres => Ok(Self::Postgres(Box::new(PostgresAdapter::from_env(
                run_id,
                cfg.db.url_env.clone(),
            )?))),
            DbEngine::Sqlite => {
                let relative = cfg.db.sqlite_path.as_ref().ok_or_else(|| {
                    FactoryError::Config(
                        "engine = \"sqlite\" needs sqlite_path, or there is no file to open"
                            .to_owned(),
                    )
                })?;
                let path = absolute(work, relative);
                Ok(Self::Sqlite(SqliteAdapter::new(
                    path,
                    run_dir.join("snapshots"),
                    cfg.db.url_env.clone(),
                )))
            }
            DbEngine::Supabase => {
                let spec = tools
                    .iter()
                    .find(|(name, _)| name == supabase::TOOL)
                    .map(|(_, spec)| spec.clone())
                    .ok_or_else(|| {
                        FactoryError::Config(format!(
                            "engine = \"supabase\" needs {} in tools.toml",
                            supabase::TOOL
                        ))
                    })?;
                Ok(Self::Supabase(SupabaseAdapter::new(
                    work.to_path_buf(),
                    spec,
                    run_id,
                    cfg.db.url_env.clone(),
                )))
            }
        }
    }
}

/// `relative` resolved against `root`, unless it is already absolute.
///
/// A path in the config is relative to the repository root, which is what a reader of
/// `substance.toml` expects; an absolute one is left alone so a caller can point at a file outside
/// the tree when it needs to.
#[must_use]
pub fn absolute(root: &Path, relative: &str) -> PathBuf {
    let path = Path::new(relative);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// Whether the config names a database at all.
///
/// A default `ProjectConfig` says Postgres, so this is not "is `[db]` present" but "would an adapter
/// have anything to connect to": a Postgres run with no `SUBSTANCE_PG_ADMIN_URL` has no database, and
/// the runtime stage should record that rather than try.
pub fn configured(cfg: &ProjectConfig) -> Result<(), String> {
    match cfg.db.engine {
        DbEngine::Postgres => {
            if std::env::var(super::postgres::ADMIN_URL_ENV).is_err() {
                return Err(format!(
                    "no database: {} is not set, so there is no server to create one on",
                    super::postgres::ADMIN_URL_ENV
                ));
            }
            Ok(())
        }
        DbEngine::Sqlite => match &cfg.db.sqlite_path {
            Some(path) if !path.trim().is_empty() => Ok(()),
            _ => Err("no database: engine = \"sqlite\" needs sqlite_path".to_owned()),
        },
        DbEngine::Supabase => Ok(()),
    }
}

/// The engine the config names, for a note that has no adapter.
#[must_use]
pub fn engine_name(cfg: &DbConfig) -> &'static str {
    match cfg.engine {
        DbEngine::Postgres => "postgres",
        DbEngine::Sqlite => "sqlite",
        DbEngine::Supabase => "supabase",
    }
}
