//! Control-plane-over-network integration test (#280 Task 7).
//!
//! The supervisor binds a **TCP** control listener secured with TLS + a
//! bearer token, and a client dials it over the same transport to run
//! `status` and `spawn`. Uses a fake launcher so no real worker process
//! runs — the assertion is on the *control plane* (endpoint assignment over
//! the network), not on a live agent.

use std::sync::Arc;
use std::time::Duration;

use caliban_supervisor::proto::{AgentRecord, AgentStatus, SpawnSpec};
use caliban_supervisor::store::AgentStore;
use caliban_supervisor::transport::{BindSpec, Endpoint, tls_client_from_pem, tls_server_from_pem};
use caliban_supervisor::{
    NetworkConfig, Supervisor, SupervisorClient, WorkerHandle, WorkerLauncher,
};

/// Fake launcher: a quick-exit child. In TCP mode the supervisor assigns a
/// TCP endpoint, so there is no per-agent socket file for the worker to
/// create — a bare `exit 0` is enough to exercise the control plane.
struct NopLauncher;

impl WorkerLauncher for NopLauncher {
    fn launch(&self, _record: &AgentRecord) -> std::io::Result<WorkerHandle> {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("exit 0");
        let child = cmd.spawn()?;
        let pid = child.id().expect("sh pid");
        Ok(WorkerHandle { pid, child })
    }
}

/// Launcher whose worker stays alive so the agent holds `Running` while the
/// test drives a status report. (`NopLauncher`'s child exits immediately and
/// would race the monitor task to a terminal state, which `report_status`
/// refuses.)
struct SleepLauncher;

impl WorkerLauncher for SleepLauncher {
    fn launch(&self, _record: &AgentRecord) -> std::io::Result<WorkerHandle> {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30");
        let child = cmd.spawn()?;
        let pid = child.id().expect("sh pid");
        Ok(WorkerHandle { pid, child })
    }
}

/// Poll `list()` until agent `id` reports `want`, or the bounded loop expires.
/// Returns the last-observed status (`None` if the agent never appeared).
async fn wait_for_status(
    client: &SupervisorClient,
    id: &str,
    want: AgentStatus,
) -> Option<AgentStatus> {
    let mut last = None;
    for _ in 0..200 {
        let recs = client.list().await.unwrap();
        if let Some(r) = recs.iter().find(|r| r.id == id) {
            last = Some(r.status);
            if r.status == want {
                return last;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    last
}

fn spec() -> SpawnSpec {
    SpawnSpec {
        label: Some("test".into()),
        frontmatter_path: None,
        initial_prompt: "hi".into(),
        model: None,
        provider: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: true,
        interactive: false,
        inherited_hooks_config: None,
        source: None,
    }
}

#[tokio::test]
async fn control_plane_over_tcp_tls_token() {
    // Self-signed cert for "localhost" — used as the server's identity and,
    // trusted as its own CA, as the client's root of trust.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.key_pair.serialize_pem().into_bytes();

    let token = "tok-123".to_string();
    let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let store = AgentStore::new(dir.path().join("data"));
    let agent_dir = dir.path().join("agents-rt");

    // Bind 127.0.0.1:0 so the OS picks a free port (no port-in-use flakiness);
    // capture the real port via `bound_addr()` once `serve` has bound.
    let bind = BindSpec {
        endpoint: Endpoint::Tcp {
            addr: "127.0.0.1:0".into(),
        },
        tls: Some(tls_server),
        token: Some(token.clone()),
    };
    let network = NetworkConfig {
        advertise_host: "localhost".into(),
        agent_port_base: 7100,
        agent_tls: None,
        agent_token: None,
    };
    let supervisor = Arc::new(Supervisor::with_bind(
        bind,
        Some(network),
        store,
        agent_dir,
        Arc::new(NopLauncher),
    ));
    let server = Arc::clone(&supervisor);
    let handle = tokio::spawn(async move { Arc::clone(&server).serve().await });

    // Wait for the OS-assigned port to become observable.
    let mut addr = None;
    for _ in 0..200 {
        if let Some(a) = supervisor.bound_addr() {
            addr = Some(a);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let addr = addr.expect("supervisor never published a bound TCP addr");

    // Client dials over TLS + token. server_name "localhost" matches the cert
    // SAN even though we connect to the 127.0.0.1 literal.
    let tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
    let client = SupervisorClient::new_tcp(addr, Some(tls_client), Some(token));

    // status() works over the network.
    let status = client.status().await.unwrap();
    assert!(status.uptime_secs < 5);
    match status.endpoint {
        Endpoint::Tcp { .. } => {}
        Endpoint::Unix { .. } => panic!("control endpoint should be TCP, got Unix"),
    }

    // spawn() returns a TCP agent endpoint using the advertise host + a port
    // at or above the configured base.
    let (id, endpoint) = client.spawn(spec()).await.unwrap();
    assert!(!id.is_empty());
    let Endpoint::Tcp { addr: agent_addr } = endpoint else {
        panic!("expected a TCP agent endpoint");
    };
    assert!(
        agent_addr.starts_with("localhost:"),
        "agent addr should use the advertise host, got {agent_addr}"
    );
    let port: u16 = agent_addr
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .expect("agent addr should end in a numeric port");
    assert!(port >= 7100, "agent port {port} should be >= base 7100");

    supervisor.cancel_token().cancel();
    let _ = handle.await;
}

/// #510 / #280 QA gap: the worker→daemon status path (`ReportStatus`) was wired
/// over TCP+TLS but never exercised — the coverage above stops at
/// `status()`/`spawn()`. An Idle report must actually traverse the TLS control
/// listener and flip the agent's status; if it is silently dropped (as it was
/// when the launcher never forwarded the control-plane CA) an interactive agent
/// is stranded in `Running` and can never be offered input — the user-visible
/// symptom in #510.
#[tokio::test]
async fn report_status_over_tcp_tls_flips_agent_to_idle() {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.key_pair.serialize_pem().into_bytes();
    let token = "tok-status".to_string();
    let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let store = AgentStore::new(dir.path().join("data"));
    let agent_dir = dir.path().join("agents-rt");

    let bind = BindSpec {
        endpoint: Endpoint::Tcp {
            addr: "127.0.0.1:0".into(),
        },
        tls: Some(tls_server),
        token: Some(token.clone()),
    };
    let network = NetworkConfig {
        advertise_host: "localhost".into(),
        agent_port_base: 7100,
        agent_tls: None,
        agent_token: None,
    };
    let supervisor = Arc::new(Supervisor::with_bind(
        bind,
        Some(network),
        store,
        agent_dir,
        Arc::new(SleepLauncher),
    ));
    let server = Arc::clone(&supervisor);
    let handle = tokio::spawn(async move { Arc::clone(&server).serve().await });

    let mut addr = None;
    for _ in 0..200 {
        if let Some(a) = supervisor.bound_addr() {
            addr = Some(a);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let addr = addr.expect("supervisor never published a bound TCP addr");

    let tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
    let client = SupervisorClient::new_tcp(addr, Some(tls_client), Some(token));

    // Spawn an agent; its sleeping worker holds the agent in `Running`.
    let (id, _endpoint) = client.spawn(spec()).await.unwrap();
    assert_eq!(
        wait_for_status(&client, &id, AgentStatus::Running).await,
        Some(AgentStatus::Running),
        "agent should reach Running once its worker is launched"
    );

    // The path under test: an Idle report dialed over the TLS control listener.
    client.report_status(&id, AgentStatus::Idle).await.unwrap();

    assert_eq!(
        wait_for_status(&client, &id, AgentStatus::Idle).await,
        Some(AgentStatus::Idle),
        "an Idle report must traverse the TLS control listener and flip the agent"
    );

    let _ = client.kill(&id).await; // stop the sleeping worker
    supervisor.cancel_token().cancel();
    let _ = handle.await;
}
