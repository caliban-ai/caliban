//! Headless HTTP serve adapter — `caliban http serve` (ADR 0055 / #531).
//!
//! Lets a script or `curl` drive caliban over plain HTTP/JSON with no protocol
//! client, over the shared [`crate::serve::registry::DriveRegistry`]. Same
//! poll-based, cursor-driven shape as the MCP surface, with the `{v, event}`
//! envelope. This is where the [`crate::serve::auth::AuthGate`] bearer path
//! actually bites: loopback is open, a non-loopback peer must present a
//! `Authorization: Bearer <token>` matching `CALIBAN_DRIVE_TOKEN`, and a
//! non-loopback bind with no token configured is refused (fail closed).
//!
//! Endpoints:
//! - `POST /runs` `{ prompt, interactive? }` → `{ run_id }`
//! - `GET  /runs/:id/events?cursor=N` → `{ events: [{v,event}…], next_cursor, status, permission_request? }`
//! - `GET  /runs/:id/status` → `{ status }`
//! - `POST /runs/:id/input` `{ text?, end? }` → `{ ok }`
//! - `POST /runs/:id/permit` `{ tool_use_id, allow, reason? }` → `{ ok }`

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, extract::Request};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::serve::auth::{AuthGate, Peer};
use crate::serve::permissions::PermissionDecision;
use crate::serve::registry::{
    AgentFactory, DriveRegistry, PermitOutcome, RunSpec, SendInputError, build_prod_factory,
};

/// Wire schema version for the event envelope (matches the MCP surface).
const ENVELOPE_VERSION: u8 = 1;

/// Shared state behind the HTTP handlers.
#[derive(Clone)]
struct AppState {
    registry: DriveRegistry,
    factory: Arc<dyn AgentFactory>,
    auth: Arc<AuthGate>,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RunBody {
    prompt: String,
    #[serde(default)]
    interactive: bool,
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    cursor: usize,
}

#[derive(Deserialize)]
struct InputBody {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    end: bool,
}

#[derive(Deserialize)]
struct PermitBody {
    tool_use_id: String,
    allow: bool,
    #[serde(default)]
    reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Router + handlers
// ---------------------------------------------------------------------------

/// Build the HTTP router over `state`, with the shared auth gate applied to
/// every route.
fn router(state: AppState) -> Router {
    Router::new()
        .route("/runs", post(create_run))
        .route("/runs/:id/events", get(get_events))
        .route("/runs/:id/status", get(get_status))
        .route("/runs/:id/input", post(post_input))
        .route("/runs/:id/permit", post(post_permit))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state)
}

/// Reject unauthorized connections (ADR 0055 auth model): loopback is open; a
/// non-loopback peer must present a valid bearer token.
async fn auth_middleware(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let peer = Peer::from_addr(&addr);
    let presented = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match state.auth.authorize(peer, presented) {
        o if o.is_allowed() => next.run(request).await,
        _ => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
    }
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

async fn create_run(State(state): State<AppState>, Json(body): Json<RunBody>) -> Response {
    let built = match state.factory.build_run(&RunSpec {
        prompt: body.prompt,
        interactive: body.interactive,
    }) {
        Ok(b) => b,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to build run: {e}"),
            );
        }
    };
    let run_id = state.registry.spawn(built);
    Json(json!({ "run_id": run_id })).into_response()
}

async fn get_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Response {
    let Some(view) = state.registry.poll(&id, q.cursor) else {
        return error(StatusCode::NOT_FOUND, format!("unknown run_id: {id}"));
    };
    let events: Vec<Value> = view
        .events
        .iter()
        .map(|e| json!({ "v": ENVELOPE_VERSION, "event": e }))
        .collect();
    let permission = view.pending.as_ref().map(|p| {
        json!({
            "tool_use_id": p.tool_use_id,
            "tool_name": p.tool_name,
            "input": p.input,
        })
    });
    Json(json!({
        "events": events,
        "next_cursor": view.next_cursor,
        "status": view.status,
        "permission_request": permission,
    }))
    .into_response()
}

async fn get_status(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.registry.status(&id) {
        Some(status) => Json(json!({ "status": status })).into_response(),
        None => error(StatusCode::NOT_FOUND, format!("unknown run_id: {id}")),
    }
}

async fn post_input(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<InputBody>,
) -> Response {
    let inbound = if body.end {
        caliban_drive::DriveInbound::EndInput
    } else if let Some(text) = body.text {
        caliban_drive::DriveInbound::UserMessage { text }
    } else {
        return error(
            StatusCode::BAD_REQUEST,
            "input requires `text` or `end: true`",
        );
    };
    match state.registry.send_input(&id, inbound) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(SendInputError::UnknownRun) => {
            error(StatusCode::NOT_FOUND, format!("unknown run_id: {id}"))
        }
        Err(SendInputError::Ended(e)) => {
            error(StatusCode::CONFLICT, format!("cannot send input: {e}"))
        }
    }
}

async fn post_permit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PermitBody>,
) -> Response {
    let decision = if body.allow {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Deny(body.reason.unwrap_or_else(|| "denied by client".into()))
    };
    match state.registry.permit(&id, &body.tool_use_id, decision) {
        PermitOutcome::Answered => Json(json!({ "ok": true })).into_response(),
        PermitOutcome::UnknownRun => error(StatusCode::NOT_FOUND, format!("unknown run_id: {id}")),
        PermitOutcome::NoPending => error(
            StatusCode::CONFLICT,
            "no pending permission request for this run",
        ),
        PermitOutcome::Mismatch { expected } => error(
            StatusCode::CONFLICT,
            format!(
                "no pending permission with tool_use_id {}; current prompt is {expected}",
                body.tool_use_id
            ),
        ),
        PermitOutcome::RunGone => error(
            StatusCode::CONFLICT,
            "run is no longer waiting on that prompt",
        ),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Default bind address for `caliban http serve`.
pub(crate) const DEFAULT_ADDR: &str = "127.0.0.1:8730";

/// Serve caliban over HTTP (`caliban http serve`).
///
/// # Errors
///
/// Propagates address parsing, the fail-closed bind policy (a non-loopback bind
/// with no token), settings/provider construction, and bind/serve I/O errors.
pub(crate) async fn run_serve(args: &crate::args::Args, addr: &str) -> anyhow::Result<i32> {
    use anyhow::Context as _;

    let addr: SocketAddr = addr
        .parse()
        .with_context(|| format!("invalid --addr {addr}"))?;
    let auth = AuthGate::from_env();
    auth.check_bind(&addr)?;

    let settings = crate::startup::load_layered_settings(args, &std::env::current_dir()?)
        .map_err(|e| anyhow::anyhow!("failed to load settings: {e}"))?
        .settings;
    let helper_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(
        settings.api_key_helper.as_ref(),
    ));
    let provider = crate::startup::build_provider(args, &helper_pool)?;
    let factory = Arc::new(build_prod_factory(args, settings, provider)?);

    let state = AppState {
        registry: DriveRegistry::new(),
        factory,
        auth: Arc::new(auth),
    };
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(%addr, "caliban http serve listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("http server terminated with error")?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header::AUTHORIZATION};
    use caliban_agent_core::{Agent, ToolRegistry};
    use caliban_provider::{
        MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta,
        Usage,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt as _; // for `oneshot`

    use super::{AppState, RunSpec, router};
    use crate::serve::auth::AuthGate;
    use crate::serve::permissions::DriveAskHandler;
    use crate::serve::registry::{AgentFactory, BuiltRun, DriveRegistry};

    fn text_turn(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
        vec![
            Ok(StreamEvent::MessageStart {
                id: "m".into(),
                model: "mock-model".into(),
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Text,
            }),
            Ok(StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text(text.to_string()),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                usage_delta: Some(Usage::default()),
            }),
            Ok(StreamEvent::MessageStop),
        ]
    }

    struct MockFactory;
    impl AgentFactory for MockFactory {
        fn build_run(&self, spec: &RunSpec) -> anyhow::Result<BuiltRun> {
            let mp = Arc::new(MockProvider::new());
            mp.enqueue_stream(text_turn("hi"));
            let agent = Agent::builder()
                .provider(mp as Arc<dyn Provider + Send + Sync>)
                .tools(ToolRegistry::default())
                .model("mock-model")
                .max_tokens(64)
                .build()
                .expect("agent builds");
            let (_ask, perm_rx) = DriveAskHandler::pair();
            Ok(BuiltRun {
                agent: Arc::new(agent),
                messages: vec![caliban_provider::Message::user_text(spec.prompt.clone())],
                perm_rx,
                interactive: spec.interactive,
            })
        }
    }

    fn app(auth: AuthGate) -> axum::Router {
        router(AppState {
            registry: DriveRegistry::new(),
            factory: Arc::new(MockFactory),
            auth: Arc::new(auth),
        })
    }

    /// Send a request with a loopback peer and read the (status, JSON body).
    async fn send(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
        bearer: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        // ConnectInfo is injected by the make-service in production; in tests we
        // set the extension directly so the auth middleware sees a peer.
        let body = body.map_or(Body::empty(), |b| Body::from(b.to_string()));
        let mut request = req.body(body).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:5555".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let resp = app.clone().oneshot(request).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }

    async fn send_remote(app: &axum::Router, uri: &str, bearer: Option<&str>) -> StatusCode {
        let mut req = Request::builder().method("GET").uri(uri);
        if let Some(t) = bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        let mut request = req.body(Body::empty()).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "203.0.113.7:5555".parse::<std::net::SocketAddr>().unwrap(),
        ));
        app.clone().oneshot(request).await.unwrap().status()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_events_status_input_over_http() {
        let app = app(AuthGate::new(None));

        // run (interactive so input applies)
        let (st, body) = send(
            &app,
            "POST",
            "/runs",
            Some(json!({ "prompt": "hi", "interactive": true })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let run_id = body["run_id"].as_str().unwrap().to_string();

        // poll (stream) until awaiting_input
        let mut cursor = 0u64;
        let mut saw_turn_start = false;
        let mut awaited = false;
        for _ in 0..500 {
            let (_s, b) = send(
                &app,
                "GET",
                &format!("/runs/{run_id}/events?cursor={cursor}"),
                None,
                None,
            )
            .await;
            for ev in b["events"].as_array().unwrap() {
                assert_eq!(ev["v"], 1);
                if ev["event"]["type"] == "TurnStart" {
                    saw_turn_start = true;
                }
            }
            cursor = b["next_cursor"].as_u64().unwrap();
            if b["status"]["state"] == "awaiting_input" {
                awaited = true;
            }
            // Keep draining until we have both — the watch-based status can flip
            // to awaiting_input before the drainer task pushes the events.
            if awaited && saw_turn_start {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        assert!(saw_turn_start, "never saw TurnStart");
        assert!(awaited, "run never reached awaiting_input");

        // input: end
        let (st, b) = send(
            &app,
            "POST",
            &format!("/runs/{run_id}/input"),
            Some(json!({ "end": true })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(b["ok"], true);

        // drain to done
        let mut done = false;
        for _ in 0..500 {
            let (_s, b) = send(&app, "GET", &format!("/runs/{run_id}/status"), None, None).await;
            if b["status"]["state"] == "done" {
                done = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        assert!(done);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_run_is_404() {
        let app = app(AuthGate::new(None));
        let (st, _) = send(&app, "GET", "/runs/nope/status", None, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = send(&app, "GET", "/runs/nope/events", None, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = send(
            &app,
            "POST",
            "/runs/nope/input",
            Some(json!({ "end": true })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = send(
            &app,
            "POST",
            "/runs/nope/permit",
            Some(json!({ "tool_use_id": "x", "allow": true })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn input_requires_text_or_end() {
        let app = app(AuthGate::new(None));
        let (st, body) = send(&app, "POST", "/runs", Some(json!({ "prompt": "hi" })), None).await;
        let run_id = body["run_id"].as_str().unwrap().to_string();
        let (st2, _) = send(
            &app,
            "POST",
            &format!("/runs/{run_id}/input"),
            Some(json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(st2, StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_peer_requires_bearer_token() {
        // Token configured: a remote peer must present it.
        let app = app(AuthGate::new(Some("s3cret".into())));
        assert_eq!(
            send_remote(&app, "/runs/x/status", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send_remote(&app, "/runs/x/status", Some("wrong")).await,
            StatusCode::UNAUTHORIZED
        );
        // Correct token gets past auth (then 404 for the unknown run).
        assert_eq!(
            send_remote(&app, "/runs/x/status", Some("s3cret")).await,
            StatusCode::NOT_FOUND
        );
    }
}
