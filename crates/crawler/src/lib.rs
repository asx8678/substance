//! The deterministic state-graph crawler that builds the app model (M4-T01).
//!
//! ## What is here, and what is not
//!
//! The modules above are the crawler's decisions: which screen to visit next, what a route template
//! is, how controls are ordered, what a form field is filled with, whether a login worked, whether an
//! action logged out, what the model and the HAR say. They are pure functions of their inputs, which
//! is what makes a crawl reproducible and what makes each rule checkable without a browser.
//!
//! The driving itself -- `goto`, `list_controls`, `act`, `observe_idle` -- needs a driver and an app,
//! so it lives in the orchestrator's explore stage, which owns the boot and the teardown. That split
//! is the same one the recorder uses: the crate that knows what to ask does not also decide how to
//! start a browser.

pub mod cache;
pub mod controls;
pub mod effects;
pub mod forms;
pub mod frontier;
pub mod har;
pub mod model;
pub mod route;

pub use frontier::{Frontier, Pending};
pub use model::{Visited, build};
pub use route::{is_identifier, template_path};
