//! Network-agnostic transport seam for the caliband protocol.
//!
//! Turns an [`Endpoint`] (+ optional TLS + optional bearer token) into a
//! duplex byte stream, either as a server ([`Listener`]) or client
//! ([`connect`]). The NDJSON protocol (`proto`, `TurnEvent`, `AttachInbound`)
//! rides *on top* of a [`BoxConn`] unchanged — TLS and the token preamble are
//! transport framing below it. See ADR 0051.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

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

/// Server-side TLS material. Filled in Task 2.
#[derive(Clone)]
pub struct TlsServer {}

/// Client-side TLS material. Filled in Task 2.
#[derive(Clone)]
pub struct TlsClient {}

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

    /// Accept one connection, returning a boxed duplex stream. (TLS
    /// handshake + token check are added in Tasks 2–3.)
    pub async fn accept(&self) -> std::io::Result<BoxConn> {
        match self {
            Listener::Unix(l) => {
                let (stream, _addr) = l.accept().await?;
                Ok(Box::new(stream))
            }
            Listener::Tcp { listener, .. } => {
                let (stream, _addr) = listener.accept().await?;
                Ok(Box::new(stream))
            }
        }
    }
}

/// Dial a connection per `spec`. (TLS + token preamble added in Tasks 2–3.)
pub async fn connect(spec: &ConnectSpec) -> std::io::Result<BoxConn> {
    match &spec.endpoint {
        Endpoint::Unix { path } => {
            let stream = UnixStream::connect(path).await?;
            Ok(Box::new(stream))
        }
        Endpoint::Tcp { addr } => {
            let stream = TcpStream::connect(addr).await?;
            Ok(Box::new(stream))
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
}
