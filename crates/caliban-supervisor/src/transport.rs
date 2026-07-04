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
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
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
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<Result<_, _>>()
        .map_err(std::io::Error::other)?;
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut &key_pem[..])
        .map_err(std::io::Error::other)?
        .ok_or_else(|| std::io::Error::other("no private key in PEM"))?;
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
    for cert in rustls_pemfile::certs(&mut &ca_pem[..]) {
        roots
            .add(cert.map_err(std::io::Error::other)?)
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

/// How to bind a listener.
pub struct BindSpec {
    /// Address family + address.
    pub endpoint: Endpoint,
    /// TLS (TCP only). `None` = plaintext. Used from Task 2.
    pub tls: Option<TlsServer>,
    /// Required bearer token for network connections. Used from Task 3.
    pub token: Option<String>,
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
    /// TLS handshake when server TLS is configured. (Token check added in
    /// Task 3.)
    pub async fn accept(&self) -> std::io::Result<BoxConn> {
        match self {
            Listener::Unix(l) => {
                let (stream, _addr) = l.accept().await?;
                Ok(Box::new(stream))
            }
            Listener::Tcp { listener, tls, .. } => {
                let (stream, _addr) = listener.accept().await?;
                match tls {
                    None => Ok(Box::new(stream)),
                    Some(t) => {
                        let tls_stream = t.acceptor.accept(stream).await?;
                        Ok(Box::new(tls_stream))
                    }
                }
            }
        }
    }
}

/// Dial a connection per `spec`. Performs the TLS handshake when client TLS
/// is configured. (Token preamble added in Task 3.)
pub async fn connect(spec: &ConnectSpec) -> std::io::Result<BoxConn> {
    match &spec.endpoint {
        Endpoint::Unix { path } => {
            let stream = UnixStream::connect(path).await?;
            Ok(Box::new(stream))
        }
        Endpoint::Tcp { addr } => {
            let stream = TcpStream::connect(addr).await?;
            match &spec.tls {
                None => Ok(Box::new(stream)),
                Some(t) => {
                    let name = ServerName::try_from(t.server_name.clone())
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                    let tls_stream = t.connector.connect(name, stream).await?;
                    Ok(Box::new(tls_stream))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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
}
