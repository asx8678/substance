//! Tearing a run's database down from the interrupt handler (M2-T04).
//!
//! The interrupt handler in `substance-runner` cleans up what a cancelled future still owned, and it
//! cannot name [`Adapter`](super::factory::Adapter) -- `substance-adapters` depends on
//! `substance-runner`, not the other way round. [`Cleanup`](substance_runner::Cleanup) in the runner
//! is the trait it calls, and this type implements it for a database.
//!
//! ## Why the cleanup holds names rather than the adapter
//!
//! The adapter is a local value in the runtime stage, dropped when the stage returns. Anything the
//! registrar holds lives until the registrar does, which is longer -- so an `Arc<Adapter>` would keep
//! the adapter alive past its stage and fire its own `Drop` (the one that removes the Postgres slot)
//! long after the run finished. Instead this holds the *names* a teardown needs, taken at registration
//! time, and runs the same statements the adapter's teardown does.
//!
//! The two paths are not duplicates fighting each other: both are idempotent (`DROP ... IF EXISTS`,
//! `pg_drop_replication_slot` only when the slot is listed), so whichever runs first wins and the
//! other does nothing. Postgres's own guard is the third path and is defused by an explicit teardown.
//!
//! ## What it cannot clean up
//!
//! A SQLite run's file and a Supabase run's containers. The file is inside the work copy, which the
//! run directory's removal takes with it; the containers are stopped by
//! [`SupabaseAdapter::teardown`](super::supabase::SupabaseAdapter), which needs a permit and a CLI
//! call the interrupt path cannot make safely. Both facts are recorded rather than hidden.

use std::future::Future;
use std::pin::Pin;

use substance_runner::Cleanup;

/// One database's teardown, for the interrupt handler.
#[derive(Debug, Clone)]
pub struct DatabaseCleanup {
    /// The engine's name, for the line the handler records.
    engine: String,
    /// What the teardown has to reach, when this engine can be torn down from here.
    target: Option<Target>,
}

/// The names a Postgres teardown needs.
///
/// Deliberately not the connection string: the admin URL carries a password and the registrar is a
/// long-lived, freely cloned value that a `Debug` print or a note could reach. A teardown reconnects
/// from `SUBSTANCE_PG_ADMIN_URL`, exactly as the adapter's own `from_env` does, and never holds the
/// credential itself.
#[derive(Debug, Clone)]
struct Target {
    /// The run's database names: the app's, the seeded template, and any snapshots.
    databases: Vec<String>,
    /// The run's replication slot.
    slot: String,
}

impl DatabaseCleanup {
    /// A cleanup for the engine `engine`, with no target.
    ///
    /// Used for the engines this path cannot reach; `cleanup` then reports why rather than pretending
    /// it did something.
    #[must_use]
    pub fn of(engine: &str) -> Self {
        Self {
            engine: engine.to_owned(),
            target: None,
        }
    }

    /// A cleanup that knows the Postgres names to drop.
    #[must_use]
    pub fn postgres(databases: Vec<String>, slot: String) -> Self {
        Self {
            engine: "postgres".to_owned(),
            target: Some(Target { databases, slot }),
        }
    }

    /// The engine this cleanup belongs to.
    #[must_use]
    pub fn engine(&self) -> &str {
        &self.engine
    }
}

impl Cleanup for DatabaseCleanup {
    fn cleanup(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + '_>> {
        Box::pin(async move {
            match (&self.target, self.engine.as_str()) {
                (Some(target), _) => {
                    super::postgres::drop_names(&target.databases, &target.slot).await
                }
                (None, "sqlite") => vec![
                    "sqlite: the database file is inside the work copy, which the run directory \
                     removal takes with it"
                        .to_owned(),
                ],
                (None, other) => vec![format!(
                    "{other}: this path cannot tear it down; its adapter's teardown is the one that \
                     does, and substance doctor lists anything it left"
                )],
            }
        })
    }
}
