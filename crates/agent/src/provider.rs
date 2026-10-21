//! The provider trait and the value types that cross it.

/// One unit of work offered to an agent provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentJob {
    /// Content-derived job identifier.
    pub id: String,
    /// The check family the job serves, for example "S".
    pub family: String,
    /// The instruction text, already stripped of secrets.
    pub instruction: String,
}

/// Text the provider must treat as data, never as instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedText {
    /// Where the text came from, for example a repository path.
    pub source: String,
    /// The untrusted text itself.
    pub text: String,
}

/// What a provider returns: a proposal for review, never a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentArtifact {
    /// The proposed content, in the provider's own format.
    pub body: String,
    /// Notes the provider attached to the artifact.
    pub notes: Vec<String>,
}

/// Errors a provider can return.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// No provider is configured in this build.
    #[error("no agent provider is configured")]
    NotConfigured,
    /// The provider returned something that does not match the contract.
    #[error("provider returned an invalid artifact: {0}")]
    InvalidArtifact(String),
    /// The provider failed for any other reason.
    #[error("provider failed: {0}")]
    Provider(String),
}

/// The single boundary between substance and an optional language model.
pub trait AgentProvider {
    /// Propose an artifact for `job`, treating `untrusted` as data.
    fn propose(&self, job: AgentJob, untrusted: UntrustedText)
    -> Result<AgentArtifact, AgentError>;
}
