//! Errors emitted by the model router during construction and dispatch.

use caliban_provider::Error as ProviderError;

/// Errors returned by [`crate::ModelRouter::builder().build()`] and dispatch.
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
    /// A route's `fallback = [...]` list references an unknown route id.
    #[error("router: route '{from}' references unknown fallback id '{missing}'")]
    UnknownFallbackId {
        /// The route declaring the fallback list.
        from: String,
        /// The id that wasn't found in the route set.
        missing: String,
    },
    /// Resolution produced an empty candidate list.
    #[error("router: no candidate route for purpose={purpose:?} (capability needs: {needs})")]
    NoCandidate {
        /// The purpose that was requested.
        purpose: caliban_provider::RequestPurpose,
        /// Human-readable summary of the capability needs that filtered everything out.
        needs: String,
    },
    /// All candidate routes in the chain returned fatal errors.
    #[error("router: fallback exhausted (tried {tried:?}); last error: {last_error}")]
    FallbackExhausted {
        /// Ids of the routes that were attempted (in order).
        tried: Vec<String>,
        /// The error returned by the last route in the chain.
        last_error: String,
    },
}

impl RouterError {
    /// Convert into a [`ProviderError`] so the router can satisfy the
    /// `Provider` trait contract on dispatch paths.
    pub(crate) fn into_provider_error(self) -> ProviderError {
        match &self {
            RouterError::NoCandidate { .. }
            | RouterError::UnknownFallbackId { .. }
            | RouterError::EmptyProviders
            | RouterError::UnknownProvider(_)
            | RouterError::DefaultPurposeUnrouted(_) => {
                ProviderError::InvalidRequest(self.to_string())
            }
            RouterError::FallbackExhausted { .. } => {
                ProviderError::ModelUnavailable(self.to_string())
            }
        }
    }
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, RouterError>;
