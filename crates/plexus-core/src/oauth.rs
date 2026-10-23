//! OAuth for remote MCP servers (MCP authorization spec: metadata discovery,
//! dynamic client registration, auth code + PKCE in the system browser).
//! Each connection keeps its own client registration and tokens in the
//! keychain under its `oauth` key, so two accounts of one service never mix.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, anyhow};
use axum::{
    extract::{Query, State},
    response::Html,
};
use rmcp::transport::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore,
    StoredCredentials, auth::OAuthState,
};
use serde::Deserialize;

use crate::{
    hub::Hub,
    model::Connection,
    secrets::{SecretStore, connection_key},
};

/// Keychain-backed `CredentialStore` for one connection.
pub struct KeychainCredentials {
    secrets: Arc<dyn SecretStore>,
    key: String,
}

impl KeychainCredentials {
    pub fn new(secrets: Arc<dyn SecretStore>, connection_id: &str) -> Self {
        Self {
            secrets,
            key: connection_key(connection_id, "oauth"),
        }
    }
}

fn store_error(e: impl std::fmt::Display) -> AuthError {
    AuthError::InternalError(format!("credential store: {e}"))
}

#[async_trait::async_trait]
impl CredentialStore for KeychainCredentials {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        match self.secrets.get(&self.key).map_err(store_error)? {
            Some(raw) => Ok(serde_json::from_str(&raw).ok()),
            None => Ok(None),
        }
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let raw = serde_json::to_string(&credentials).map_err(store_error)?;
        self.secrets.set(&self.key, &raw).map_err(store_error)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.secrets.delete(&self.key).map_err(store_error)
    }
}

/// Browser sign-ins in progress, keyed by the OAuth `state` parameter.
#[derive(Default)]
pub struct PendingFlows(tokio::sync::Mutex<HashMap<String, (String, OAuthState)>>);

fn url_of(connection: &Connection) -> Result<&str> {
    match &connection.transport {
        crate::model::Transport::Http { url, .. } => Ok(url),
        _ => Err(anyhow!("`{}` is not a remote connection", connection.alias)),
    }
}

/// HTTP client for an OAuth connection, refreshing tokens as needed. Errors
/// mention "needs authorization" when the user has to sign in again.
pub async fn authorized_client(
    hub: &Hub,
    connection: &Connection,
) -> Result<AuthClient<reqwest::Client>> {
    let mut manager = AuthorizationManager::new(url_of(connection)?)
        .await
        .context("OAuth setup")?;
    manager.set_credential_store(KeychainCredentials::new(
        hub.secrets().clone(),
        &connection.id,
    ));
    let has_tokens = manager
        .initialize_from_store()
        .await
        .map_err(|e| anyhow!("needs authorization ({e})"))?;
    if !has_tokens {
        return Err(anyhow!("needs authorization"));
    }
    // Fail fast (and as needs_auth) if the refresh token was revoked.
    manager
        .get_access_token()
        .await
        .map_err(|e| anyhow!("needs authorization ({e})"))?;
    Ok(AuthClient::new(reqwest::Client::default(), manager))
}

impl Hub {
    /// Begin a browser sign-in; returns the URL to open.
    pub async fn start_oauth(&self, connection_id: &str, redirect_uri: &str) -> Result<String> {
        let connection = self.connection(connection_id)?;
        let mut state = OAuthState::new(url_of(&connection)?, None)
            .await
            .context("OAuth setup")?;
        if let OAuthState::Unauthorized(manager) = &mut state {
            manager.set_credential_store(KeychainCredentials::new(
                self.secrets().clone(),
                connection_id,
            ));
        }
        state
            .start_authorization(
                AuthorizationRequest::new(redirect_uri).with_client_name("Plexus"),
            )
            .await
            .context("could not start sign-in (does this server support OAuth?)")?;
        let auth_url = state.get_authorization_url().await?;
        let csrf = reqwest::Url::parse(&auth_url)?
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .ok_or_else(|| anyhow!("authorization URL has no state parameter"))?;
        self.oauth_flows()
            .0
            .lock()
            .await
            .insert(csrf, (connection_id.to_string(), state));
        Ok(auth_url)
    }

    /// Finish a sign-in from the redirect; returns the connection name.
    /// `issuer` is the RFC 9207 `iss` parameter, required by servers that
    /// advertise it (e.g. Sentry).
    pub async fn finish_oauth(&self, code: &str, csrf: &str, issuer: Option<&str>) -> Result<String> {
        let (connection_id, mut state) = self
            .oauth_flows()
            .0
            .lock()
            .await
            .remove(csrf)
            .ok_or_else(|| anyhow!("this sign-in link expired; start again from Plexus"))?;
        state
            .handle_callback_with_issuer(code, csrf, issuer)
            .await
            .context("token exchange failed")?;
        // Tokens are persisted by the credential store during the exchange.
        let connection = self.connection(&connection_id)?;
        let hub = self.clone();
        tokio::spawn(async move {
            hub.stop_upstream(&connection_id).await;
            if let Err(e) = hub.refresh_capabilities(&connection_id).await {
                tracing::warn!("post sign-in discovery failed: {e:#}");
            }
        });
        Ok(connection.name)
    }

    pub async fn sign_out(&self, connection_id: &str) -> Result<()> {
        self.secrets()
            .delete(&connection_key(connection_id, "oauth"))?;
        self.stop_upstream(connection_id).await;
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Redirect target for the browser leg of the OAuth flow.
pub async fn callback(State(hub): State<Hub>, Query(q): Query<CallbackParams>) -> Html<String> {
    let outcome = match (q.code, q.state, q.error) {
        (_, _, Some(err)) => Err(q.error_description.unwrap_or(err)),
        (Some(code), Some(state), None) => hub
            .finish_oauth(&code, &state, q.iss.as_deref())
            .await
            .map_err(|e| format!("{e:#}")),
        _ => Err("missing code or state".into()),
    };
    let (title, body) = match outcome {
        Ok(name) => (
            "Signed in".to_string(),
            format!("Plexus is now connected to {}. You can close this tab.", escape(&name)),
        ),
        Err(e) => ("Sign-in failed".to_string(), escape(&e)),
    };
    Html(format!(
        r#"<!doctype html><meta charset="utf-8"><title>{title} · Plexus</title>
<style>body{{font:15px -apple-system,system-ui,sans-serif;display:grid;place-items:center;height:90vh;margin:0;background:#f3f2f8;color:#1c1a2e}}
@media(prefers-color-scheme:dark){{body{{background:#15131f;color:#eceaf6}}}}main{{max-width:28rem;text-align:center}}h1{{font-size:22px}}</style>
<main><h1>{title}</h1><p>{body}</p></main>"#
    ))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
