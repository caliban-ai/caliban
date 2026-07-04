# caliband Network Transport (NDJSON over TCP+TLS + bearer) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give caliband a network transport that carries its existing NDJSON control + per-agent-stream protocol over TCP with TLS and a bearer token, while the Unix-domain socket stays the unchanged local default.

**Architecture:** Introduce a `transport` module in `caliban-supervisor` exposing an `Endpoint` (Unix path or TCP `host:port`), a `Listener` that accepts boxed duplex connections, and a `connect()` that dials them — with optional rustls TLS and an optional bearer-token preamble folded into the transport layer so the NDJSON protocol code above it is byte-for-byte unchanged. The control server (`Supervisor`), the control client (`SupervisorClient`), the per-agent worker listener (`caliban/src/worker.rs`), and the attach client (`caliban/src/agents_cli.rs`) all migrate off concrete `UnixListener`/`UnixStream` onto this seam — Unix-first as a pure refactor, then network mode is switched on by config. See ADR 0051.

**Tech Stack:** Rust, tokio, serde_json (NDJSON on the wire, unchanged), `tokio-rustls` (ring provider), `rustls-pemfile`, `rcgen` (test certs).

## Global Constraints

- **Decision A / ADR 0051 governs:** NDJSON message types (`CtlRequest`/`CtlReply` in `caliban_supervisor::proto`, `TurnEvent`/`AttachInbound`) stay **byte-for-byte on the wire** — this is a transport lift, not a protocol change. The bearer token and TLS handshake are transport framing that sit *below* the NDJSON, not new protocol messages.
- **Unix path unchanged & default:** with no network config, caliband binds a Unix socket exactly as today; all existing tests pass unmodified. Local Unix connections are unauthenticated (filesystem permissions are the boundary) and un-encrypted, as today.
- **gRPC is explicitly out of scope** — it is deferred to #314. Do not add tonic/prost.
- **TLS provider:** use `tokio-rustls` with the **`ring`** crypto provider (`default-features = false, features = ["ring", "tls12"]`). Do **not** pull `aws-lc-rs` (the workspace deliberately avoids it — see `Cargo.toml` reqwest/aws-sdk comments).
- **Mode is mutually exclusive & config-selected**, mirroring caliban's `--database-url` topology switch: a daemon is either Unix-mode or TCP-mode for its whole lifetime. Endpoints within one daemon are therefore all one scheme.
- **Rust edition/lints:** workspace lints are `-D warnings`-strict (clippy pedantic in places). Every task ends green under `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
- **No `unwrap()`/`expect()` in non-test code** on the transport path; surface `std::io::Error` (the existing protocol I/O convention).

---

## File Structure

- **Create** `crates/caliban-supervisor/src/transport.rs` — the transport seam: `Endpoint`, `BoxConn`/`Conn`, `TlsServer`/`TlsClient`, `BindSpec`/`ConnectSpec`, `Listener`, `connect()`, PEM helpers, token preamble. One responsibility: turn an `Endpoint` (+ optional TLS + optional token) into a duplex byte stream, either as a server (accept) or a client (connect).
- **Modify** `crates/caliban-supervisor/src/lib.rs` — declare + re-export `transport`.
- **Modify** `crates/caliban-supervisor/src/proto.rs` — replace `socket_path: PathBuf` endpoint fields with `endpoint: Endpoint`.
- **Modify** `crates/caliban-supervisor/src/server.rs` — `Supervisor` binds through `transport::Listener`; `handle_client` over a `BoxConn`; per-agent endpoint assignment (Unix path vs TCP port).
- **Modify** `crates/caliban-supervisor/src/client.rs` — `SupervisorClient` dials through `transport::connect`; carries a `ConnectSpec`.
- **Modify** `crates/caliban-supervisor/src/bin/caliband.rs` — new CLI flags/env for network mode; build a `BindSpec`.
- **Modify** `caliban/src/worker.rs` — per-agent listener binds through `transport::Listener`; `serve_attach_client` over a `BoxConn`; worker learns its listen endpoint from an arg.
- **Modify** `caliban/src/agents_cli.rs` — `run_attach` dials through `transport::connect`; daemon-launch passes the per-agent listen endpoint; client reads network config from env.
- **Modify** `crates/caliban-supervisor/Cargo.toml` + workspace `Cargo.toml` — add `tokio-rustls`, `rustls-pemfile`, dev `rcgen`.
- **Create** `crates/caliban-supervisor/tests/network_transport.rs` — end-to-end control-plane-over-TCP+TLS+token test (Task 7).
- **Create** `caliban/tests/attach_over_network.rs` — end-to-end per-agent-stream-over-TCP+TLS+token attach test (Task 8).

---

### Task 1: Transport module — `Endpoint` + Unix/TCP `Listener`/`connect` (no TLS, no token)

Establishes the seam with plain transports. TLS and token arrive in Tasks 2–3.

**Files:**
- Create: `crates/caliban-supervisor/src/transport.rs`
- Modify: `crates/caliban-supervisor/src/lib.rs` (add `pub mod transport;` and re-exports)
- Test: inline `#[cfg(test)]` in `transport.rs`

**Interfaces:**
- Produces:
  - `pub enum Endpoint { Unix { path: PathBuf }, Tcp { addr: String } }` — `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]`, `#[serde(tag = "scheme", rename_all = "snake_case")]`.
  - `pub trait Conn: AsyncRead + AsyncWrite + Unpin + Send {}` with blanket impl; `pub type BoxConn = Box<dyn Conn>;`
  - `pub struct BindSpec { pub endpoint: Endpoint, pub tls: Option<TlsServer>, pub token: Option<String> }` (TLS/token fields defined here but unused until Tasks 2–3; construct with `tls: None, token: None`).
  - `pub struct ConnectSpec { pub endpoint: Endpoint, pub tls: Option<TlsClient>, pub token: Option<String> }`.
  - `pub enum Listener { … }` with `pub async fn bind(spec: &BindSpec) -> std::io::Result<Listener>` and `pub async fn accept(&self) -> std::io::Result<BoxConn>`.
  - `pub async fn connect(spec: &ConnectSpec) -> std::io::Result<BoxConn>`.
  - Placeholder types (filled in Task 2): `pub struct TlsServer;` and `pub struct TlsClient;` — define as empty `#[derive(Clone)] pub struct TlsServer {}` now so `BindSpec`/`ConnectSpec` compile; Task 2 gives them fields.

- [ ] **Step 1: Write the failing test** (`transport.rs` `#[cfg(test)]`)

```rust
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
        let bind = BindSpec { endpoint: Endpoint::Unix { path: path.clone() }, tls: None, token: None };
        let listener = Listener::bind(&bind).await.unwrap();
        let server = tokio::spawn(echo_once(listener));
        let mut c = connect(&ConnectSpec { endpoint: Endpoint::Unix { path }, tls: None, token: None }).await.unwrap();
        c.write_all(b"hello").await.unwrap();
        let mut got = [0u8; 5];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tcp_roundtrip() {
        let bind = BindSpec { endpoint: Endpoint::Tcp { addr: "127.0.0.1:0".into() }, tls: None, token: None };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap(); // real bound "127.0.0.1:PORT"
        let server = tokio::spawn(echo_once(listener));
        let mut c = connect(&ConnectSpec { endpoint: Endpoint::Tcp { addr }, tls: None, token: None }).await.unwrap();
        c.write_all(b"world").await.unwrap();
        let mut got = [0u8; 5];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"world");
        server.await.unwrap();
    }

    #[test]
    fn endpoint_serde_tagged() {
        let e = Endpoint::Tcp { addr: "h:7".into() };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(v["scheme"], "tcp");
        assert_eq!(v["addr"], "h:7");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-supervisor transport::tests -- --nocapture`
Expected: FAIL — `transport` module does not exist.

- [ ] **Step 3: Write minimal implementation** (`transport.rs`)

```rust
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
```

Add to `crates/caliban-supervisor/src/lib.rs` (place the `pub mod` with the other module declarations, and re-export the primary types next to the existing `pub use`):

```rust
pub mod transport;
pub use transport::{connect, BindSpec, BoxConn, ConnectSpec, Endpoint, Listener};
```

Add deps — workspace `Cargo.toml` `[workspace.dependencies]` table:

```toml
tokio-rustls   = { version = "0.26", default-features = false, features = ["ring", "tls12"] }
rustls-pemfile = "2"
rcgen          = "0.13"
```

And `crates/caliban-supervisor/Cargo.toml`:

```toml
# under [dependencies]
tokio-rustls   = { workspace = true }
rustls-pemfile = { workspace = true }
# under [dev-dependencies]
rcgen          = { workspace = true }
```

(Task 1 does not yet *use* `tokio-rustls`/`rustls-pemfile`; if the `-D warnings` unused-crate-dependency lint is active, defer adding them to Task 2. Check with `cargo clippy -p caliban-supervisor -- -D warnings` — if it flags unused deps, move the two `[dependencies]` lines to Task 2's diff.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p caliban-supervisor transport::tests -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-supervisor/src/transport.rs crates/caliban-supervisor/src/lib.rs crates/caliban-supervisor/Cargo.toml Cargo.toml Cargo.lock
git commit -m "feat(supervisor): transport seam — Endpoint + Unix/TCP Listener/connect (#280)"
```

---

### Task 2: TLS in the transport (rustls acceptor/connector + PEM helpers)

**Files:**
- Modify: `crates/caliban-supervisor/src/transport.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `Endpoint`, `Listener`, `connect`, `BindSpec`, `ConnectSpec`, `BoxConn` (Task 1).
- Produces:
  - `TlsServer { acceptor: tokio_rustls::TlsAcceptor }` (replaces the empty struct).
  - `TlsClient { connector: tokio_rustls::TlsConnector, server_name: String }`.
  - `pub fn tls_server_from_pem(cert_pem: &[u8], key_pem: &[u8]) -> std::io::Result<TlsServer>`.
  - `pub fn tls_client_from_pem(ca_pem: &[u8], server_name: &str) -> std::io::Result<TlsClient>`.
  - `Listener::accept` performs the TLS handshake when `tls` is `Some`; `connect` performs it when `spec.tls` is `Some`.

- [ ] **Step 1: Write the failing test**

```rust
    // add to transport::tests
    fn test_certs() -> (Vec<u8>, Vec<u8>) {
        // rcgen self-signed cert for "localhost".
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        (cert.cert.pem().into_bytes(), cert.key_pair.serialize_pem().into_bytes())
    }

    #[tokio::test]
    async fn tcp_tls_roundtrip() {
        let (cert_pem, key_pem) = test_certs();
        let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();
        let bind = BindSpec {
            endpoint: Endpoint::Tcp { addr: "127.0.0.1:0".into() },
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
        }).await.unwrap();
        c.write_all(b"tls!!").await.unwrap();
        let mut got = [0u8; 5];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"tls!!");
        server.await.unwrap();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-supervisor transport::tests::tcp_tls_roundtrip`
Expected: FAIL — `tls_server_from_pem` not found.

- [ ] **Step 3: Write minimal implementation**

Replace the empty `TlsServer`/`TlsClient` and extend `accept`/`connect`. Key code:

```rust
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[derive(Clone)]
pub struct TlsServer {
    pub acceptor: TlsAcceptor,
}

#[derive(Clone)]
pub struct TlsClient {
    pub connector: TlsConnector,
    pub server_name: String,
}

/// Build server TLS from a PEM cert chain + PKCS#8 private key.
pub fn tls_server_from_pem(cert_pem: &[u8], key_pem: &[u8]) -> std::io::Result<TlsServer> {
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
    Ok(TlsServer { acceptor: TlsAcceptor::from(Arc::new(config)) })
}

/// Build client TLS trusting `ca_pem`, verifying the server presents `server_name`.
pub fn tls_client_from_pem(ca_pem: &[u8], server_name: &str) -> std::io::Result<TlsClient> {
    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &ca_pem[..]) {
        roots.add(cert.map_err(std::io::Error::other)?).map_err(std::io::Error::other)?;
    }
    let config = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    Ok(TlsClient {
        connector: TlsConnector::from(Arc::new(config)),
        server_name: server_name.to_string(),
    })
}
```

In `Listener::accept`, the `Tcp` arm becomes:

```rust
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
```

In `connect`, the `Tcp` arm becomes:

```rust
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
```

Note: install the ring crypto provider once. Add a helper called from both `tls_*_from_pem`:

```rust
fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}
```

Call `ensure_crypto_provider();` at the top of both `tls_server_from_pem` and `tls_client_from_pem`.

If Task 1 deferred the `tokio-rustls`/`rustls-pemfile` `[dependencies]` lines, add them now.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p caliban-supervisor transport::tests`
Expected: PASS (all Task 1 + `tcp_tls_roundtrip`).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-supervisor/src/transport.rs crates/caliban-supervisor/Cargo.toml Cargo.toml Cargo.lock
git commit -m "feat(supervisor): TLS (rustls/ring) in transport seam (#280)"
```

---

### Task 3: Bearer-token preamble in the transport

Network (TCP) connections present a one-line token preamble immediately after the (optional) TLS handshake; the server validates it before returning the connection. Unix connections skip it.

**Files:**
- Modify: `crates/caliban-supervisor/src/transport.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: everything from Tasks 1–2.
- Produces: `Listener::accept` rejects a TCP connection with a missing/wrong token (returns `std::io::Error` of kind `PermissionDenied`); `connect` writes the token preamble when `spec.token` is `Some`. Wire format of the preamble: exactly one line `{"bearer":"<token>"}\n` (JSON object with a single `bearer` string field), read/written **byte-by-byte up to the first `\n`** so no protocol bytes are consumed past it.

- [ ] **Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn tcp_token_accept_and_reject() {
        let bind = BindSpec {
            endpoint: Endpoint::Tcp { addr: "127.0.0.1:0".into() },
            tls: None,
            token: Some("s3cret".into()),
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Server accepts twice: once good, once bad.
        let srv = tokio::spawn(async move {
            let good = listener.accept().await; // good token → Ok
            let bad = listener.accept().await;  // bad token  → Err(PermissionDenied)
            (good.is_ok(), bad.err().map(|e| e.kind()))
        });

        // Good client.
        let mut ok = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr: addr.clone() },
            tls: None,
            token: Some("s3cret".into()),
        }).await.unwrap();
        ok.write_all(b"x").await.unwrap(); // keep the conn alive briefly

        // Bad client.
        let bad = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr },
            tls: None,
            token: Some("wrong".into()),
        }).await;
        // connect() itself succeeds at the TCP layer; the server rejects post-preamble.
        // The bad conn may connect but the server-side accept errored.
        drop(bad);

        let (good_ok, bad_kind) = srv.await.unwrap();
        assert!(good_ok, "good token should be accepted");
        assert_eq!(bad_kind, Some(std::io::ErrorKind::PermissionDenied));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-supervisor transport::tests::tcp_token_accept_and_reject`
Expected: FAIL — server currently ignores the token; `bad_kind` is `None`.

- [ ] **Step 3: Write minimal implementation**

Add preamble helpers and wire them into the TCP arms (they run *after* the TLS wrap, on the `BoxConn`):

```rust
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "no token preamble"));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > 4096 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "token preamble too long"));
        }
    }
    String::from_utf8(buf).map_err(std::io::Error::other)
}

async fn server_check_token(conn: &mut BoxConn, expected: &str) -> std::io::Result<()> {
    let line = read_preamble_line(conn).await?;
    let preamble: TokenPreamble = serde_json::from_str(&line).map_err(std::io::Error::other)?;
    // Constant-time-ish compare is overkill for a shared daemon token; plain eq.
    if preamble.bearer == expected {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "bad bearer token"))
    }
}

async fn client_send_token(conn: &mut BoxConn, token: &str) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&TokenPreamble { bearer: token.to_string() })
        .map_err(std::io::Error::other)?;
    line.push(b'\n');
    conn.write_all(&line).await?;
    conn.flush().await
}
```

In `Listener::accept`, the `Tcp` arm — after building the (possibly TLS) `BoxConn`, before returning it:

```rust
Listener::Tcp { listener, tls, token } => {
    let (stream, _addr) = listener.accept().await?;
    let mut conn: BoxConn = match tls {
        None => Box::new(stream),
        Some(t) => Box::new(t.acceptor.accept(stream).await?),
    };
    if let Some(expected) = token {
        server_check_token(&mut conn, expected).await?;
    }
    Ok(conn)
}
```

In `connect`, the `Tcp` arm — after building the `BoxConn`, before returning:

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p caliban-supervisor transport::tests`
Expected: PASS (all prior + `tcp_token_accept_and_reject`).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-supervisor/src/transport.rs
git commit -m "feat(supervisor): bearer-token preamble in transport seam (#280)"
```

---

### Task 4: Migrate `proto` endpoint fields to `Endpoint` (Unix-only, pure refactor)

Replace the four `socket_path: PathBuf` fields with `endpoint: Endpoint`, updating every compile site so the workspace stays green. No behavior change — everything constructs `Endpoint::Unix`.

**Files:**
- Modify: `crates/caliban-supervisor/src/proto.rs` (fields on `AgentRecord`, `DaemonStatus`, `CtlReply::Spawned`, `CtlReply::AttachAck`)
- Modify: `crates/caliban-supervisor/src/server.rs` (construct `Endpoint::Unix`; keep the `PathBuf` locally for filesystem cleanup)
- Modify: `crates/caliban-supervisor/src/client.rs` (`spawn`/`attach` return `Endpoint`)
- Modify: `crates/caliban-supervisor/src/registry.rs` (if it constructs `AgentRecord` — the `register(spec, socket_path)` signature; see note)
- Modify: `caliban/src/agents_cli.rs` (callers of `spawn`/`attach` + any `AgentRecord.socket_path` reads)
- Modify: `caliban/src/worker.rs` (if it reads `record.socket_path`)

**Interfaces:**
- Consumes: `Endpoint` (Task 1).
- Produces:
  - `AgentRecord.endpoint: Endpoint` (was `socket_path: PathBuf`); `DaemonStatus.endpoint: Endpoint` (was `socket_path`).
  - `CtlReply::Spawned { id, endpoint: Endpoint }`; `CtlReply::AttachAck { endpoint: Endpoint }`.
  - `SupervisorClient::spawn(...) -> Result<(AgentId, Endpoint), ClientError>`; `SupervisorClient::attach(...) -> Result<Endpoint, ClientError>`.

**Note on `Registry::register`:** it currently takes a `socket_path: PathBuf` and stores it on the record. Keep its parameter a `PathBuf` for the Unix case and construct `Endpoint::Unix { path }` inside `register` when building the `AgentRecord` — OR change its signature to take `endpoint: Endpoint`. Prefer changing `register(&mut self, spec: SpawnSpec, endpoint: Endpoint) -> AgentRecord` so the registry is transport-agnostic (Task 7 needs to register TCP agents). Read `registry.rs` first to confirm the exact signature and every caller.

- [ ] **Step 1: Update the failing test surface**

There is an existing test that asserts on `socket_path`. Search and update:

Run: `rg -n "socket_path" crates/caliban-supervisor caliban/src`
For each proto/reply/record usage, switch to `endpoint`. Where a test asserts the returned value is a path, assert it matches `Endpoint::Unix { path }`. Example transformation in a supervisor lifecycle test:

```rust
// before:
let (id, socket_path) = client.spawn(spec).await.unwrap();
assert!(socket_path.exists());
// after:
let (id, endpoint) = client.spawn(spec).await.unwrap();
let caliban_supervisor::Endpoint::Unix { path } = endpoint else { panic!("expected unix endpoint") };
assert!(path.exists());
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p caliban-supervisor --no-run`
Expected: FAIL — `socket_path` field/return no longer exists (after Step 3 edits) OR tests reference the new `endpoint` before the type changes. (Do Step 1 test edits and Step 3 source edits together; the checkpoint is a clean compile.)

- [ ] **Step 3: Make the change**

In `proto.rs`:
- `AgentRecord`: replace `pub socket_path: PathBuf,` with `pub endpoint: crate::transport::Endpoint,` (keep `session_dir: PathBuf`).
- `DaemonStatus`: replace `pub socket_path: PathBuf,` with `pub endpoint: crate::transport::Endpoint,`.
- `CtlReply::Spawned`: `socket_path: PathBuf` → `endpoint: crate::transport::Endpoint`.
- `CtlReply::AttachAck`: `socket_path: PathBuf` → `endpoint: crate::transport::Endpoint`.

In `server.rs` `dispatch`:
- `Spawn`/`Respawn`: keep computing the Unix `socket_path: PathBuf` (still needed for filesystem cleanup in `launch_and_monitor` and worker `--socket`), and construct `Endpoint::Unix { path: socket_path.clone() }` for the registry/reply. So `register(spec, Endpoint::Unix { path: socket_path.clone() })`, and separately retain the `PathBuf` for cleanup.
- `Attach`: `CtlReply::AttachAck { endpoint: rec.endpoint.clone() }`.
- `Status`: `endpoint: Endpoint::Unix { path: self.socket_path.clone() }`.

**Important:** `launch_and_monitor` unlinks `rec.socket_path` on worker exit and passes it to the launcher (`--socket`). Since `AgentRecord` no longer has `socket_path`, thread the Unix `PathBuf` separately. Read `proc.rs` `WorkerLauncher::launch(&rec)` — it reads `rec.socket_path`. Change the launcher to derive the socket path from `rec.endpoint` when it is `Endpoint::Unix`, or add a helper `AgentRecord::unix_socket_path(&self) -> Option<&Path>` returning `Some(path)` for `Endpoint::Unix`. Add that helper on `AgentRecord`:

```rust
impl AgentRecord {
    /// The Unix socket path, when this agent is served over a Unix socket.
    pub fn unix_socket_path(&self) -> Option<&std::path::Path> {
        match &self.endpoint {
            crate::transport::Endpoint::Unix { path } => Some(path.as_path()),
            crate::transport::Endpoint::Tcp { .. } => None,
        }
    }
}
```

Use `rec.unix_socket_path()` for the filesystem-cleanup and launcher `--socket` sites (Task 7 will add the TCP `--listen` branch).

In `client.rs`: `spawn` returns `(AgentId, Endpoint)`, `attach` returns `Endpoint` — match `CtlReply::Spawned { id, endpoint }` / `CtlReply::AttachAck { endpoint }`.

In `agents_cli.rs`: callers of `client.spawn()`/`client.attach()` now get an `Endpoint`; where they previously passed a `&Path` to `run_attach`, pass the `Endpoint` (Task 6 changes `run_attach`'s signature — for now, extract the Unix path: `if let Endpoint::Unix { path } = &endpoint { run_attach(path, id).await }`). Read the exact call sites first.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-supervisor && cargo test -p caliban`
Expected: PASS — all existing lifecycle/attach tests green; behavior unchanged (Unix endpoints throughout).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(supervisor): proto endpoints as Endpoint (Unix-only, no behavior change) (#280)"
```

---

### Task 5: Rewire control server + client through the transport seam (Unix mode)

Swap the concrete `UnixListener`/`UnixStream` in `Supervisor` and `SupervisorClient` for `transport::Listener`/`connect`. Still Unix-only; all tests stay green.

**Files:**
- Modify: `crates/caliban-supervisor/src/server.rs`
- Modify: `crates/caliban-supervisor/src/client.rs`

**Interfaces:**
- Consumes: `transport::{Listener, BindSpec, connect, ConnectSpec, Endpoint, BoxConn}`.
- Produces:
  - `Supervisor` binds via `Listener::bind(&BindSpec { endpoint: Endpoint::Unix { path: self.socket_path.clone() }, tls: None, token: None })`; `handle_client(self, conn: BoxConn)`.
  - `write_reply(write: &mut (dyn AsyncWrite + Unpin + Send), reply: &CtlReply)` (was `&mut OwnedWriteHalf`).
  - `SupervisorClient` holds a `ConnectSpec` (built from its endpoint); `request` dials `transport::connect`. Keep `SupervisorClient::new(impl Into<PathBuf>)` constructing a Unix `ConnectSpec` (back-compat for all existing callers).

- [ ] **Step 1: Confirm the existing tests are the safety net**

No new test. The full existing `caliban-supervisor` + `caliban` suites (daemon lifecycle, spawn/kill/respawn, attach) are the regression gate — they must stay green through this internal swap.

Run (baseline, before edits): `cargo test -p caliban-supervisor` → note the passing count.

- [ ] **Step 2: Make the server change** (`server.rs`)

- `serve`: replace the `UnixListener::bind` + `listener.accept()` with:

```rust
let bind = crate::transport::BindSpec {
    endpoint: crate::transport::Endpoint::Unix { path: self.socket_path.clone() },
    tls: None,
    token: None,
};
let listener = crate::transport::Listener::bind(&bind).await?;
```

(Drop the manual `remove_file` stale-unlink + `create_dir_all` for the socket path — `Listener::bind` does both for the Unix arm. Keep the `agent_runtime_dir` `create_dir_all` and the crashed-sweep block.)

In the accept loop:

```rust
accepted = listener.accept() => {
    match accepted {
        Ok(conn) => {
            let me = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = me.handle_client(conn).await {
                    tracing::warn!(error = %e, "client handler error");
                }
            });
        }
        Err(e) => tracing::warn!(error = %e, "accept failed"),
    }
}
```

- `handle_client`: signature `async fn handle_client(self: Arc<Self>, conn: BoxConn)`. Split with `tokio::io::split`:

```rust
async fn handle_client(self: Arc<Self>, conn: crate::transport::BoxConn) -> std::io::Result<()> {
    let (read_half, mut write_half) = tokio::io::split(conn);
    let mut reader = BufReader::new(read_half);
    // … loop unchanged, but write_reply now takes &mut write_half (a WriteHalf<BoxConn>) …
}
```

- `write_reply`: make it accept any writer:

```rust
async fn write_reply<W: tokio::io::AsyncWrite + Unpin>(
    stream: &mut W,
    reply: &CtlReply,
) -> std::io::Result<()> {
    let mut body = serde_json::to_vec(reply).map_err(std::io::Error::other)?;
    body.push(b'\n');
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}
```

Remove the now-unused `use tokio::net::{UnixListener, UnixStream};`.

- [ ] **Step 3: Make the client change** (`client.rs`)

Replace the `socket_path`-based `request` with a `ConnectSpec`:

```rust
pub struct SupervisorClient {
    spec_endpoint: crate::transport::Endpoint,
    tls: Option<crate::transport::TlsClient>,
    token: Option<String>,
}

impl SupervisorClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            spec_endpoint: crate::transport::Endpoint::Unix { path: socket_path.into() },
            tls: None,
            token: None,
        }
    }

    pub async fn request(&self, req: &CtlRequest) -> Result<CtlReply, ClientError> {
        // Preserve the "not running" nicety for the Unix case.
        if let crate::transport::Endpoint::Unix { path } = &self.spec_endpoint {
            if !path.exists() {
                return Err(ClientError::NotRunning(path.clone()));
            }
        }
        let spec = crate::transport::ConnectSpec {
            endpoint: self.spec_endpoint.clone(),
            tls: self.tls.clone(),
            token: self.token.clone(),
        };
        let conn = crate::transport::connect(&spec).await?;
        let (read_half, mut write_half) = tokio::io::split(conn);
        let mut body = serde_json::to_vec(req)?;
        body.push(b'\n');
        write_half.write_all(&body).await?;
        write_half.flush().await?;
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await;
        match read {
            Ok(Ok(0)) => Err(ClientError::Unexpected("daemon closed connection".into())),
            Ok(Ok(_)) => Ok(serde_json::from_str(line.trim_end())?),
            Ok(Err(e)) => Err(ClientError::Io(e)),
            Err(_) => Err(ClientError::Unexpected("daemon timeout".into())),
        }
    }
}
```

`socket_path(&self) -> &Path`: change to return the endpoint or keep a Unix-only accessor. Callers in `agents_cli.rs`/`worker.rs` use it for messages — replace with an `endpoint(&self) -> &Endpoint` accessor and update the (few) callers, OR keep `socket_path` returning the Unix path via `match`. Read the callers first; prefer adding `pub fn endpoint(&self) -> &crate::transport::Endpoint { &self.spec_endpoint }` and updating call sites' display strings to `%self.spec_endpoint` style. Keep `TlsClient` derive `Clone` (Task 2 already does).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-supervisor && cargo test -p caliban`
Expected: PASS — same count as the Step 1 baseline. Behavior is identical; only the socket plumbing moved behind the seam.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(supervisor): control server+client over transport seam (Unix mode) (#280)"
```

---

### Task 6: Rewire worker per-agent listener + attach client through the transport (Unix mode)

**Files:**
- Modify: `caliban/src/worker.rs` (per-agent listener + `serve_attach_client` + `write_line`)
- Modify: `caliban/src/agents_cli.rs` (`run_attach` over `transport::connect`)

**Interfaces:**
- Consumes: `caliban_supervisor::transport::{Listener, BindSpec, connect, ConnectSpec, Endpoint, BoxConn}`.
- Produces:
  - Worker binds its per-agent listener via `transport::Listener` from an `Endpoint` (Unix path for now — Task 7 makes it configurable to TCP).
  - `serve_attach_client(conn: BoxConn, hub, inbox, clients)`.
  - `run_attach(endpoint: &Endpoint, id: &str) -> i32`.

- [ ] **Step 1: Existing tests are the gate**

The worker attach path is covered by `caliban` integration tests (attach streaming, interactive inbound). Baseline them first.

Run: `cargo test -p caliban` → note passing count.

- [ ] **Step 2: Change the worker** (`worker.rs`)

- Replace the `UnixListener::bind(socket)` block (lines ~287–303) with:

```rust
let listen_endpoint = caliban_supervisor::transport::Endpoint::Unix { path: socket.to_path_buf() };
let bind = caliban_supervisor::transport::BindSpec { endpoint: listen_endpoint, tls: None, token: None };
let listener = match caliban_supervisor::transport::Listener::bind(&bind).await {
    Ok(l) => l,
    Err(e) => {
        eprintln!("[caliban __agent-worker] bind {} failed: {e}", socket.display());
        return 74;
    }
};
```

- Accept loop (lines ~347–359): `while let Ok(conn) = listener.accept().await { … tokio::spawn(serve_attach_client(conn, …)); }`.

- `serve_attach_client` signature → `conn: caliban_supervisor::transport::BoxConn`; split with `tokio::io::split(conn)` instead of `stream.into_split()`. The inbound task `read_inbound_frames(read_half, tx)` now takes a `ReadHalf<BoxConn>` — its body uses `AsyncBufReadExt`/`lines()` so it is already generic; adjust its parameter type to `R: AsyncRead + Unpin + Send` if it is currently typed to `OwnedReadHalf`. Read `read_inbound_frames` (lines ~202–229) and generalize its signature.

- `write_line` → generic over the writer:

```rust
async fn write_line<W: tokio::io::AsyncWrite + Unpin>(stream: &mut W, line: &str) -> std::io::Result<()> {
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await
}
```

- [ ] **Step 3: Change the attach client** (`agents_cli.rs`)

`run_attach` takes an `Endpoint`:

```rust
async fn run_attach(endpoint: &caliban_supervisor::transport::Endpoint, id: &str) -> i32 {
    let conn = match caliban_supervisor::transport::connect(
        &caliban_supervisor::transport::ConnectSpec { endpoint: endpoint.clone(), tls: None, token: None },
    ).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban: cannot attach to {id} at {endpoint:?} ({e}); the agent may have finished — try `caliban logs {id}`");
            return 74;
        }
    };
    eprintln!("caliban: attached to {id} (type to send · Ctrl+D end-of-input · Ctrl+C detach)");
    let (read_half, write_half) = tokio::io::split(conn);
    let send = tokio::spawn(crate::attach::stdin_to_frames(tokio::io::stdin(), write_half));
    let mut out = std::io::stdout();
    let code = tokio::select! {
        r = crate::attach::stream_attach(read_half, &mut out) => match r {
            Ok(()) => 0,
            Err(e) => { eprintln!("caliban: attach stream error: {e}"); 1 }
        },
        _ = tokio::signal::ctrl_c() => { eprintln!("\ncaliban: detached from {id}"); 0 }
    };
    send.abort();
    code
}
```

Update `run_attach`'s callers to pass the `Endpoint` from `AttachAck`/`Spawned` directly (drop the Task-4 temporary Unix-path extraction).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban`
Expected: PASS — same count as Step 1 baseline.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(caliban): worker listener + attach client over transport seam (Unix mode) (#280)"
```

---

### Task 7: Server-side network mode — `caliband` flags, TCP/TLS/token bind, per-agent TCP ports

Turn on the network path on the **server** side: the `caliband` binary gains config for a TCP listen address, TLS material, a token, an advertised host, and a per-agent port base; the `Supervisor` binds accordingly and assigns TCP endpoints to agents; the worker is launched to bind a TCP per-agent socket.

**Files:**
- Modify: `crates/caliban-supervisor/src/bin/caliband.rs` (flags/env → `BindSpec` + network config)
- Modify: `crates/caliban-supervisor/src/server.rs` (`Supervisor` carries a `BindSpec` + `NetworkConfig`; per-agent endpoint assignment)
- Modify: `crates/caliban-supervisor/src/proc.rs` (launcher passes `--socket <path>` for Unix or `--listen <addr>` for TCP)
- Modify: `caliban/src/worker.rs` (accept a `--listen tcp://host:port` arg → bind TCP per-agent socket; `--socket` stays for Unix)
- Test: `crates/caliban-supervisor/tests/network_transport.rs` (new)

**Interfaces:**
- Consumes: transport seam (Tasks 1–3), `Endpoint`, `AgentRecord::unix_socket_path`.
- Produces:
  - `Supervisor::with_bind_spec(bind: BindSpec, net: Option<NetworkConfig>, …)` or a builder — a way to construct a TCP-mode supervisor. Read the existing `new`/`with_launcher` constructors and extend them minimally (add an optional `NetworkConfig` field defaulting to `None` = Unix mode).
  - `pub struct NetworkConfig { pub advertise_host: String, pub agent_port_base: u16, pub tls: Option<TlsServer>, pub tls_client_for_workers: Option<TlsClient>, pub token: Option<String> }` — what the supervisor needs to assign TCP agent endpoints and to tell workers how to listen.
  - Env/flags on `caliband`: `--listen <addr>` / `CALIBAN_DAEMON_LISTEN` (e.g. `0.0.0.0:7070`; absent = Unix mode), `--advertise-host` / `CALIBAN_DAEMON_ADVERTISE_HOST` (DNS/host clients dial; default derived from `--listen` host), `--agent-port-base` / `CALIBAN_DAEMON_AGENT_PORT_BASE` (default `7100`), `--tls-cert`/`--tls-key`/`--tls-ca` / `CALIBAN_DAEMON_TLS_*` (PEM file paths), `--token` / `CALIBAN_DAEMON_TOKEN`.

**Per-agent port assignment (decision, documented here):** the supervisor owns the pod's network namespace, so it assigns each agent a distinct TCP port from a monotonic counter starting at `agent_port_base` (`AtomicU16`). The advertised per-agent `Endpoint::Tcp { addr }` is `"{advertise_host}:{port}"`; the worker is launched with `--listen 0.0.0.0:{port}`. This preserves today's "supervisor assigns the endpoint and returns it immediately from Spawn" property (no report-back round-trip). Monotonic assignment (not `base + index`) avoids colliding with a still-draining worker on the same slot. **Limitation to log & carry to QA:** a very long-lived daemon spawning >~64k agents would exhaust the counter; acceptable for the MVP (agent lifetimes are bounded; warm-pool reuse is P4/#-warmpool). This limit is called out in the ADR's "Revisit if" and must be surfaced in the whole-branch review.

- [ ] **Step 1: Write the failing test** (`tests/network_transport.rs`)

A control-plane-over-network test using the fake launcher (no LLM), asserting `status`/`spawn` work over TCP+TLS+token. Use the existing fake `WorkerLauncher` pattern from the crate's other integration tests (read `crates/caliban-supervisor/tests/` for the established harness + `with_launcher`/`with_signaller` usage; reuse it).

```rust
// tests/network_transport.rs
use std::sync::Arc;
use caliban_supervisor::transport::{tls_client_from_pem, tls_server_from_pem, ConnectSpec, Endpoint};
// … reuse the crate's test fakes for AgentStore + WorkerLauncher …

#[tokio::test]
async fn control_plane_over_tcp_tls_token() {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.key_pair.serialize_pem().into_bytes();

    // Build a TCP-mode supervisor bound to 127.0.0.1:0 with TLS + token,
    // using the fake launcher so no real worker runs.
    // (Construct via the new NetworkConfig-aware constructor; see Step 3.)
    let token = "tok-123".to_string();
    let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();
    // … build supervisor with BindSpec{ endpoint: Tcp("127.0.0.1:0"), tls: Some(tls_server), token: Some(token) } …
    // … spawn serve(); capture the bound addr via a small accessor or by binding a known port …

    // Client dials over TLS + token and calls Status.
    let tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
    let client = /* SupervisorClient with Tcp endpoint + tls_client + token */;
    let status = client.status().await.unwrap();
    assert!(status.uptime_secs < 5);
}
```

Because capturing the OS-assigned `:0` port requires an accessor, bind a fixed free port in the test (pick a high port, e.g. `127.0.0.1:53701`) to keep the test self-contained, and document that choice. If flakiness on a busy port is a concern, add a `Supervisor::bound_addr() -> Option<String>` accessor (set after `Listener::bind`) and use `:0`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-supervisor --test network_transport`
Expected: FAIL — no TCP-mode constructor / `SupervisorClient` TCP wiring yet.

- [ ] **Step 3: Implement server network mode**

- `caliband.rs`: parse the new flags/env (follow the existing `--repo-root`/`--socket-path` arg handling in the binary). When `--listen`/`CALIBAN_DAEMON_LISTEN` is set, load PEM files (if TLS flags given) via `tls_server_from_pem`, build `NetworkConfig`, and construct a TCP-mode `Supervisor` with `BindSpec { endpoint: Endpoint::Tcp{ addr }, tls, token }`. Otherwise keep today's Unix `BindSpec`.
- `server.rs`: add an `Option<NetworkConfig>` field + `AtomicU16` port counter. In `serve`, `Listener::bind(&self.bind_spec)`. In `dispatch` `Spawn`/`Respawn`, branch on mode:
  - Unix: today's behavior (assign `agent_runtime_dir.join("<uuid>-agent.sock")`, `Endpoint::Unix`, launch worker with `--socket <path>`).
  - TCP: `let port = base + counter.fetch_add(1, Relaxed)`; advertised `Endpoint::Tcp { addr: format!("{}:{}", net.advertise_host, port) }`; launch worker with `--listen 0.0.0.0:{port}` (+ TLS/token env for the worker's own per-agent listener — see note). Register that endpoint.
- `proc.rs`: `ExecWorkerLauncher::launch` currently passes `--socket <rec.unix_socket_path()>`. Add: when `rec.endpoint` is `Endpoint::Tcp`, pass `--listen 0.0.0.0:{port}` (parse the port from the advertised addr, or thread the local bind addr through the record/launcher). Cleaner: store the worker's *local* listen addr separately from the *advertised* endpoint — add a `launch_hint` the supervisor passes to the launcher. Read `proc.rs` and choose the minimal threading; document it.

**Per-agent TLS/token for the worker's own listener:** the worker binds its per-agent socket; in TCP mode that socket also needs TLS + token so prospero's attach is secured. The supervisor passes the same cert/key/token to the worker via env (`CALIBAN_AGENT_TLS_CERT/KEY`, `CALIBAN_AGENT_TOKEN`) or CLI. Keep it symmetric with the control listener. (Task 8 exercises this end-to-end.)

- `worker.rs`: accept `--listen tcp://0.0.0.0:port` (or `--listen 0.0.0.0:port`) as an alternative to `--socket <path>`; when present, build `Endpoint::Tcp` + load TLS/token from env and bind via `transport::Listener` with that `BindSpec`. The Unix `--socket` path stays the default.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p caliban-supervisor --test network_transport`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(supervisor): network (TCP/TLS/token) server mode + per-agent TCP ports (#280)"
```

---

### Task 8: Client-side network mode + per-agent-stream attach over the network (e2e)

The acceptance test: a real (fake-LLM) worker binds a **TCP+TLS+token** per-agent socket; a client attaches **over the network**, asserts `TurnEvent` NDJSON flows outbound and `AttachInbound` flows inbound.

**Files:**
- Modify: `caliban/src/agents_cli.rs` (build a `SupervisorClient` + attach `ConnectSpec` from network env: `CALIBAN_DAEMON_LISTEN`/`_TOKEN`/`_TLS_CA` on the client side)
- Modify: `crates/caliban-supervisor/src/client.rs` (add `SupervisorClient::new_network(endpoint, tls, token)` constructor)
- Test: `caliban/tests/attach_over_network.rs` (new)

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `SupervisorClient::new_network(endpoint: Endpoint, tls: Option<TlsClient>, token: Option<String>) -> Self`.
  - `run_attach` already takes an `Endpoint` (Task 6); extend its `ConnectSpec` to carry the client TLS + token when the endpoint is `Tcp` (thread them from the caller/env).

- [ ] **Step 1: Write the failing test** (`caliban/tests/attach_over_network.rs`)

Drive a worker to bind a TCP per-agent socket with TLS+token, then attach over the network. Reuse the worker test harness that other `caliban` attach tests use (read `caliban/tests/` for how they spawn `__agent-worker` / a fake). Skeleton:

```rust
use caliban_supervisor::transport::{connect, tls_client_from_pem, tls_server_from_pem, ConnectSpec, Endpoint};

#[tokio::test]
async fn attach_streams_turnevents_over_tcp_tls_token() {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.key_pair.serialize_pem().into_bytes();

    // Bind a TCP+TLS+token per-agent listener that emits two TurnEvents then EOFs,
    // mirroring serve_attach_client's outbound framing (reuse the worker path if a
    // test hook exists; otherwise a minimal server that writes two NDJSON TurnEvent lines).
    let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();
    // … bind Listener{ Tcp("127.0.0.1:0"), tls: Some(tls_server), token: Some("t") }, accept, write 2 events …

    // Client attaches over the network and reads them.
    let tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
    let conn = connect(&ConnectSpec { endpoint: Endpoint::Tcp { addr }, tls: Some(tls_client), token: Some("t".into()) }).await.unwrap();
    let (read_half, _write_half) = tokio::io::split(conn);
    let mut out: Vec<u8> = Vec::new();
    caliban::attach::stream_attach(read_half, &mut out).await.unwrap(); // if stream_attach is crate-private, assert via a thin test hook
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("hello world"));
    assert!(s.contains("done"));
}
```

Note: `stream_attach`/`AttachInbound` are `pub(crate)` in `caliban`. For an integration test under `caliban/tests/`, either (a) add a `#[doc(hidden)] pub` test-only re-export, or (b) put this test as a `#[cfg(test)]` unit test inside `src/attach.rs`/`src/worker.rs` where the items are visible. Prefer (b): a unit test in `worker.rs` that stands up the worker's own `serve_attach_client` over a TCP+TLS+token `Listener` and attaches via `transport::connect`, asserting the real outbound framing + a real inbound `AttachInbound::UserMessage` round-trip. This exercises the *actual* worker path, satisfying "integration test attaches to an agent over the network."

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban attach_over_network` (or the worker-module test name)
Expected: FAIL — client network constructor / TLS threading not present, or the worker path not yet reachable over TCP in the test.

- [ ] **Step 3: Implement**

- `client.rs`: add `new_network`. 
- `agents_cli.rs`: `ensure_daemon`/attach flows read client-side network env (`CALIBAN_DAEMON_LISTEN`, `CALIBAN_DAEMON_TOKEN`, `CALIBAN_DAEMON_TLS_CA` for the trust root + server name) and build a network `SupervisorClient`; when the daemon returns a `Tcp` attach endpoint, `run_attach` uses the client TLS+token. When no network env is set, everything stays Unix (unchanged).
- Make the test pass by standing up the real worker per-agent listener over TCP+TLS+token (per Step 1 option b).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p caliban` and `cargo test -p caliban-supervisor`
Expected: PASS — including the new network attach test and all prior tests.

- [ ] **Step 5: Full-gate + commit**

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
git add -A
git commit -m "feat(caliban): client network mode + per-agent attach over TCP+TLS+token; e2e test (#280)"
```

---

## Self-Review

**1. Spec coverage** (acceptance criteria of #280 + ADR 0051):
- "caliband serves control … over the network" → Tasks 5 + 7 (control server over `transport::Listener`, TCP/TLS/token mode). ✓
- "… + per-agent stream over the network" → Tasks 6 + 7 + 8 (worker per-agent listener over the seam; TCP mode; e2e attach). ✓
- "local Unix path unchanged" → Tasks 4–6 are Unix-only pure refactors keeping all existing tests green; network mode is opt-in via config (Task 7). ✓
- "integration test attaches to an agent over the network" → Task 8. ✓
- TLS → Task 2; bearer token → Task 3; NDJSON unchanged on the wire → Tasks 1/4 keep `serde_json` framing, token/TLS are sub-protocol. ✓
- gRPC excluded (→ #314). ✓

**2. Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N". A few tasks direct the implementer to *read* a specific file before editing (registry.rs signature, proc.rs launcher, existing test harness) — these are grounded reads of named symbols, not placeholders; the exact edits and target signatures are specified.

**3. Type consistency:** `Endpoint` (tag `scheme`, `Unix{path}`/`Tcp{addr}`) is used identically across proto (Task 4), server/client (Task 5), worker/attach (Task 6). `BoxConn`/`Conn`, `BindSpec`/`ConnectSpec`, `TlsServer`/`TlsClient`, `tls_server_from_pem`/`tls_client_from_pem`, `NetworkConfig` names are stable across tasks. `SupervisorClient::spawn -> (AgentId, Endpoint)` and `attach -> Endpoint` (Task 4) are consumed by Task 6's `run_attach(&Endpoint, ...)`.

**Carry-overs to flag in the whole-branch review (from the k8s epic memory):** (a) the monotonic per-agent port counter's 64k ceiling (Task 7); (b) prospero's `AgentHandle.socket` mirror must adopt the same `Endpoint` shape when prospero #64 (K8sFleet) lands — this ticket makes caliband's side transport-agnostic, closing the "generalize AgentHandle.socket" carry-over on the caliban side; (c) the store on-disk format for `AgentRecord` changed (`socket_path` → `endpoint`) — a daemon reading a pre-upgrade store will fail to deserialize; acceptable (runtime-dir scratch, wiped across versions) but must be noted in the PR.
