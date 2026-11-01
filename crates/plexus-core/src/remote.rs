//! Remote access through the tunnel. Bearer-token clients use the profile
//! token as they do locally. Web connectors (claude.ai, ChatGPT) need OAuth,
//! so each profile is also an OAuth 2.1 authorization server with issuer
//! `{base}/auth/{slug}`: protected resource metadata (RFC 9728), server
//! metadata (RFC 8414), dynamic client registration (RFC 7591), and auth code
//! with PKCE. Consent happens in a popup on this Mac, never on the web page,
//! so reaching the public URL is not enough to get in.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::Result;
use axum::{
    Form, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    hub::{GrantRequest, Hub, HubEvent, generate_token},
    model::{Profile, RemoteGrant, now},
    oauth::{escape, html_page},
    store::{OAuthClient, RefreshToken},
};

const ACCESS_TTL: Duration = Duration::from_secs(3600);
const REFRESH_TTL_SECS: i64 = 90 * 86_400;
const CODE_TTL: Duration = Duration::from_secs(120);
const CONSENT_TIMEOUT: Duration = Duration::from_secs(300);
/// Pending consent prompts at once; more would let anyone with the URL spam popups.
const MAX_PENDING: usize = 3;
const MAX_UNUSED_CLIENTS: usize = 100;

/// Base URL of the server as the requester reached it: `https://….trycloudflare.com`
/// through the tunnel, `http://127.0.0.1:7420` locally. Set by the Host guard.
#[derive(Clone)]
pub(crate) struct PublicBase(pub String);

#[derive(Default)]
pub(crate) struct RemoteState {
    /// sha256(access token) → token
    access: Mutex<HashMap<String, AccessToken>>,
    codes: Mutex<HashMap<String, AuthCode>>,
    pending: Mutex<HashMap<String, PendingAuth>>,
}

struct AccessToken {
    grant_id: String,
    profile_id: String,
    expires: Instant,
}

struct AuthCode {
    profile_id: String,
    client_id: String,
    redirect_uri: String,
    challenge: String,
    expires: Instant,
}

struct PendingAuth {
    profile_id: String,
    client_id: String,
    redirect_uri: String,
    state: Option<String>,
    challenge: String,
    code: String,
    issuer: String,
    started: Instant,
    /// None while the popup is open.
    approved: Option<bool>,
}

impl RemoteState {
    pub(crate) fn check_access_token(&self, profile_id: &str, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let access = self.access.lock().unwrap();
        access
            .get(&hash(token))
            .is_some_and(|t| t.profile_id == profile_id && t.expires > Instant::now())
    }

    fn issue_access_token(&self, grant_id: &str, profile_id: &str) -> String {
        let token = generate_token();
        let mut access = self.access.lock().unwrap();
        let now = Instant::now();
        access.retain(|_, t| t.expires > now);
        access.insert(
            hash(&token),
            AccessToken {
                grant_id: grant_id.into(),
                profile_id: profile_id.into(),
                expires: now + ACCESS_TTL,
            },
        );
        token
    }

    fn revoke_grant(&self, grant_id: &str) {
        self.access
            .lock()
            .unwrap()
            .retain(|_, t| t.grant_id != grant_id);
    }

    pub(crate) fn clear_pending(&self) {
        self.pending.lock().unwrap().clear();
        self.codes.lock().unwrap().clear();
    }
}

fn hash(s: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(s.as_bytes()))
}

fn new_refresh() -> (String, RefreshToken) {
    let token = generate_token();
    let refresh = RefreshToken {
        hash: hash(&token),
        expires_at: now() + REFRESH_TTL_SECS,
    };
    (token, refresh)
}

impl Hub {
    pub fn remote_grants(&self, profile_id: &str) -> Result<Vec<RemoteGrant>> {
        self.store().list_grants(profile_id)
    }

    pub fn revoke_grant(&self, grant_id: &str) -> Result<()> {
        let profile_id = self.store().delete_grant(grant_id)?;
        self.remote().revoke_grant(grant_id);
        if let Some(profile_id) = profile_id {
            self.emit(HubEvent::GrantsChanged { profile_id });
        }
        Ok(())
    }
}

fn issuer(base: &str, slug: &str) -> String {
    format!("{base}/auth/{slug}")
}

/// `WWW-Authenticate` value pointing clients at the profile's resource metadata.
pub(crate) fn www_authenticate(base: &str, slug: &str) -> String {
    format!(
        r#"Bearer error="invalid_token", resource_metadata="{base}/.well-known/oauth-protected-resource/mcp/{slug}""#
    )
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

// ---- metadata ----

pub(crate) async fn resource_metadata(
    State(hub): State<Hub>,
    axum::Extension(PublicBase(base)): axum::Extension<PublicBase>,
    Path(slug): Path<String>,
) -> Response {
    let Some(profile) = hub.profile_by_slug(&slug) else {
        return not_found();
    };
    Json(json!({
        "resource": format!("{base}/mcp/{slug}"),
        "authorization_servers": [issuer(&base, &slug)],
        "bearer_methods_supported": ["header"],
        "resource_name": format!("Plexus · {}", profile.name),
    }))
    .into_response()
}

pub(crate) async fn server_metadata(
    State(hub): State<Hub>,
    axum::Extension(PublicBase(base)): axum::Extension<PublicBase>,
    Path(slug): Path<String>,
) -> Response {
    if hub.profile_by_slug(&slug).is_none() {
        return not_found();
    }
    let issuer = issuer(&base, &slug);
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "registration_endpoint": format!("{issuer}/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
        "authorization_response_iss_parameter_supported": true,
    }))
    .into_response()
}

// ---- dynamic client registration ----

#[derive(Deserialize)]
pub(crate) struct RegisterRequest {
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    token_endpoint_auth_method: Option<String>,
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// https anywhere, http only to loopback (native apps), or an app's own scheme.
fn acceptable_redirect(uri: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(uri) else {
        return false;
    };
    match url.scheme() {
        "https" => url.fragment().is_none(),
        "http" => matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        ),
        "javascript" | "data" | "file" | "vbscript" | "blob" => false,
        _ => true,
    }
}

pub(crate) async fn register(
    State(hub): State<Hub>,
    Path(slug): Path<String>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    if hub.profile_by_slug(&slug).is_none() {
        return not_found();
    }
    if req.redirect_uris.is_empty() || !req.redirect_uris.iter().all(|u| acceptable_redirect(u)) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect_uris must be https, loopback http, or an app scheme",
        );
    }
    let method = req
        .token_endpoint_auth_method
        .unwrap_or_else(|| "none".into());
    let secret = match method.as_str() {
        "none" => None,
        "client_secret_post" | "client_secret_basic" => Some(generate_token()),
        _ => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_client_metadata",
                "unsupported token_endpoint_auth_method",
            );
        }
    };
    let name: String = req
        .client_name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Unnamed client".into())
        .chars()
        .take(80)
        .collect();
    let client = OAuthClient {
        client_id: uuid::Uuid::new_v4().to_string(),
        name,
        redirect_uris: req.redirect_uris,
        secret_hash: secret.as_deref().map(hash),
    };
    if let Err(e) = hub
        .store()
        .prune_oauth_clients(MAX_UNUSED_CLIENTS)
        .and_then(|_| hub.store().insert_oauth_client(&client))
    {
        tracing::error!("client registration failed: {e:#}");
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "registration failed");
    }
    let mut body = json!({
        "client_id": client.client_id,
        "client_id_issued_at": now(),
        "client_name": client.name,
        "redirect_uris": client.redirect_uris,
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": method,
    });
    if let Some(secret) = secret {
        body["client_secret"] = json!(secret);
        body["client_secret_expires_at"] = json!(0);
    }
    (StatusCode::CREATED, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

// ---- authorization ----

#[derive(Deserialize)]
pub(crate) struct AuthorizeParams {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    resource: Option<String>,
}

fn error_page(status: StatusCode, title: &str, message: &str) -> Response {
    (status, Html(html_page(title, &escape(message), ""))).into_response()
}

/// Back to the client with `error=…` (only once the redirect URI is trusted).
fn redirect_error(redirect_uri: &str, error: &str, state: Option<&str>, issuer: &str) -> Response {
    redirect_with(redirect_uri, &[("error", error)], state, issuer)
}

fn redirect_with(redirect_uri: &str, params: &[(&str, &str)], state: Option<&str>, issuer: &str) -> Response {
    let Ok(mut url) = reqwest::Url::parse(redirect_uri) else {
        return error_page(StatusCode::BAD_REQUEST, "Sign-in failed", "Invalid redirect URI.");
    };
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
        if let Some(state) = state {
            q.append_pair("state", state);
        }
        q.append_pair("iss", issuer);
    }
    Redirect::to(url.as_str()).into_response()
}

pub(crate) async fn authorize(
    State(hub): State<Hub>,
    axum::Extension(PublicBase(base)): axum::Extension<PublicBase>,
    Path(slug): Path<String>,
    Query(q): Query<AuthorizeParams>,
) -> Response {
    let Some(profile) = hub.profile_by_slug(&slug) else {
        return error_page(StatusCode::NOT_FOUND, "Sign-in failed", "This Plexus profile does not exist.");
    };
    let client = match q.client_id.as_deref().map(|id| hub.store().oauth_client(id)) {
        Some(Ok(Some(c))) => c,
        _ => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Sign-in failed",
                "Unknown client. Remove the connector and add it again.",
            );
        }
    };
    let redirect_uri = match q.redirect_uri {
        Some(uri) if client.redirect_uris.contains(&uri) => uri,
        None if client.redirect_uris.len() == 1 => client.redirect_uris[0].clone(),
        _ => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Sign-in failed",
                "The redirect URI does not match the registered client.",
            );
        }
    };
    let issuer = issuer(&base, &slug);
    let state = q.state.as_deref();
    if q.response_type.as_deref() != Some("code") {
        return redirect_error(&redirect_uri, "unsupported_response_type", state, &issuer);
    }
    let challenge = match (q.code_challenge, q.code_challenge_method.as_deref()) {
        (Some(c), Some("S256")) if !c.is_empty() => c,
        _ => return redirect_error(&redirect_uri, "invalid_request", state, &issuer),
    };
    if let Some(resource) = &q.resource
        && !resource.trim_end_matches('/').ends_with(&format!("/mcp/{slug}"))
    {
        return redirect_error(&redirect_uri, "invalid_target", state, &issuer);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let code = format!("{:04}", rand::random_range(0..10_000));
    {
        let mut pending = hub.remote().pending.lock().unwrap();
        pending.retain(|_, p| p.started.elapsed() < CONSENT_TIMEOUT * 2);
        if pending.values().filter(|p| p.approved.is_none()).count() >= MAX_PENDING {
            return error_page(
                StatusCode::TOO_MANY_REQUESTS,
                "Try again shortly",
                "Other sign-in requests are waiting for approval in Plexus.",
            );
        }
        pending.insert(
            id.clone(),
            PendingAuth {
                profile_id: profile.id.clone(),
                client_id: client.client_id.clone(),
                redirect_uri: redirect_uri.clone(),
                state: q.state.clone(),
                challenge,
                code: code.clone(),
                issuer,
                started: Instant::now(),
                approved: None,
            },
        );
    }
    ask_for_consent(&hub, &profile, &client, &redirect_uri, id.clone(), code);
    Redirect::to(&format!("{base}/auth/{slug}/authorize/wait?id={id}")).into_response()
}

fn ask_for_consent(
    hub: &Hub,
    profile: &Profile,
    client: &OAuthClient,
    redirect_uri: &str,
    id: String,
    code: String,
) {
    let request = GrantRequest {
        id: id.clone(),
        profile_name: profile.name.clone(),
        client_name: client.name.clone(),
        redirect_host: reqwest::Url::parse(redirect_uri)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_else(|| redirect_uri.to_string()),
        code,
        timeout_secs: CONSENT_TIMEOUT.as_secs(),
    };
    let broker = hub.approval_broker();
    let hub = hub.clone();
    tokio::spawn(async move {
        let approved = match broker {
            Some(b) => tokio::time::timeout(CONSENT_TIMEOUT, b.request_grant(request))
                .await
                .unwrap_or(false),
            None => false,
        };
        if let Some(p) = hub.remote().pending.lock().unwrap().get_mut(&id) {
            p.approved = Some(approved);
        }
    });
}

#[derive(Deserialize)]
pub(crate) struct WaitParams {
    id: String,
}

pub(crate) async fn authorize_wait(State(hub): State<Hub>, Query(q): Query<WaitParams>) -> Response {
    let mut pending = hub.remote().pending.lock().unwrap();
    let Some(p) = pending.get(&q.id) else {
        return error_page(
            StatusCode::NOT_FOUND,
            "Sign-in expired",
            "This request expired. Start again from your client.",
        );
    };
    let Some(approved) = p.approved else {
        let body = format!(
            r#"Approve this request in the Plexus window on your Mac.<br>Check that it shows the code
<div style="font:600 34px ui-monospace,Menlo,monospace;letter-spacing:.2em;margin:18px 0">{}</div>
<small style="opacity:.6">This page continues by itself.</small>"#,
            escape(&p.code)
        );
        return Html(html_page("Waiting for approval", &body, r#"<meta http-equiv="refresh" content="2">"#))
            .into_response();
    };
    let p = pending.remove(&q.id).expect("present");
    drop(pending);
    if !approved {
        return redirect_error(&p.redirect_uri, "access_denied", p.state.as_deref(), &p.issuer);
    }
    let code = generate_token();
    hub.remote().codes.lock().unwrap().insert(
        code.clone(),
        AuthCode {
            profile_id: p.profile_id,
            client_id: p.client_id,
            redirect_uri: p.redirect_uri.clone(),
            challenge: p.challenge,
            expires: Instant::now() + CODE_TTL,
        },
    );
    redirect_with(&p.redirect_uri, &[("code", &code)], p.state.as_deref(), &p.issuer)
}

// ---- token endpoint ----

#[derive(Deserialize)]
pub(crate) struct TokenParams {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

/// Client credentials from HTTP Basic or the form body.
fn client_credentials(headers: &HeaderMap, form: &TokenParams) -> (Option<String>, Option<String>) {
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| {
            let (id, secret) = s.split_once(':')?;
            let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
            Some((decode(id), decode(secret)))
        });
    match basic {
        Some((id, secret)) => (Some(id), Some(secret)),
        None => (form.client_id.clone(), form.client_secret.clone()),
    }
}

fn token_response(access: String, refresh: String) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": ACCESS_TTL.as_secs(),
            "refresh_token": refresh,
        })),
    )
        .into_response()
}

pub(crate) async fn token(
    State(hub): State<Hub>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Form(form): Form<TokenParams>,
) -> Response {
    let Some(profile) = hub.profile_by_slug(&slug) else {
        return not_found();
    };
    let (client_id, secret) = client_credentials(&headers, &form);
    let client = match client_id.as_deref().map(|id| hub.store().oauth_client(id)) {
        Some(Ok(Some(c))) => c,
        _ => return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", "unknown client"),
    };
    if let Some(expected) = &client.secret_hash
        && secret.as_deref().map(hash).as_ref() != Some(expected)
    {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", "bad client secret");
    }
    let invalid_grant = |msg| oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", msg);

    match form.grant_type.as_str() {
        "authorization_code" => {
            let Some(code) = form
                .code
                .as_deref()
                .and_then(|c| hub.remote().codes.lock().unwrap().remove(c))
            else {
                return invalid_grant("unknown or used code");
            };
            let verifier = form.code_verifier.as_deref().unwrap_or_default();
            if code.expires < Instant::now()
                || code.client_id != client.client_id
                || code.profile_id != profile.id
                || form.redirect_uri.as_ref().is_some_and(|u| *u != code.redirect_uri)
            {
                return invalid_grant("code does not match this request");
            }
            let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(verifier.as_bytes()));
            if verifier.is_empty() || computed != code.challenge {
                return invalid_grant("PKCE verification failed");
            }
            let (refresh, stored) = new_refresh();
            let grant = RemoteGrant {
                id: uuid::Uuid::new_v4().to_string(),
                profile_id: profile.id.clone(),
                client_name: client.name.clone(),
                redirect_uri: code.redirect_uri,
                created_at: now(),
                last_used_at: now(),
            };
            if let Err(e) = hub.store().insert_grant(&grant, &client.client_id, &stored) {
                tracing::error!("saving grant failed: {e:#}");
                return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not save grant");
            }
            hub.emit(HubEvent::GrantsChanged {
                profile_id: profile.id.clone(),
            });
            let access = hub.remote().issue_access_token(&grant.id, &profile.id);
            token_response(access, refresh)
        }
        "refresh_token" => {
            let old_hash = hash(form.refresh_token.as_deref().unwrap_or_default());
            let Ok(Some((grant_id, profile_id, grant_client, expires_at))) =
                hub.store().grant_by_refresh(&old_hash)
            else {
                return invalid_grant("unknown refresh token");
            };
            if grant_client != client.client_id || profile_id != profile.id || expires_at < now() {
                return invalid_grant("refresh token does not match this request");
            }
            let (refresh, stored) = new_refresh();
            match hub.store().rotate_refresh(&grant_id, &old_hash, &stored) {
                Ok(true) => {}
                _ => return invalid_grant("refresh token was already used"),
            }
            let access = hub.remote().issue_access_token(&grant_id, &profile_id);
            token_response(access, refresh)
        }
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "use authorization_code or refresh_token",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::acceptable_redirect;

    #[test]
    fn redirect_uris() {
        assert!(acceptable_redirect("https://claude.ai/api/mcp/auth_callback"));
        assert!(acceptable_redirect("http://127.0.0.1:33418/callback"));
        assert!(acceptable_redirect("http://localhost/cb"));
        assert!(acceptable_redirect("cursor://anysphere.cursor-retrieval/oauth/callback"));
        assert!(!acceptable_redirect("http://evil.example/cb"));
        assert!(!acceptable_redirect("javascript:alert(1)"));
        assert!(!acceptable_redirect("not a url"));
    }
}
