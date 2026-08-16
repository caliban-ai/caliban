//! Shared authentication gate for the drive surfaces (ADR 0055, building on the
//! ADR 0051 bearer scheme).
//!
//! One policy, applied uniformly by every surface (MCP-server, ACP, HTTP serve):
//!
//! - **Loopback is open.** A connection from the local host is trusted, exactly
//!   as the caliband Unix socket is — loopback / filesystem is the boundary.
//! - **Remote requires a bearer token.** A non-loopback peer must present a
//!   bearer token matching the one configured out of band (env / secret),
//!   compared in constant time. A surface bound to a non-loopback address with
//!   no token configured must not serve — fail closed (ADR 0051 / #400).
//!
//! The token lives in a [`secrecy::SecretString`], so it is redacted in `Debug`
//! and never logged. Transport encryption (TLS) for remote binds is the
//! adapter's / deployment's concern and is out of scope for this gate; the
//! MCP-server surface additionally layers the ADR 0023 OAuth flow on top.

use std::net::IpAddr;
use std::net::SocketAddr;

use secrecy::{ExposeSecret as _, SecretString};

/// Environment variable the drive surfaces read the bearer token from.
pub(crate) const TOKEN_ENV: &str = "CALIBAN_DRIVE_TOKEN";

/// Classification of a connecting peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Peer {
    /// Connected over loopback (or a local Unix socket) — trusted.
    Loopback,
    /// Connected from a non-loopback address — must authenticate.
    Remote,
}

impl Peer {
    /// Classify a peer by its remote IP.
    pub(crate) fn from_ip(ip: IpAddr) -> Self {
        if ip.is_loopback() {
            Self::Loopback
        } else {
            Self::Remote
        }
    }

    /// Classify a peer by its remote socket address.
    pub(crate) fn from_addr(addr: &SocketAddr) -> Self {
        Self::from_ip(addr.ip())
    }
}

/// Why a connection was denied — a stable, non-sensitive reason safe to return
/// to a client and to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DenyReason {
    /// Remote peer presented no bearer token.
    MissingToken,
    /// Remote peer presented a token that did not match.
    BadToken,
    /// Remote peer, but the surface has no token configured to check against.
    NoTokenConfigured,
}

impl DenyReason {
    /// A stable, non-sensitive description of the denial.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingToken => "missing bearer token",
            Self::BadToken => "invalid bearer token",
            Self::NoTokenConfigured => {
                "remote connections require a bearer token, but none is configured"
            }
        }
    }
}

/// The result of an authorization check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthOutcome {
    /// The connection is authorized.
    Allow,
    /// The connection is rejected, with a reason safe to surface.
    Deny(DenyReason),
}

impl AuthOutcome {
    /// Whether the outcome authorizes the connection.
    pub(crate) fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// The shared auth gate: an optional configured bearer token, applied uniformly
/// by every drive surface. One instance is the single source of truth for the
/// policy across MCP-server / ACP / HTTP serve.
#[derive(Clone, Default)]
pub(crate) struct AuthGate {
    /// The required bearer token for remote peers. `None` denies every remote
    /// peer (loopback-only).
    required: Option<SecretString>,
}

impl std::fmt::Debug for AuthGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the token — only whether one is set.
        f.debug_struct("AuthGate")
            .field(
                "token",
                if self.required.is_some() {
                    &"<set>"
                } else {
                    &"<unset>"
                },
            )
            .finish()
    }
}

impl AuthGate {
    /// A gate configured with an explicit optional token. An empty or
    /// whitespace-only token is treated as absent (matching ADR 0051).
    pub(crate) fn new(token: Option<String>) -> Self {
        let required = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .map(SecretString::from);
        Self { required }
    }

    /// Build a gate from the environment ([`TOKEN_ENV`]). Absent / empty → no
    /// token (loopback-only).
    pub(crate) fn from_env() -> Self {
        Self::new(std::env::var(TOKEN_ENV).ok())
    }

    /// Whether a bearer token is configured.
    pub(crate) fn has_token(&self) -> bool {
        self.required.is_some()
    }

    /// Authorize a connection from `peer` presenting `presented` (the bearer
    /// token extracted from the transport, if any).
    pub(crate) fn authorize(&self, peer: Peer, presented: Option<&str>) -> AuthOutcome {
        match peer {
            Peer::Loopback => AuthOutcome::Allow,
            Peer::Remote => match &self.required {
                None => AuthOutcome::Deny(DenyReason::NoTokenConfigured),
                Some(required) => match presented {
                    None => AuthOutcome::Deny(DenyReason::MissingToken),
                    Some(tok)
                        if constant_time_eq(
                            tok.as_bytes(),
                            required.expose_secret().as_bytes(),
                        ) =>
                    {
                        AuthOutcome::Allow
                    }
                    Some(_) => AuthOutcome::Deny(DenyReason::BadToken),
                },
            },
        }
    }

    /// Fail-closed bind policy: a surface must not bind to a non-loopback address
    /// without a token configured (ADR 0051 — never serve unauthenticated on the
    /// network). Loopback binds are always allowed.
    ///
    /// # Errors
    ///
    /// Returns [`BindPolicyError::UnauthenticatedRemoteBind`] when the bind would
    /// be unauthenticated.
    pub(crate) fn check_bind(&self, addr: &SocketAddr) -> Result<(), BindPolicyError> {
        if Peer::from_addr(addr) == Peer::Remote && self.required.is_none() {
            return Err(BindPolicyError::UnauthenticatedRemoteBind);
        }
        Ok(())
    }
}

/// Error returned when a bind would violate the fail-closed network policy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum BindPolicyError {
    /// A non-loopback bind was attempted with no bearer token configured.
    #[error(
        "refusing to bind a drive surface to a non-loopback address without a bearer token (set {TOKEN_ENV})"
    )]
    UnauthenticatedRemoteBind,
}

/// Length-then-content constant-time byte comparison (mirrors the caliband
/// bearer check, ADR 0051 / #401). Leaks the length of the expected token, but
/// not its contents — an accepted tradeoff for a locally-generated token.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    use super::{AuthGate, AuthOutcome, DenyReason, Peer};

    fn loopback_v4() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 7777))
    }
    fn remote_v4() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(10, 0, 0, 5), 7777))
    }

    #[test]
    fn peer_classification() {
        assert_eq!(Peer::from_ip(Ipv4Addr::LOCALHOST.into()), Peer::Loopback);
        assert_eq!(Peer::from_ip(Ipv6Addr::LOCALHOST.into()), Peer::Loopback);
        assert_eq!(
            Peer::from_ip(Ipv4Addr::new(192, 168, 1, 4).into()),
            Peer::Remote
        );
    }

    #[test]
    fn loopback_is_open_regardless_of_token() {
        // No token configured.
        let gate = AuthGate::new(None);
        assert!(gate.authorize(Peer::Loopback, None).is_allowed());
        // Token configured — loopback still needs none.
        let gate = AuthGate::new(Some("s3cret".into()));
        assert!(gate.authorize(Peer::Loopback, None).is_allowed());
        assert!(
            gate.authorize(Peer::Loopback, Some("anything"))
                .is_allowed()
        );
    }

    #[test]
    fn remote_without_configured_token_is_denied() {
        let gate = AuthGate::new(None);
        assert_eq!(
            gate.authorize(Peer::Remote, Some("whatever")),
            AuthOutcome::Deny(DenyReason::NoTokenConfigured)
        );
    }

    #[test]
    fn remote_requires_matching_token() {
        let gate = AuthGate::new(Some("correct-horse".into()));
        assert_eq!(
            gate.authorize(Peer::Remote, None),
            AuthOutcome::Deny(DenyReason::MissingToken)
        );
        assert_eq!(
            gate.authorize(Peer::Remote, Some("wrong")),
            AuthOutcome::Deny(DenyReason::BadToken)
        );
        assert_eq!(
            gate.authorize(Peer::Remote, Some("correct-horse")),
            AuthOutcome::Allow
        );
    }

    #[test]
    fn empty_or_blank_token_is_treated_as_absent() {
        assert!(!AuthGate::new(Some(String::new())).has_token());
        assert!(!AuthGate::new(Some("   ".into())).has_token());
        assert!(AuthGate::new(Some("x".into())).has_token());
    }

    #[test]
    fn bind_policy_fails_closed_for_unauthenticated_remote() {
        // Loopback always binds.
        assert!(AuthGate::new(None).check_bind(&loopback_v4()).is_ok());
        // Remote without a token is refused.
        assert!(AuthGate::new(None).check_bind(&remote_v4()).is_err());
        // Remote with a token is allowed.
        assert!(
            AuthGate::new(Some("tok".into()))
                .check_bind(&remote_v4())
                .is_ok()
        );
    }

    #[test]
    fn debug_never_leaks_the_token() {
        let gate = AuthGate::new(Some("super-secret-value".into()));
        let rendered = format!("{gate:?}");
        assert!(!rendered.contains("super-secret-value"), "{rendered}");
        assert!(rendered.contains("<set>"));
    }
}
