//! Stack and database adapters, lockfile parsers and route tables.
//!
//! M1-T04 lands [`deps`]: one parser per lockfile format, all producing
//! [`substance_core::deps::Dependency`].

pub mod deps;
