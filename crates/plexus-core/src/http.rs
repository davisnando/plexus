//! Local HTTP endpoint: `POST|GET|DELETE /mcp/{profile}` with a bearer token.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::FromRef,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::{
    hub::{Hub, HubEvent},
    server::HubServer,
};

type ProfileService = StreamableHttpService<HubServer, LocalSessionManager>;

#[derive(Clone)]
struct AppState {
    hub: Hub,
    services: Arc<Mutex<HashMap<String, (ProfileService, CancellationToken)>>>,
    ct: CancellationToken,
}

impl FromRef<AppState> for Hub {
    fn from_ref(state: &AppState) -> Hub {
        state.hub.clone()
    }
}

pub struct HttpServer {
    pub addr: SocketAddr,
    ct: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl HttpServer {
    pub async fn shutdown(self) {
        self.ct.cancel();
        let _ = self.task.await;
    }
}

/// Bind and serve the hub. Use port 0 for an ephemeral port (tests).
pub async fn serve(hub: Hub, port: u16) -> Result<HttpServer> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("could not bind 127.0.0.1:{port}"))?;
    let addr = listener.local_addr()?;
    hub.set_server_port(addr.port());
    let ct = CancellationToken::new();
    let state = AppState {
        hub: hub.clone(),
        services: Default::default(),
        ct: ct.clone(),
    };

    // Drop per-profile services (and their sessions) when profiles disappear.
    {
        let state = state.clone();
        let mut rx = hub.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = state.ct.cancelled() => break,
                    event = rx.recv() => match event {
                        Ok(HubEvent::ProfilesChanged) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let live: Vec<String> = state
                                .hub
                                .list_profiles()
                                .map(|ps| ps.into_iter().map(|p| p.id).collect())
                                .unwrap_or_default();
                            state.services.lock().unwrap().retain(|id, (_, token)| {
                                let keep = live.contains(id);
                                if !keep {
                                    token.cancel();
                                }
                                keep
                            });
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    },
                }
            }
        });
    }

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/mcp/{slug}", any(mcp))
        .route("/oauth/callback", get(crate::oauth::callback))
        .with_state(state);

    let task = tokio::spawn({
        let ct = ct.clone();
        async move {
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await
            {
                tracing::error!("http server stopped: {e}");
            }
        }
    });
    Ok(HttpServer { addr, ct, task })
}

async fn mcp(State(state): State<AppState>, Path(slug): Path<String>, req: Request) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let Some(profile_id) = state.hub.authenticate(&slug, token) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"invalid or missing bearer token for this profile"}"#,
        )
            .into_response();
    };
    let service = {
        let mut services = state.services.lock().unwrap();
        services
            .entry(profile_id.clone())
            .or_insert_with(|| {
                let token = state.ct.child_token();
                let hub = state.hub.clone();
                let config = StreamableHttpServerConfig::default()
                    .with_cancellation_token(token.clone())
                    // Non-browser MCP clients don't send Origin; any browser page does.
                    .enforce_origin_validation();
                let service = StreamableHttpService::new(
                    move || Ok(HubServer::new(hub.clone(), profile_id.clone())),
                    Arc::new(LocalSessionManager::default()),
                    config,
                );
                (service, token)
            })
            .0
            .clone()
    };
    match service.oneshot(req).await {
        Ok(response) => response.map(axum::body::Body::new),
        Err(never) => match never {},
    }
}
