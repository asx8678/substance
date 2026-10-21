//! The optional LLM layer, behind the `agent` feature (off by default, ADR-0006).
//!
//! The crate defines the provider boundary only; v1 ships no implementation.

#[cfg(feature = "agent")]
mod provider;

#[cfg(feature = "agent")]
pub use provider::{AgentArtifact, AgentError, AgentJob, AgentProvider, UntrustedText};
