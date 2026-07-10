//! Network-agnostic transport seam for the caliband protocol.
//!
//! Turns an [`Endpoint`] (+ optional TLS + optional bearer token) into a
//! duplex byte stream, either as a server ([`Listener`]) or client
//! ([`connect`]). The NDJSON protocol (`proto`, `TurnEvent`, `AttachInbound`)
//! rides *on top* of a [`BoxConn`] unchanged — TLS and the token preamble are
//! transport framing below it. See ADR 0051.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// Where a caliband socket lives, independent of transport family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "snake_case")]
pub enum Endpoint {
    /// Local Unix-domain socket at this filesystem path.
    Unix {
        /// Socket file path.
        path: PathBuf,
    },
    /// TCP endpoint as a `host:port` string (host may be a DNS name).
    Tcp {
        /// `host:port`.
        addr: String,
    },
}

/// A duplex byte stream over any transport family.
pub trait Conn: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Conn for T {}

/// Boxed duplex connection handed to the NDJSON protocol layer.
pub type BoxConn = Box<dyn Conn>;

/// Server-side TLS material.
#[derive(Clone)]
pub struct TlsServer {
    /// Handshake acceptor built from a cert chain + private key.
    pub acceptor: TlsAcceptor,
}

/// Client-side TLS material.
#[derive(Clone)]
pub struct TlsClient {
    /// Handshake connector built from a trusted CA store.
    pub connector: TlsConnector,
    /// Expected server name (SNI / cert validation target).
    pub server_name: String,
}

/// Install the `ring` crypto provider as the process default, exactly once.
fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

/// Build server TLS from a PEM cert chain + private key.
pub fn tls_server_from_pem(cert_pem: &[u8], key_pem: &[u8]) -> std::io::Result<TlsServer> {
    ensure_crypto_provider();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let key: PrivateKeyDer<'static> =
        PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| std::io::Error::other(e.to_string()))?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(std::io::Error::other)?;
    Ok(TlsServer {
        acceptor: TlsAcceptor::from(Arc::new(config)),
    })
}

/// Build client TLS trusting `ca_pem`, verifying the server presents `server_name`.
pub fn tls_client_from_pem(ca_pem: &[u8], server_name: &str) -> std::io::Result<TlsClient> {
    ensure_crypto_provider();
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(ca_pem) {
        roots
            .add(cert.map_err(|e| std::io::Error::other(e.to_string()))?)
            .map_err(std::io::Error::other)?;
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsClient {
        connector: TlsConnector::from(Arc::new(config)),
        server_name: server_name.to_string(),
    })
}

/// Wire format of the bearer-token preamble: a single JSON object on its own
/// line, `{"bearer":"<token>"}\n`. Sits below the NDJSON protocol — TCP only,
/// applied after the (optional) TLS handshake so the token travels encrypted
/// when TLS is on. Unix connections never send or expect this.
#[derive(Serialize, Deserialize)]
struct TokenPreamble {
    bearer: String,
}

/// Read one `\n`-terminated line byte-by-byte (bounded), so nothing past the
/// newline is consumed from the protocol stream that follows.
async fn read_preamble_line(conn: &mut BoxConn) -> std::io::Result<String> {
    let mut buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        let n = conn.read(&mut byte).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "no token preamble",
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > 4096 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "token preamble too long",
            ));
        }
    }
    String::from_utf8(buf).map_err(std::io::Error::other)
}

/// Read and validate the bearer-token preamble on `conn`, failing with
/// `PermissionDenied` if it's missing or doesn't match `expected`.
async fn server_check_token(conn: &mut BoxConn, expected: &str) -> std::io::Result<()> {
    let line = read_preamble_line(conn).await?;
    let preamble: TokenPreamble = serde_json::from_str(&line).map_err(std::io::Error::other)?;
    // Constant-time compare: the bearer token is the sole auth factor on the
    // network plane, so a byte-wise short-circuit (`==`) leaks it to a timing
    // attacker over many trials (#401).
    if constant_time_eq(preamble.bearer.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "bad bearer token",
        ))
    }
}

/// Length-then-content constant-time byte comparison. The length check can
/// leak the *length* of the expected token (not its contents), which is an
/// accepted tradeoff for a locally-generated bearer token.
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

/// Write the bearer-token preamble line onto `conn`.
async fn client_send_token(conn: &mut BoxConn, token: &str) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&TokenPreamble {
        bearer: token.to_string(),
    })
    .map_err(std::io::Error::other)?;
    line.push(b'\n');
    conn.write_all(&line).await?;
    conn.flush().await
}

/// How to bind a listener.
pub struct BindSpec {
    /// Address family + address.
    pub endpoint: Endpoint,
    /// TLS (TCP only). `None` = plaintext. Used from Task 2.
    pub tls: Option<TlsServer>,
    /// Required bearer token for network connections. Used from Task 3.
    pub token: Option<String>,
}

/// Fail-closed credential policy for a network (TCP) listener (#288, #400).
///
/// A `--listen` listener must never bind unauthenticated, and must never carry
/// a bearer token over plaintext (an on-path observer could steal it). Returns
/// `Err(reason)` unless **both** a non-empty token and TLS are configured.
/// Empty/whitespace-only tokens are treated as absent. Unix-socket mode is
/// local and filesystem-permission-scoped, so it does not use this guard.
///
/// This is the single source of truth for the policy, applied to **both** the
/// daemon control-plane listener (`caliband --listen`) and the per-agent
/// session-plane listener the worker binds — the #288 fix originally guarded
/// only the latter, leaving the control socket fail-open (#400).
pub fn require_network_credentials(token: Option<&str>, tls_present: bool) -> Result<(), String> {
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    if token.is_none() {
        return Err(
            "a non-empty bearer token is required for network (--listen) mode; \
             refusing to bind an unauthenticated listener"
                .to_owned(),
        );
    }
    if !tls_present {
        return Err(
            "TLS (--tls-cert/--tls-key) is required for network (--listen) mode; \
             refusing to send the bearer token over plaintext"
                .to_owned(),
        );
    }
    Ok(())
}

/// How to dial a connection.
pub struct ConnectSpec {
    /// Target address.
    pub endpoint: Endpoint,
    /// TLS (TCP only). Used from Task 2.
    pub tls: Option<TlsClient>,
    /// Bearer token to present. Used from Task 3.
    pub token: Option<String>,
}

/// A raw accepted connection whose TLS handshake and token check have **not**
/// yet run. Produced by [`Listener::accept_raw`] so the accept loop returns
/// immediately; the handshake/auth is deferred to [`Incoming::authenticate`]
/// (run in a spawned task under a timeout) so a slow peer can't wedge the loop.
pub struct Incoming {
    stream: RawStream,
    tls: Option<TlsAcceptor>,
    token: Option<String>,
}

enum RawStream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Incoming {
    /// Perform the (optional) TLS handshake, then the (optional) token check.
    /// Consumes `self`, yielding the ready [`BoxConn`]. Intended to be wrapped
    /// in `tokio::time::timeout` by the caller.
    pub async fn authenticate(self) -> std::io::Result<BoxConn> {
        let mut conn: BoxConn = match self.stream {
            RawStream::Unix(s) => Box::new(s),
            RawStream::Tcp(s) => match self.tls {
                None => Box::new(s),
                Some(acceptor) => Box::new(acceptor.accept(s).await?),
            },
        };
        if let Some(expected) = self.token {
            server_check_token(&mut conn, &expected).await?;
        }
        Ok(conn)
    }
}

/// A bound listener over one transport family.
pub enum Listener {
    /// Unix-domain.
    Unix(UnixListener),
    /// TCP (TLS/token applied at accept-time from Task 2/3).
    Tcp {
        /// Underlying listener.
        listener: TcpListener,
        /// Server TLS material, if any.
        tls: Option<TlsServer>,
        /// Required bearer token, if any.
        token: Option<String>,
    },
}

impl Listener {
    /// Bind a listener per `spec`.
    pub async fn bind(spec: &BindSpec) -> std::io::Result<Listener> {
        match &spec.endpoint {
            Endpoint::Unix { path } => {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                let _ = tokio::fs::remove_file(path).await;
                Ok(Listener::Unix(UnixListener::bind(path)?))
            }
            Endpoint::Tcp { addr } => {
                let listener = TcpListener::bind(addr).await?;
                Ok(Listener::Tcp {
                    listener,
                    tls: spec.tls.clone(),
                    token: spec.token.clone(),
                })
            }
        }
    }

    /// The actually-bound TCP address (resolves `:0` to the real port).
    /// Returns `None` for a Unix listener.
    pub fn local_addr(&self) -> Option<String> {
        match self {
            Listener::Unix(_) => None,
            Listener::Tcp { listener, .. } => listener.local_addr().ok().map(|a| a.to_string()),
        }
    }

    /// Accept one connection, returning a boxed duplex stream. Performs the
    /// TLS handshake when server TLS is configured, then — for TCP — checks
    /// the bearer-token preamble when a token is configured.
    pub async fn accept(&self) -> std::io::Result<BoxConn> {
        self.accept_raw().await?.authenticate().await
    }

    /// Accept the next connection **without** performing the TLS handshake or
    /// token check. Returns immediately after the raw `accept(2)`, so a slow or
    /// silent peer can never stall the accept loop (#401). The caller runs
    /// [`Incoming::authenticate`] on the returned value — typically inside a
    /// spawned task under a timeout — so the handshake happens concurrently.
    pub async fn accept_raw(&self) -> std::io::Result<Incoming> {
        match self {
            Listener::Unix(l) => {
                let (stream, _addr) = l.accept().await?;
                Ok(Incoming {
                    stream: RawStream::Unix(stream),
                    tls: None,
                    token: None,
                })
            }
            Listener::Tcp {
                listener,
                tls,
                token,
            } => {
                let (stream, _addr) = listener.accept().await?;
                Ok(Incoming {
                    stream: RawStream::Tcp(stream),
                    tls: tls.as_ref().map(|t| t.acceptor.clone()),
                    token: token.clone(),
                })
            }
        }
    }
}

/// Dial a connection per `spec`. Performs the TLS handshake when client TLS
/// is configured, then — for TCP — sends the bearer-token preamble when a
/// token is configured.
pub async fn connect(spec: &ConnectSpec) -> std::io::Result<BoxConn> {
    match &spec.endpoint {
        Endpoint::Unix { path } => {
            let stream = UnixStream::connect(path).await?;
            Ok(Box::new(stream))
        }
        Endpoint::Tcp { addr } => {
            let stream = TcpStream::connect(addr).await?;
            let mut conn: BoxConn = match &spec.tls {
                None => Box::new(stream),
                Some(t) => {
                    let name = ServerName::try_from(t.server_name.clone())
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                    Box::new(t.connector.connect(name, stream).await?)
                }
            };
            if let Some(token) = &spec.token {
                client_send_token(&mut conn, token).await?;
            }
            Ok(conn)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_semantics_of_plain_eq() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secr")); // length differs
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn require_network_credentials_rejects_missing_token() {
        let err = require_network_credentials(None, true).unwrap_err();
        assert!(err.contains("token"), "unexpected: {err}");
    }

    #[test]
    fn require_network_credentials_rejects_empty_or_whitespace_token() {
        assert!(require_network_credentials(Some(""), true).is_err());
        assert!(require_network_credentials(Some("   "), true).is_err());
    }

    #[test]
    fn require_network_credentials_rejects_token_without_tls() {
        let err = require_network_credentials(Some("secret"), false).unwrap_err();
        assert!(err.contains("TLS"), "unexpected: {err}");
    }

    #[test]
    fn require_network_credentials_accepts_token_and_tls() {
        assert!(require_network_credentials(Some("secret"), true).is_ok());
    }

    async fn echo_once(listener: Listener) {
        let mut conn = listener.accept().await.expect("accept");
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).await.expect("read");
        conn.write_all(&buf).await.expect("write");
        conn.flush().await.expect("flush");
    }

    #[tokio::test]
    async fn unix_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let bind = BindSpec {
            endpoint: Endpoint::Unix { path: path.clone() },
            tls: None,
            token: None,
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let server = tokio::spawn(echo_once(listener));
        let mut c = connect(&ConnectSpec {
            endpoint: Endpoint::Unix { path },
            tls: None,
            token: None,
        })
        .await
        .unwrap();
        c.write_all(b"hello").await.unwrap();
        let mut got = [0u8; 5];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tcp_roundtrip() {
        let bind = BindSpec {
            endpoint: Endpoint::Tcp {
                addr: "127.0.0.1:0".into(),
            },
            tls: None,
            token: None,
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap(); // real bound "127.0.0.1:PORT"
        let server = tokio::spawn(echo_once(listener));
        let mut c = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr },
            tls: None,
            token: None,
        })
        .await
        .unwrap();
        c.write_all(b"world").await.unwrap();
        let mut got = [0u8; 5];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"world");
        server.await.unwrap();
    }

    #[test]
    fn endpoint_serde_tagged() {
        let e = Endpoint::Tcp { addr: "h:7".into() };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(v["scheme"], "tcp");
        assert_eq!(v["addr"], "h:7");
    }

    fn test_certs() -> (Vec<u8>, Vec<u8>) {
        // rcgen self-signed cert for "localhost".
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        (
            cert.cert.pem().into_bytes(),
            cert.key_pair.serialize_pem().into_bytes(),
        )
    }

    #[tokio::test]
    async fn tcp_tls_roundtrip() {
        let (cert_pem, key_pem) = test_certs();
        let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();
        let bind = BindSpec {
            endpoint: Endpoint::Tcp {
                addr: "127.0.0.1:0".into(),
            },
            tls: Some(tls_server),
            token: None,
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(echo_once(listener));
        // Client trusts the self-signed cert as its CA, expects name "localhost".
        let tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
        let mut c = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr },
            tls: Some(tls_client),
            token: None,
        })
        .await
        .unwrap();
        c.write_all(b"tls!!").await.unwrap();
        let mut got = [0u8; 5];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"tls!!");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tcp_token_accept_and_reject() {
        let bind = BindSpec {
            endpoint: Endpoint::Tcp {
                addr: "127.0.0.1:0".into(),
            },
            tls: None,
            token: Some("s3cret".into()),
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Server accepts twice: once good, once bad.
        let srv = tokio::spawn(async move {
            let good = listener.accept().await; // good token → Ok
            let bad = listener.accept().await; // bad token  → Err(PermissionDenied)
            (good.is_ok(), bad.err().map(|e| e.kind()))
        });

        // Good client.
        let mut ok = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr: addr.clone() },
            tls: None,
            token: Some("s3cret".into()),
        })
        .await
        .unwrap();
        ok.write_all(b"x").await.unwrap(); // keep the conn alive briefly

        // Bad client.
        let bad = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr },
            tls: None,
            token: Some("wrong".into()),
        })
        .await;
        // connect() itself succeeds at the TCP layer; the server rejects post-preamble.
        // The bad conn may connect but the server-side accept errored.
        drop(bad);

        let (good_ok, bad_kind) = srv.await.unwrap();
        assert!(good_ok, "good token should be accepted");
        assert_eq!(bad_kind, Some(std::io::ErrorKind::PermissionDenied));
    }
}
