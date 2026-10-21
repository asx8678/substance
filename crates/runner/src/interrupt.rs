//! What a Ctrl-C does to a run in progress (M2-T03).
//!
//! A run owns things that outlive a dropped future: an app in its own process group, and on
//! Postgres a logical replication slot that pins WAL and two databases. The default Ctrl-C
//! behaviour kills the process where it stands, which runs no `Drop` for the app's guard and no
//! cleanup for the database, so the server keeps the slot and the app keeps its port.
//!
//! Three things are needed to do better, and each is here for its own reason:
//!
//! - **A copy of what to stop, made before the first resource exists.** `Drop` needs `&mut self`
//!   on an open app, which a cleanup cannot get once the future holding it is cancelled. So a
//!   [`Registrar`] keeps its own handles: a process group id, and a [`Cleanup`] for anything that
//!   cleans itself up asynchronously.
//! - **A second listener.** `tokio::signal::ctrl_c()` keeps one handler per process: calling it
//!   again replaces the previous handler, so a caller that awaits its own `ctrl_c()` and then runs
//!   a cleanup cancels the other listener in silence. The installer therefore owns the only
//!   listener, and every consumer waits on a broadcast it sends.
//! - **Exit code 3, not 130.** The plan's exit codes are 0, 1, 2 and 3; an interrupt that stopped a
//!   run cleanly is "something inside substance stopped this", which is 3. The code itself lives in
//!   the CLI, which owns every exit code; this crate only says the run was interrupted.
//!
//! ## What this cannot cover
//!
//! `SIGKILL` (and a process that dies before any handler is installed, or a power cut) runs no
//! code at all. A leftover `substance_%` database or slot is therefore possible, which is why
//! `substance doctor` (M2-T06) sweeps them by name rather than trusting that teardown always ran.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, broadcast};

/// Something the interrupt handler can clean up asynchronously.
///
/// A trait rather than a direct dependency on the database adapters, because `substance-adapters`
/// already depends on this crate: the arrow has to point this way. The boxed future is the price of
/// a trait object without `async_trait` here -- the runner does not need that crate's macro for one
/// method, and a plain `async fn` in a trait is not object-safe.
pub trait Cleanup: Send + Sync + std::fmt::Debug {
    /// Removes what this holds, returning what it removed. Best effort; never fails a run.
    fn cleanup(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + '_>>;
}

/// One registered resource, described well enough to clean it up from the listener's task.
#[derive(Debug, Clone)]
pub enum Resource {
    /// An app in a process group: the group leader's pid, which is also the group id.
    App {
        /// The program, for the line the cleanup prints.
        program: String,
        /// The process group to signal.
        group: i32,
    },
    /// A resource with its own asynchronous teardown: a provisioned database, typically.
    Owned(Arc<dyn Cleanup>),
}

/// Holds what a Ctrl-C has to reach, so the handler can act on resources a cancelled future still
/// owns.
///
/// Cloned freely: every clone sees the same registrations, because they share one list. That is
/// what lets a stage register from inside a future the signal handler will cancel.
#[derive(Debug, Clone, Default)]
pub struct Registrar {
    /// Registered resources, in the order they were registered.
    resources: Arc<Mutex<Vec<Resource>>>,
}

impl Registrar {
    /// An empty registrar.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an app's process group, so an interrupt can signal it.
    pub fn register_app(&self, program: &str, group: i32) {
        self.push(Resource::App {
            program: program.to_owned(),
            group,
        });
    }

    /// Records something with its own asynchronous teardown.
    pub fn register_cleanup(&self, cleanup: Arc<dyn Cleanup>) {
        self.push(Resource::Owned(cleanup));
    }

    /// Forgets an app a normal path already stopped.
    ///
    /// Called after a successful stop, so an interrupt that arrives later does not signal the group
    /// again -- and, more importantly, does not signal a pid the OS has since given to something
    /// else. A stale group id is the one piece of this module that could act on the wrong process.
    pub fn forget_app(&self, group: i32) {
        if let Ok(mut resources) = self.resources.lock() {
            resources.retain(|held| !matches!(held, Resource::App { group: held_group, .. } if *held_group == group));
        }
    }

    /// Everything still registered, in registration order.
    #[must_use]
    pub fn resources(&self) -> Vec<Resource> {
        self.resources
            .lock()
            .map(|resources| resources.clone())
            .unwrap_or_default()
    }

    /// Appends `resource` unless the same one is already there.
    ///
    /// Deduplicated because a resource registered twice would be cleaned up twice and reported
    /// twice; `Resource` has no `PartialEq` for `Owned` (a trait object cannot be compared), so the
    /// check is by pointer identity for adapters and by group for apps.
    fn push(&self, resource: Resource) {
        if let Ok(mut resources) = self.resources.lock() {
            let duplicate = match &resource {
                Resource::App { group, .. } => resources
                    .iter()
                    .any(|held| matches!(held, Resource::App { group: held_group, .. } if held_group == group)),
                Resource::Owned(cleanup) => resources.iter().any(|held| {
                    matches!(held, Resource::Owned(held_cleanup) if Arc::ptr_eq(held_cleanup, cleanup))
                }),
            };
            if !duplicate {
                resources.push(resource);
            }
        }
    }
}

/// What the listener has cleaned up, and whether it is done.
///
/// Shared between the listener task and the run: the task does the cleaning, and the run waits here
/// before it returns. Waiting is what makes a Ctrl-C exit orderly -- returning from `run` while a
/// cleanup task is still connecting to a server would end the runtime and cancel it, which is
/// exactly the leak this module exists to prevent.
#[derive(Debug, Default)]
struct Completion {
    /// Set once every registered resource has been acted on.
    done: Notify,
    /// What was removed. `None` until the cleanup finished.
    removed: Mutex<Option<Vec<String>>>,
}

/// A running interrupt listener.
///
/// Dropping it stops the listener, which is what a finished run wants: a process that stays alive
/// after `run` returned (a test harness, the CLI while it prints its summary) must not act on a
/// Ctrl-C that belongs to whatever runs next.
#[derive(Debug)]
pub struct InterruptGuard {
    /// Notified to tell the listener task to finish.
    stop: Arc<Notify>,
    /// What the cleanup did, so a caller can wait for it and report it.
    completion: Arc<Completion>,
}

impl InterruptGuard {
    /// A guard for a run with no listener, which is what a platform without signals gets.
    ///
    /// Its `wait_cleaned` returns immediately with an empty list, so the run's interrupt path does
    /// not have to know whether a listener exists.
    #[must_use]
    pub fn none() -> Self {
        let completion = Arc::new(Completion::default());
        // Already complete: nothing was registered, so nothing remains to clean.
        if let Ok(mut removed) = completion.removed.lock() {
            *removed = Some(Vec::new());
        }
        Self {
            stop: Arc::new(Notify::new()),
            completion,
        }
    }

    /// Stops the listener.
    pub fn stop(&self) {
        self.stop.notify_waiters();
    }

    /// Waits for the cleanup to finish, up to `within`, and returns what it removed.
    ///
    /// Bounded because the caller is on its way out: a server that accepts the connection and then
    /// stops answering must not hold the process open. An empty list means either that nothing was
    /// registered or that the cleanup had not finished -- the run's note says the run was
    /// interrupted either way, and `substance doctor` is what reconciles the remainder.
    pub async fn wait_cleaned(&self, within: std::time::Duration) -> Vec<String> {
        let _ = tokio::time::timeout(within, async {
            // A cleanup that already finished never notifies again, so the state is checked first:
            // `notified()` alone would wait forever on a completed cleanup.
            loop {
                if let Ok(removed) = self.completion.removed.lock()
                    && let Some(removed) = removed.as_ref()
                {
                    return removed.clone();
                }
                self.completion.done.notified().await;
            }
        })
        .await;
        self.completion
            .removed
            .lock()
            .ok()
            .and_then(|removed| removed.clone())
            .unwrap_or_default()
    }
}

/// Installs the process's Ctrl-C handler.
///
/// Returns the guard that stops the listener and a receiver the run waits on. Two consumers, doing
/// two different jobs:
///
/// - The listener task itself cleans up: it signals every registered app group and awaits every
///   registered [`Cleanup`]. This is what makes the cleanup happen even when the run's own future
///   has been cancelled at an await point, which is the common case for Ctrl-C.
/// - The receiver tells the run to stop waiting and return 3, so the caller's normal error path and
///   report writing still happen rather than being skipped by an abrupt exit.
///
/// # Errors
///
/// Returns the signal API's error when no handler can be installed, which happens on a platform
/// without one. A run that cannot be interrupted cleanly is still a run, so a caller reports it and
/// carries on rather than failing.
pub fn install(registrar: Registrar) -> std::io::Result<(InterruptGuard, broadcast::Receiver<()>)> {
    // One signal: Ctrl-C. SIGTERM would be the other candidate, and the CLI can add it later by
    // installing a second listener here rather than by every stage learning about signals.
    let (sender, receiver) = broadcast::channel::<()>(4);
    let stop = Arc::new(Notify::new());
    let completion = Arc::new(Completion::default());
    let guard = InterruptGuard {
        stop: Arc::clone(&stop),
        completion: Arc::clone(&completion),
    };
    let wake = sender;
    tokio::spawn(async move {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if result.is_err() {
                    return;
                }
                // Told first, so the run can stop writing and return while the cleanup still has
                // something to clean. The cleanup below runs in this task, not in the run's.
                let _ = wake.send(());
                let mut removed = Vec::new();
                for resource in registrar.resources() {
                    match resource {
                        // A no-op when the app's own guard already killed the group, which is why
                        // killing by group id is safe to do twice.
                        Resource::App { group, program } => {
                            if kill_group_now(group) {
                                removed.push(format!("process group {group} ({program})"));
                            }
                        }
                        // Awaited here rather than left to `Drop`: a cancelled future's fields are
                        // dropped by whoever cancels it, and on a Postgres adapter that means the
                        // guard's bounded thread instead of this unbounded await. Doing it here is
                        // the orderly path; the guard is the fallback when nothing runs this.
                        Resource::Owned(cleanup) => {
                            removed.extend(cleanup.cleanup().await);
                        }
                    }
                }
                removed.sort();
                if let Ok(mut slot) = completion.removed.lock() {
                    *slot = Some(removed);
                }
                // Notified after the list is stored, so a waiter that wakes sees the result.
                completion.done.notify_waiters();
            }
            () = stop.notified() => {}
        }
    });
    Ok((guard, receiver))
}

/// Kills a process group if it still exists, using the same path as a normal stop.
fn kill_group_now(group: i32) -> bool {
    crate::process::kill_group_now(group)
}
