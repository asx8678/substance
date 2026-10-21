//! The permission value that guards repo-controlled execution (M1-T01, §4.1).
//!
//! Some steps run code the repository controls: install, build, start, migrate, seed, its
//! own tests, and tools that load its config (knip, eslint). All of them need
//! `--allow-exec`. The gate is a type, so forgetting it is a compile error rather than a
//! review note: every function that spawns repo-controlled code takes `&ExecPermit`, and
//! the only way to obtain one is [`ExecPermit::grant`], which intake calls
//! (`crates/orchestrator/src/intake.rs`) and nothing else does — `scripts/guards.sh`
//! enforces that.

/// Proof that the caller passed `--allow-exec`.
///
/// Zero-sized: it carries no data, only the fact that intake saw the flag. It is neither
/// `Clone` nor `Copy`, so one permit is one grant.
#[derive(Debug)]
pub struct ExecPermit {
    /// Private, so no other module can build one with a struct literal.
    _private: (),
}

impl ExecPermit {
    /// Grants the permit. Only intake calls this, and only when `--allow-exec` is set.
    ///
    /// Named `grant` rather than `new` on purpose: the name says what the caller is
    /// doing, and it keeps the type off `Default`, which would hand out a grant nobody
    /// asked for.
    #[must_use]
    pub fn grant() -> Self {
        Self { _private: () }
    }
}
