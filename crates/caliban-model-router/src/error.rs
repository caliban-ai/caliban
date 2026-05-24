//! Errors emitted by the model router during construction.

/// Errors returned by [`crate::ModelRouter::builder().build()`].
#[derive(thiserror::Error, Debug)]
pub enum RouterError {
    /// The provider map is empty — the router needs at least one provider.
    #[error("router: no providers configured")]
    EmptyProviders,
    /// A route references a provider name that wasn't registered in the
    /// provider map.
    #[error("router: route references unknown provider '{0}'")]
    UnknownProvider(String),
    /// No route matches the configured `default_purpose`.
    #[error("router: no route matches default_purpose={0:?}")]
    DefaultPurposeUnrouted(caliban_provider::RequestPurpose),
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, RouterError>;
