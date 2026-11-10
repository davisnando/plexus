//! Local HTTP endpoint: `POST|GET|DELETE /mcp` with a bearer token (and the
//! legacy `/mcp/{profile}` URLs as aliases), plus the OAuth routes remote
//! clients use through the tunnel.

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
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::{
    hub::{Hub, HubEvent},
    remote::{self, PublicBase},
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

    // Drop the endpoint's service (and its sessions) if the endpoint changes.
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
        .route("/mcp", any(mcp_root))
        .route("/mcp/{slug}", any(mcp_legacy))
        .route("/oauth/callback", get(crate::oauth::callback))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(remote::root_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp/{slug}",
            get(remote::resource_metadata),
        )
        // RFC 8414 path insertion, plus the OIDC and path-appended forms some clients try.
        .route(
            "/.well-known/oauth-authorization-server/auth/{slug}",
            get(remote::server_metadata),
        )
        .route(
            "/.well-known/openid-configuration/auth/{slug}",
            get(remote::server_metadata),
        )
        .route(
            "/auth/{slug}/.well-known/oauth-authorization-server",
            get(remote::server_metadata),
        )
        .route(
            "/auth/{slug}/.well-known/openid-configuration",
            get(remote::server_metadata),
        )
        .route("/auth/{slug}/register", post(remote::register))
        .route("/auth/{slug}/authorize", get(remote::authorize))
        .route("/auth/{slug}/authorize/wait", get(remote::authorize_wait))
        .route("/auth/{slug}/token", post(remote::token))
        .layer(middleware::from_fn_with_state(hub.clone(), guard))
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

/// Splits `host[:port]`, keeping IPv6 brackets: `[::1]:7420` → `[::1]`.
fn hostname(host: &str) -> &str {
    match host.rfind(':') {
        Some(i) if !host[i..].contains(']') => &host[..i],
        _ => host,
    }
}

/// Requests through the tunnel arrive from cloudflared on loopback, so they
/// are told apart by `Host` (the public hostname) and Cloudflare's `cf-ray`.
/// They may only reach the MCP and OAuth routes; everything else, such as
/// the upstream OAuth callback, stays local. Any other `Host` is refused,
/// which blocks DNS rebinding.
async fn guard(State(hub): State<Hub>, mut req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let via_cloudflare = req.headers().contains_key("cf-ray");
    let local = matches!(hostname(&host), "127.0.0.1" | "localhost" | "[::1]") && !via_cloudflare;
    let base = if local {
        format!("http://{host}")
    } else if hub.tunnel_host().is_some_and(|h| h == host) {
        let path = req.uri().path();
        let public = path == "/health"
            || path == "/mcp"
            || path.starts_with("/mcp/")
            || path.starts_with("/auth/")
            || path.starts_with("/.well-known/");
        if !public {
            return StatusCode::NOT_FOUND.into_response();
        }
        format!("https://{host}")
    } else {
        return (StatusCode::FORBIDDEN, "unexpected Host header").into_response();
    };
    req.extensions_mut().insert(PublicBase(base));
    next.run(req).await
}

async fn mcp_root(State(state): State<AppState>, req: Request) -> Response {
    mcp(state, None, req).await
}

async fn mcp_legacy(State(state): State<AppState>, Path(slug): Path<String>, req: Request) -> Response {
    mcp(state, Some(slug), req).await
}

async fn mcp(state: AppState, slug: Option<String>, req: Request) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let Some(profile_id) = state.hub.authenticate(slug.as_deref(), token) else {
        let mut response = (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"invalid or missing bearer token"}"#,
        )
            .into_response();
        // Points OAuth clients (claude.ai connectors) at the sign-in flow.
        if let Some(PublicBase(base)) = req.extensions().get::<PublicBase>()
            && slug.as_deref().is_none_or(|s| state.hub.profile_by_slug(s).is_some())
            && let Ok(value) = remote::www_authenticate(base, slug.as_deref()).parse()
        {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
        return response;
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
                    // `guard` checks Host, including the tunnel's public hostname.
                    .disable_allowed_hosts()
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
