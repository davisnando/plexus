use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock, Weak,
        atomic::{AtomicI64, AtomicU16, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures::future::BoxFuture;
use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, ContentBlock,
        Implementation, JsonObject, ReadResourceRequestParams, ReadResourceResult, Resource, Tool,
    },
    service::{NotificationContext, Peer, RunningService},
    transport::{
        StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::Serialize;
use tokio::{io::AsyncBufReadExt, sync::broadcast};

use crate::{
    model::*,
    naming,
    search::Bm25,
    secrets::{SecretStore, connection_key, profile_token_key},
    store::{CapabilityCache, Store},
};

const START_TIMEOUT: Duration = Duration::from_secs(30);
const STDERR_LINES: usize = 20;
const REAP_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubEvent {
    ConnectionStatus {
        connection_id: String,
        status: ConnectionStatus,
    },
    CapabilitiesChanged {
        connection_id: String,
    },
    ConnectionsChanged,
    ProfilesChanged,
    CallLogged {
        entry: CallLogEntry,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub profile_name: String,
    pub connection_name: String,
    pub connection_alias: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    /// Why the agent says it wants to make this call, if it gave a reason.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    AllowOnce,
    AlwaysAllow,
    Deny,
}

/// Asks the human whether a tool call may proceed. Implemented by the desktop
/// shell with a popup window; the hub applies the timeout.
pub trait ApprovalBroker: Send + Sync {
    fn request(&self, req: ApprovalRequest) -> BoxFuture<'static, ApprovalDecision>;
}

/// Opens a URL in the user's browser; returns whether that worked.
pub type UrlOpener = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What [`Hub::reconnect`] did.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ReconnectOutcome {
    Connected {
        tool_count: usize,
    },
    /// A browser sign-in was started; the user has to finish it.
    SignInStarted {
        url: String,
        opened: bool,
    },
    Failed {
        message: String,
    },
}

/// An upstream tool as exposed through a profile.
#[derive(Debug, Clone)]
pub struct ExposedTool {
    pub name: String,
    pub connection: Connection,
    pub tool: Tool,
    pub policy: ToolPolicy,
}

#[derive(Clone)]
pub struct Hub(Arc<Inner>);

struct Inner {
    store: Store,
    secrets: Arc<dyn SecretStore>,
    events: broadcast::Sender<HubEvent>,
    /// Fires with a profile id whenever that profile's tool/resource list may have changed.
    list_changed: broadcast::Sender<String>,
    caches: RwLock<HashMap<String, CapabilityCache>>,
    statuses: RwLock<HashMap<String, ConnectionStatus>>,
    upstreams: Mutex<HashMap<String, Arc<Upstream>>>,
    /// slug -> (profile id, token)
    tokens: RwLock<HashMap<String, (String, String)>>,
    approval: RwLock<Option<Arc<dyn ApprovalBroker>>>,
    url_opener: RwLock<Option<UrlOpener>>,
    /// Port the MCP endpoint is bound to (0 until `http::serve` runs).
    server_port: AtomicU16,
    legacy_peers: Mutex<Vec<(String, Peer<RoleServer>)>>,
    shell_path: Option<String>,
    oauth: crate::oauth::PendingFlows,
}

struct Upstream {
    service: tokio::sync::Mutex<Option<RunningService<RoleClient, UpstreamHandler>>>,
    last_used: AtomicI64,
    inflight: AtomicUsize,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl Upstream {
    fn new() -> Self {
        Self {
            service: tokio::sync::Mutex::new(None),
            last_used: AtomicI64::new(now()),
            inflight: AtomicUsize::new(0),
            stderr: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn stderr_tail(&self) -> String {
        let lines = self.stderr.lock().unwrap();
        lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

/// Decrements the in-flight counter and stamps last use on drop.
struct InflightGuard(Arc<Upstream>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.last_used.store(now(), Ordering::Relaxed);
        self.0.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct UpstreamHandler {
    hub: Weak<Inner>,
    connection_id: String,
}

impl UpstreamHandler {
    fn refresh_later(&self) {
        if let Some(inner) = self.hub.upgrade() {
            let hub = Hub(inner);
            let id = self.connection_id.clone();
            tokio::spawn(async move {
                if let Err(e) = hub.refresh_capabilities(&id).await {
                    tracing::warn!(connection = %id, "refresh after list_changed failed: {e:#}");
                }
            });
        }
    }
}

impl ClientHandler for UpstreamHandler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("plexus", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.refresh_later();
    }

    async fn on_resource_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.refresh_later();
    }
}

pub struct HubOptions {
    pub db_path: Option<PathBuf>,
    pub secrets: Arc<dyn SecretStore>,
    /// Resolve `PATH` from the user's login shell (needed for GUI apps on macOS).
    pub resolve_shell_path: bool,
}

impl Hub {
    /// Opens the store and starts background tasks. Must run inside a Tokio runtime.
    pub async fn new(opts: HubOptions) -> Result<Self> {
        let store = match &opts.db_path {
            Some(p) => Store::open(p)?,
            None => Store::open_in_memory()?,
        };
        let shell_path = if opts.resolve_shell_path {
            tokio::task::spawn_blocking(login_shell_path)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let caches = store.load_caches()?;
        let mut tokens = HashMap::new();
        for p in store.list_profiles()? {
            let key = profile_token_key(&p.id);
            let token = match opts.secrets.get(&key)? {
                Some(t) => t,
                None => {
                    let t = generate_token();
                    opts.secrets.set(&key, &t)?;
                    t
                }
            };
            tokens.insert(p.slug.clone(), (p.id.clone(), token));
        }
        let mut statuses = HashMap::new();
        for c in store.list_connections()? {
            statuses.insert(c.id.clone(), initial_status(&c));
        }
        let (events, _) = broadcast::channel(256);
        let (list_changed, _) = broadcast::channel(64);
        let hub = Hub(Arc::new(Inner {
            store,
            secrets: opts.secrets,
            events,
            list_changed,
            caches: RwLock::new(caches),
            statuses: RwLock::new(statuses),
            upstreams: Mutex::new(HashMap::new()),
            tokens: RwLock::new(tokens),
            approval: RwLock::new(None),
            url_opener: RwLock::new(None),
            server_port: AtomicU16::new(0),
            legacy_peers: Mutex::new(Vec::new()),
            shell_path,
            oauth: Default::default(),
        }));
        hub.spawn_background();
        Ok(hub)
    }

    fn spawn_background(&self) {
        // Idle reaper + log pruning.
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let mut last_prune = Instant::now() - Duration::from_secs(86_400);
            loop {
                tokio::time::sleep(REAP_INTERVAL).await;
                let Some(inner) = weak.upgrade() else { break };
                let hub = Hub(inner);
                hub.reap_idle().await;
                if last_prune.elapsed() > Duration::from_secs(3600) {
                    last_prune = Instant::now();
                    if let Ok(s) = hub.settings() {
                        let cutoff = now() - s.log_retention_days as i64 * 86_400;
                        let _ = hub.0.store.prune_calls(cutoff);
                    }
                }
            }
        });

        // Translate hub events into per-profile list_changed fan-out.
        let weak = Arc::downgrade(&self.0);
        let mut rx = self.0.events.subscribe();
        tokio::spawn(async move {
            loop {
                let event = match rx.recv().await {
                    Ok(e) => e,
                    Err(broadcast::error::RecvError::Lagged(_)) => HubEvent::ProfilesChanged,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(inner) = weak.upgrade() else { break };
                let hub = Hub(inner);
                let profiles = match hub.list_profiles() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let affected: Vec<String> = match &event {
                    HubEvent::CapabilitiesChanged { connection_id } => profiles
                        .iter()
                        .filter(|p| p.connection_ids.contains(connection_id))
                        .map(|p| p.id.clone())
                        .collect(),
                    HubEvent::ConnectionsChanged | HubEvent::ProfilesChanged => {
                        profiles.iter().map(|p| p.id.clone()).collect()
                    }
                    _ => continue,
                };
                hub.notify_list_changed(&affected).await;
            }
        });
    }

    async fn notify_list_changed(&self, profile_ids: &[String]) {
        for id in profile_ids {
            let _ = self.0.list_changed.send(id.clone());
        }
        let peers: Vec<Peer<RoleServer>> = {
            let mut peers = self.0.legacy_peers.lock().unwrap();
            peers.retain(|(_, p)| !p.is_transport_closed());
            peers
                .iter()
                .filter(|(pid, _)| profile_ids.contains(pid))
                .map(|(_, p)| p.clone())
                .collect()
        };
        for peer in peers {
            let _ = peer.notify_tool_list_changed().await;
            let _ = peer.notify_resource_list_changed().await;
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.0.events.subscribe()
    }

    pub(crate) fn subscribe_list_changed(&self) -> broadcast::Receiver<String> {
        self.0.list_changed.subscribe()
    }

    pub(crate) fn register_legacy_peer(&self, profile_id: String, peer: Peer<RoleServer>) {
        self.0.legacy_peers.lock().unwrap().push((profile_id, peer));
    }

    fn emit(&self, event: HubEvent) {
        let _ = self.0.events.send(event);
    }

    pub fn set_approval_broker(&self, broker: Arc<dyn ApprovalBroker>) {
        *self.0.approval.write().unwrap() = Some(broker);
    }

    pub fn set_url_opener(&self, opener: UrlOpener) {
        *self.0.url_opener.write().unwrap() = Some(opener);
    }

    fn open_url(&self, url: &str) -> bool {
        let opener = self.0.url_opener.read().unwrap().clone();
        opener.is_some_and(|open| open(url))
    }

    pub(crate) fn set_server_port(&self, port: u16) {
        self.0.server_port.store(port, Ordering::Relaxed);
    }

    // ---- settings ----

    pub fn settings(&self) -> Result<Settings> {
        self.0.store.settings()
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        self.0.store.save_settings(s)
    }

    // ---- connections ----

    pub fn list_connections(&self) -> Result<Vec<Connection>> {
        self.0.store.list_connections()
    }

    pub fn connection(&self, id: &str) -> Result<Connection> {
        self.0
            .store
            .get_connection(id)?
            .ok_or_else(|| anyhow!("connection {id} not found"))
    }

    pub fn status(&self, connection_id: &str) -> ConnectionStatus {
        self.0
            .statuses
            .read()
            .unwrap()
            .get(connection_id)
            .cloned()
            .unwrap_or(ConnectionStatus::Idle)
    }

    pub fn statuses(&self) -> HashMap<String, ConnectionStatus> {
        self.0.statuses.read().unwrap().clone()
    }

    fn set_status(&self, connection_id: &str, status: ConnectionStatus) {
        let changed = {
            let mut statuses = self.0.statuses.write().unwrap();
            statuses.insert(connection_id.to_string(), status.clone()) != Some(status.clone())
        };
        if changed {
            self.emit(HubEvent::ConnectionStatus {
                connection_id: connection_id.to_string(),
                status,
            });
        }
    }

    pub fn tools(&self, connection_id: &str) -> Vec<Tool> {
        self.0
            .caches
            .read()
            .unwrap()
            .get(connection_id)
            .map(|c| c.tools.clone())
            .unwrap_or_default()
    }

    /// Which of the transport's secret keys currently have a value (never the values).
    pub fn secrets_present(&self, connection: &Connection) -> Result<Vec<String>> {
        let mut present = vec![];
        for key in connection.transport.secret_keys() {
            if self
                .0
                .secrets
                .get(&connection_key(&connection.id, &key))?
                .is_some()
            {
                present.push(key);
            }
        }
        Ok(present)
    }

    pub async fn create_connection(&self, input: ConnectionInput) -> Result<Connection> {
        naming::validate_alias(&input.alias).map_err(|e| anyhow!(e))?;
        self.ensure_alias_free(&input.alias, None)?;
        let idle = match input.idle_timeout_secs {
            Some(s) => s,
            None => self.settings()?.default_idle_timeout_secs,
        };
        let connection = Connection {
            id: uuid::Uuid::new_v4().to_string(),
            alias: input.alias,
            name: input.name,
            template_id: input.template_id,
            transport: input.transport,
            enabled: input.enabled,
            idle_timeout_secs: idle,
            created_at: now(),
        };
        self.write_secrets(&connection, &input.secrets)?;
        self.0.store.insert_connection(&connection)?;
        self.set_status(&connection.id, initial_status(&connection));
        self.emit(HubEvent::ConnectionsChanged);
        Ok(connection)
    }

    pub async fn update_connection(&self, id: &str, input: ConnectionInput) -> Result<Connection> {
        naming::validate_alias(&input.alias).map_err(|e| anyhow!(e))?;
        self.ensure_alias_free(&input.alias, Some(id))?;
        let old = self.connection(id)?;
        let connection = Connection {
            id: old.id.clone(),
            alias: input.alias,
            name: input.name,
            template_id: input.template_id,
            transport: input.transport,
            enabled: input.enabled,
            idle_timeout_secs: input.idle_timeout_secs.unwrap_or(old.idle_timeout_secs),
            created_at: old.created_at,
        };
        self.write_secrets(&connection, &input.secrets)?;
        for stale in old.transport.secret_keys() {
            if !connection.transport.secret_keys().contains(&stale) {
                self.0.secrets.delete(&connection_key(id, &stale))?;
            }
        }
        self.0.store.update_connection(&connection)?;
        if old.transport != connection.transport || !input.secrets.is_empty() || !connection.enabled
        {
            self.stop_upstream(id).await;
        }
        if old.transport != connection.transport {
            // Force rediscovery with the new configuration.
            let cache = CapabilityCache::default();
            self.0.store.save_cache(id, &cache)?;
            self.0.caches.write().unwrap().insert(id.to_string(), cache);
        }
        self.set_status(id, initial_status(&connection));
        self.emit(HubEvent::ConnectionsChanged);
        Ok(connection)
    }

    pub async fn delete_connection(&self, id: &str) -> Result<()> {
        let connection = self.connection(id)?;
        self.stop_upstream(id).await;
        self.0.upstreams.lock().unwrap().remove(id);
        for key in connection.transport.secret_keys() {
            self.0.secrets.delete(&connection_key(id, &key))?;
        }
        self.0.store.delete_connection(id)?;
        self.0.caches.write().unwrap().remove(id);
        self.0.statuses.write().unwrap().remove(id);
        self.emit(HubEvent::ConnectionsChanged);
        Ok(())
    }

    fn ensure_alias_free(&self, alias: &str, except: Option<&str>) -> Result<()> {
        let taken = self
            .list_connections()?
            .iter()
            .any(|c| c.alias == alias && Some(c.id.as_str()) != except);
        if taken {
            bail!("alias `{alias}` is already used by another connection");
        }
        Ok(())
    }

    fn write_secrets(
        &self,
        connection: &Connection,
        secrets: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let allowed = connection.transport.secret_keys();
        for (key, value) in secrets {
            if !allowed.contains(key) {
                bail!("secret `{key}` is not used by this connection");
            }
            self.0
                .secrets
                .set(&connection_key(&connection.id, key), value)?;
        }
        Ok(())
    }

    fn secret(&self, connection_id: &str, key: &str) -> Result<Option<String>> {
        self.0.secrets.get(&connection_key(connection_id, key))
    }

    /// Restart the upstream and re-read its tools. Returns the tool count.
    pub async fn test_connection(&self, id: &str) -> Result<usize> {
        self.stop_upstream(id).await;
        self.refresh_capabilities(id).await?;
        Ok(self.tools(id).len())
    }

    /// Get a connection working again on behalf of an agent: restart it, and
    /// if the server wants a sign-in, start the browser OAuth flow.
    pub async fn reconnect(&self, id: &str) -> Result<ReconnectOutcome> {
        let connection = self.connection(id)?;
        if !connection.enabled {
            return Ok(ReconnectOutcome::Failed {
                message: format!(
                    "`{}` is disabled; the user can enable it in Plexus.",
                    connection.alias
                ),
            });
        }
        if self.status(id) != ConnectionStatus::NeedsAuth {
            match self.test_connection(id).await {
                Ok(tool_count) => return Ok(ReconnectOutcome::Connected { tool_count }),
                Err(e) if self.status(id) != ConnectionStatus::NeedsAuth => {
                    return Ok(ReconnectOutcome::Failed {
                        message: format!("{e:#}"),
                    });
                }
                Err(_) => {}
            }
        }
        let Transport::Http { auth, .. } = &connection.transport else {
            return Ok(ReconnectOutcome::Failed {
                message:
                    "the server rejected its credentials; the user has to update them in Plexus."
                        .into(),
            });
        };
        match auth {
            HttpAuth::Oauth => {}
            // A remote answering 401 without auth configured: switch it to OAuth,
            // as the dashboard's "Sign in" button does.
            HttpAuth::None => {
                let mut transport = connection.transport.clone();
                if let Transport::Http { auth, .. } = &mut transport {
                    *auth = HttpAuth::Oauth;
                }
                self.update_connection(
                    id,
                    ConnectionInput {
                        alias: connection.alias.clone(),
                        name: connection.name.clone(),
                        template_id: connection.template_id.clone(),
                        transport,
                        enabled: true,
                        idle_timeout_secs: Some(connection.idle_timeout_secs),
                        secrets: Default::default(),
                    },
                )
                .await?;
            }
            HttpAuth::Bearer => {
                return Ok(ReconnectOutcome::Failed {
                    message:
                        "the server rejected the bearer token; the user has to update it in Plexus."
                            .into(),
                });
            }
        }
        let port = self.0.server_port.load(Ordering::Relaxed);
        if port == 0 {
            bail!("the Plexus server is not running");
        }
        let url = self
            .start_oauth(id, &format!("http://127.0.0.1:{port}/oauth/callback"))
            .await?;
        let opened = self.open_url(&url);
        Ok(ReconnectOutcome::SignInStarted { url, opened })
    }

    // ---- upstream lifecycle ----

    fn upstream(&self, id: &str) -> Arc<Upstream> {
        self.0
            .upstreams
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Upstream::new()))
            .clone()
    }

    /// Returns a peer for the connection, starting it if needed.
    async fn peer(&self, connection: &Connection) -> Result<(Peer<RoleClient>, Arc<Upstream>)> {
        if !connection.enabled {
            bail!("connection `{}` is disabled", connection.alias);
        }
        let upstream = self.upstream(&connection.id);
        let mut guard = upstream.service.lock().await;
        if let Some(service) = guard.as_ref() {
            if !service.is_closed() {
                return Ok((service.peer().clone(), upstream.clone()));
            }
        }
        self.set_status(&connection.id, ConnectionStatus::Starting);
        let started = tokio::time::timeout(START_TIMEOUT, self.start(connection, &upstream)).await;
        let started = match started {
            Ok(r) => r,
            Err(_) => Err(anyhow!("timed out after {}s", START_TIMEOUT.as_secs())),
        };
        match started {
            Ok(service) => {
                let peer = service.peer().clone();
                *guard = Some(service);
                upstream.last_used.store(now(), Ordering::Relaxed);
                self.set_status(&connection.id, ConnectionStatus::Running);
                Ok((peer, upstream.clone()))
            }
            Err(e) => {
                let tail = upstream.stderr_tail();
                let message = if tail.is_empty() {
                    format!("{e:#}")
                } else {
                    format!("{e:#}\n{tail}")
                };
                let status = if is_auth_error(&message) {
                    ConnectionStatus::NeedsAuth
                } else {
                    ConnectionStatus::Error {
                        message: message.clone(),
                    }
                };
                self.set_status(&connection.id, status);
                Err(anyhow!(message))
            }
        }
    }

    async fn start(
        &self,
        connection: &Connection,
        upstream: &Upstream,
    ) -> Result<RunningService<RoleClient, UpstreamHandler>> {
        let handler = UpstreamHandler {
            hub: Arc::downgrade(&self.0),
            connection_id: connection.id.clone(),
        };
        match &connection.transport {
            Transport::Stdio {
                command,
                args,
                env,
                secret_env,
            } => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args);
                if let Some(path) = &self.0.shell_path {
                    cmd.env("PATH", path);
                }
                if let Some(home) = std::env::var_os("HOME") {
                    cmd.current_dir(home);
                }
                cmd.envs(env);
                for key in secret_env {
                    let value = self
                        .secret(&connection.id, &format!("env:{key}"))?
                        .ok_or_else(|| anyhow!("secret env `{key}` is not set"))?;
                    cmd.env(key, value);
                }
                upstream.stderr.lock().unwrap().clear();
                let (transport, stderr) = TokioChildProcess::builder(cmd)
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .with_context(|| format!("failed to spawn `{command}`"))?;
                if let Some(stderr) = stderr {
                    let buffer = upstream.stderr.clone();
                    tokio::spawn(async move {
                        let mut lines = tokio::io::BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let mut b = buffer.lock().unwrap();
                            if b.len() == STDERR_LINES {
                                b.pop_front();
                            }
                            b.push_back(line);
                        }
                    });
                }
                Ok(handler.serve(transport).await?)
            }
            Transport::Http {
                url,
                auth,
                headers,
                secret_headers,
            } => {
                let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
                let mut all_headers: Vec<(String, String)> = headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for name in secret_headers {
                    let value = self
                        .secret(&connection.id, &format!("header:{name}"))?
                        .ok_or_else(|| anyhow!("secret header `{name}` is not set"))?;
                    all_headers.push((name.clone(), value));
                }
                for (k, v) in all_headers {
                    config.custom_headers.insert(
                        http::HeaderName::try_from(k.as_str())
                            .with_context(|| format!("invalid header name `{k}`"))?,
                        http::HeaderValue::try_from(v.as_str())
                            .with_context(|| format!("invalid value for header `{k}`"))?,
                    );
                }
                match auth {
                    HttpAuth::None => {}
                    HttpAuth::Bearer => {
                        let token = self
                            .secret(&connection.id, "bearer")?
                            .ok_or_else(|| anyhow!("bearer token is not set"))?;
                        config = config.auth_header(token);
                    }
                    HttpAuth::Oauth => {
                        let client = crate::oauth::authorized_client(self, connection).await?;
                        let transport = StreamableHttpClientTransport::with_client(client, config);
                        return Ok(handler.serve(transport).await?);
                    }
                }
                let transport = StreamableHttpClientTransport::from_config(config);
                Ok(handler.serve(transport).await?)
            }
        }
    }

    pub async fn stop_upstream(&self, id: &str) {
        let upstream = self.0.upstreams.lock().unwrap().get(id).cloned();
        if let Some(upstream) = upstream {
            if let Some(mut service) = upstream.service.lock().await.take() {
                let _ = service.close_with_timeout(Duration::from_secs(3)).await;
            }
        }
        if let Ok(c) = self.connection(id) {
            self.set_status(id, initial_status(&c));
        }
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.0.upstreams.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop_upstream(&id).await;
        }
    }

    async fn reap_idle(&self) {
        let Ok(connections) = self.list_connections() else {
            return;
        };
        for c in connections {
            let upstream = self.0.upstreams.lock().unwrap().get(&c.id).cloned();
            let Some(upstream) = upstream else { continue };
            let idle_for = now() - upstream.last_used.load(Ordering::Relaxed);
            let running = matches!(self.status(&c.id), ConnectionStatus::Running);
            if running
                && upstream.inflight.load(Ordering::Relaxed) == 0
                && idle_for >= c.idle_timeout_secs as i64
            {
                tracing::info!(connection = %c.alias, "stopping idle upstream");
                self.stop_upstream(&c.id).await;
            }
        }
    }

    /// Start (if needed) and re-read the connection's tools and resources.
    pub async fn refresh_capabilities(&self, id: &str) -> Result<()> {
        let connection = self.connection(id)?;
        let (peer, upstream) = self.peer(&connection).await?;
        upstream.inflight.fetch_add(1, Ordering::Relaxed);
        let _guard = InflightGuard(upstream);
        let tools = peer.list_all_tools().await.context("tools/list")?;
        let has_resources = peer
            .peer_info()
            .map(|i| i.capabilities.resources.is_some())
            .unwrap_or(false);
        let resources = if has_resources {
            peer.list_all_resources().await.unwrap_or_default()
        } else {
            vec![]
        };
        let cache = CapabilityCache {
            tools,
            resources,
            cached_at: Some(now()),
        };
        self.0.store.save_cache(id, &cache)?;
        let changed = {
            let mut caches = self.0.caches.write().unwrap();
            let old = caches.insert(id.to_string(), cache.clone());
            old.map(|o| {
                serde_json::to_value(&o.tools).ok() != serde_json::to_value(&cache.tools).ok()
                    || serde_json::to_value(&o.resources).ok()
                        != serde_json::to_value(&cache.resources).ok()
            })
            .unwrap_or(true)
        };
        if changed {
            self.emit(HubEvent::CapabilitiesChanged {
                connection_id: id.to_string(),
            });
        }
        Ok(())
    }

    // ---- profiles ----

    pub fn list_profiles(&self) -> Result<Vec<Profile>> {
        self.0.store.list_profiles()
    }

    pub fn profile(&self, id: &str) -> Result<Profile> {
        self.list_profiles()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow!("profile {id} not found"))
    }

    pub fn create_profile(&self, input: ProfileInput) -> Result<Profile> {
        naming::validate_slug(&input.slug).map_err(|e| anyhow!(e))?;
        if self.list_profiles()?.iter().any(|p| p.slug == input.slug) {
            bail!("a profile with slug `{}` already exists", input.slug);
        }
        let profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            slug: input.slug,
            name: input.name,
            mode: input.mode,
            connection_ids: input.connection_ids,
            created_at: now(),
        };
        self.0.store.upsert_profile(&profile)?;
        let token = generate_token();
        self.0
            .secrets
            .set(&profile_token_key(&profile.id), &token)?;
        self.0
            .tokens
            .write()
            .unwrap()
            .insert(profile.slug.clone(), (profile.id.clone(), token));
        self.emit(HubEvent::ProfilesChanged);
        Ok(profile)
    }

    pub fn update_profile(&self, id: &str, input: ProfileInput) -> Result<Profile> {
        naming::validate_slug(&input.slug).map_err(|e| anyhow!(e))?;
        let old = self.profile(id)?;
        if self
            .list_profiles()?
            .iter()
            .any(|p| p.slug == input.slug && p.id != id)
        {
            bail!("a profile with slug `{}` already exists", input.slug);
        }
        let profile = Profile {
            id: old.id.clone(),
            slug: input.slug,
            name: input.name,
            mode: input.mode,
            connection_ids: input.connection_ids,
            created_at: old.created_at,
        };
        self.0.store.upsert_profile(&profile)?;
        {
            let mut tokens = self.0.tokens.write().unwrap();
            if let Some(entry) = tokens.remove(&old.slug) {
                tokens.insert(profile.slug.clone(), entry);
            }
        }
        self.emit(HubEvent::ProfilesChanged);
        Ok(profile)
    }

    pub fn delete_profile(&self, id: &str) -> Result<()> {
        let profile = self.profile(id)?;
        self.0.store.delete_profile(id)?;
        self.0.secrets.delete(&profile_token_key(id))?;
        self.0.tokens.write().unwrap().remove(&profile.slug);
        self.emit(HubEvent::ProfilesChanged);
        Ok(())
    }

    pub fn profile_token(&self, id: &str) -> Result<String> {
        let profile = self.profile(id)?;
        self.0
            .tokens
            .read()
            .unwrap()
            .get(&profile.slug)
            .map(|(_, t)| t.clone())
            .ok_or_else(|| anyhow!("no token for profile"))
    }

    pub fn rotate_profile_token(&self, id: &str) -> Result<String> {
        let profile = self.profile(id)?;
        let token = generate_token();
        self.0.secrets.set(&profile_token_key(id), &token)?;
        self.0
            .tokens
            .write()
            .unwrap()
            .insert(profile.slug, (profile.id, token.clone()));
        Ok(token)
    }

    /// Constant-time token check; returns the profile id on success.
    pub fn authenticate(&self, slug: &str, token: &str) -> Option<String> {
        use subtle::ConstantTimeEq;
        let tokens = self.0.tokens.read().unwrap();
        let (id, expected) = tokens.get(slug)?;
        bool::from(expected.as_bytes().ct_eq(token.as_bytes())).then(|| id.clone())
    }

    pub fn tool_policies(&self, profile_id: &str) -> Result<HashMap<(String, String), ToolPolicy>> {
        self.0.store.tool_policies(profile_id)
    }

    pub fn set_tool_policy(
        &self,
        profile_id: &str,
        connection_id: &str,
        tool: &str,
        policy: ToolPolicy,
    ) -> Result<()> {
        self.0
            .store
            .set_tool_policy(profile_id, connection_id, tool, policy)?;
        let _ = self.0.list_changed.send(profile_id.to_string());
        Ok(())
    }

    // ---- proxying ----

    fn profile_connections(&self, profile: &Profile) -> Result<Vec<Connection>> {
        let all = self.list_connections()?;
        Ok(all
            .into_iter()
            .filter(|c| c.enabled && profile.connection_ids.contains(&c.id))
            .collect())
    }

    /// Start connections that have never been discovered so their tools are known.
    pub async fn ensure_discovered(&self, profile: &Profile) -> Result<()> {
        let pending: Vec<Connection> = {
            let caches = self.0.caches.read().unwrap();
            self.profile_connections(profile)?
                .into_iter()
                .filter(|c| caches.get(&c.id).and_then(|c| c.cached_at).is_none())
                .filter(|c| {
                    !matches!(
                        self.status(&c.id),
                        ConnectionStatus::Error { .. } | ConnectionStatus::NeedsAuth
                    )
                })
                .collect()
        };
        let results =
            futures::future::join_all(pending.iter().map(|c| self.refresh_capabilities(&c.id)))
                .await;
        for (c, r) in pending.iter().zip(results) {
            if let Err(e) = r {
                tracing::warn!(connection = %c.alias, "discovery failed: {e:#}");
            }
        }
        Ok(())
    }

    /// All tools of the profile that aren't hidden, with their exposed names.
    pub fn exposed_tools(&self, profile: &Profile) -> Result<Vec<ExposedTool>> {
        let policies = self.tool_policies(&profile.id)?;
        let caches = self.0.caches.read().unwrap();
        let mut out = vec![];
        let mut seen = HashSet::new();
        for connection in self.profile_connections(profile)? {
            let Some(cache) = caches.get(&connection.id) else {
                continue;
            };
            for tool in &cache.tools {
                let policy = policies
                    .get(&(connection.id.clone(), tool.name.to_string()))
                    .copied()
                    .unwrap_or_default();
                if policy == ToolPolicy::Hidden {
                    continue;
                }
                let name = naming::exposed_tool_name(&connection.alias, &tool.name);
                if !seen.insert(name.clone()) {
                    continue;
                }
                out.push(ExposedTool {
                    name,
                    connection: connection.clone(),
                    tool: tool.clone(),
                    policy,
                });
            }
        }
        Ok(out)
    }

    pub fn search_tools(
        &self,
        profile: &Profile,
        query: &str,
        connection_alias: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ExposedTool>> {
        let tools: Vec<ExposedTool> = self
            .exposed_tools(profile)?
            .into_iter()
            .filter(|t| connection_alias.is_none_or(|a| t.connection.alias == a))
            .collect();
        if query.trim().is_empty() {
            return Ok(tools.into_iter().take(limit).collect());
        }
        let index = Bm25::new(tools.into_iter().map(|t| {
            let text = format!(
                "{} {} {} {} {}",
                t.connection.alias,
                t.connection.name,
                t.tool.name,
                t.tool.title.clone().unwrap_or_default(),
                t.tool.description.clone().unwrap_or_default()
            );
            (t, text)
        }));
        Ok(index
            .search(query, limit)
            .into_iter()
            .map(|(t, _)| t.clone())
            .collect())
    }

    pub async fn call_tool(
        &self,
        profile: &Profile,
        exposed_name: &str,
        arguments: Option<JsonObject>,
        reason: Option<String>,
    ) -> CallToolResult {
        let Some(target) = self
            .exposed_tools(profile)
            .ok()
            .and_then(|tools| tools.into_iter().find(|t| t.name == exposed_name))
        else {
            return error_result(format!("Unknown tool `{exposed_name}`"));
        };
        let started = Instant::now();
        let (status, result) = self
            .call_resolved(profile, &target, arguments, reason)
            .await;
        let error = match (&status, &result) {
            (CallStatus::Ok, _) => None,
            (_, r) => Some(result_text(r)),
        };
        let mut entry = CallLogEntry {
            id: 0,
            ts: now(),
            profile_id: profile.id.clone(),
            connection_id: target.connection.id.clone(),
            tool: target.tool.name.to_string(),
            duration_ms: started.elapsed().as_millis() as i64,
            status,
            error,
        };
        match self.0.store.insert_call(&entry) {
            Ok(id) => {
                entry.id = id;
                self.emit(HubEvent::CallLogged { entry });
            }
            Err(e) => tracing::warn!("failed to log call: {e:#}"),
        }
        result
    }

    async fn call_resolved(
        &self,
        profile: &Profile,
        target: &ExposedTool,
        arguments: Option<JsonObject>,
        reason: Option<String>,
    ) -> (CallStatus, CallToolResult) {
        if target.policy == ToolPolicy::Approval {
            match self
                .request_approval(profile, target, &arguments, reason)
                .await
            {
                Ok(()) => {}
                Err(status) => {
                    let msg = match status {
                        CallStatus::Timeout => "The user did not approve this tool call in time.",
                        _ => "The user denied this tool call.",
                    };
                    return (status, error_result(msg));
                }
            }
        }
        let (peer, upstream) = match self.peer(&target.connection).await {
            Ok(p) => p,
            Err(e) => {
                return (
                    CallStatus::Error,
                    error_result(format!(
                        "Connection `{}` is unavailable: {e:#}",
                        target.connection.alias
                    )),
                );
            }
        };
        upstream.inflight.fetch_add(1, Ordering::Relaxed);
        let _guard = InflightGuard(upstream);
        // Always send an object: many servers reject a call with no `arguments`.
        let params = CallToolRequestParams::new(target.tool.name.to_string())
            .with_arguments(arguments.unwrap_or_default());
        match peer.call_tool(params).await {
            Ok(result) => {
                let status = if result.is_error == Some(true) {
                    CallStatus::Error
                } else {
                    CallStatus::Ok
                };
                (status, result)
            }
            Err(e) => (
                CallStatus::Error,
                error_result(format!("Upstream error: {e}")),
            ),
        }
    }

    async fn request_approval(
        &self,
        profile: &Profile,
        target: &ExposedTool,
        arguments: &Option<JsonObject>,
        reason: Option<String>,
    ) -> std::result::Result<(), CallStatus> {
        let Some(broker) = self.0.approval.read().unwrap().clone() else {
            return Err(CallStatus::Denied);
        };
        let timeout = self
            .settings()
            .map(|s| s.approval_timeout_secs)
            .unwrap_or(60);
        let request = ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            profile_name: profile.name.clone(),
            connection_name: target.connection.name.clone(),
            connection_alias: target.connection.alias.clone(),
            tool: target.tool.name.to_string(),
            arguments: arguments
                .clone()
                .map(serde_json::Value::Object)
                .unwrap_or(serde_json::Value::Null),
            reason: reason.filter(|r| !r.trim().is_empty()),
        };
        match tokio::time::timeout(Duration::from_secs(timeout), broker.request(request)).await {
            Ok(ApprovalDecision::AllowOnce) => Ok(()),
            Ok(ApprovalDecision::AlwaysAllow) => {
                let _ = self.set_tool_policy(
                    &profile.id,
                    &target.connection.id,
                    &target.tool.name,
                    ToolPolicy::Allow,
                );
                Ok(())
            }
            Ok(ApprovalDecision::Deny) => Err(CallStatus::Denied),
            Err(_) => Err(CallStatus::Timeout),
        }
    }

    pub fn exposed_resources(&self, profile: &Profile) -> Result<Vec<Resource>> {
        let caches = self.0.caches.read().unwrap();
        let mut out = vec![];
        for connection in self.profile_connections(profile)? {
            let Some(cache) = caches.get(&connection.id) else {
                continue;
            };
            for r in &cache.resources {
                let mut r = r.clone();
                r.uri = naming::resource_uri(&connection.alias, &r.uri);
                r.name = format!("{} · {}", connection.name, r.name);
                out.push(r);
            }
        }
        Ok(out)
    }

    pub async fn read_resource(&self, profile: &Profile, uri: &str) -> Result<ReadResourceResult> {
        let (alias, upstream_uri) =
            naming::parse_resource_uri(uri).ok_or_else(|| anyhow!("unknown resource `{uri}`"))?;
        let connection = self
            .profile_connections(profile)?
            .into_iter()
            .find(|c| c.alias == alias)
            .ok_or_else(|| anyhow!("unknown connection `{alias}`"))?;
        let (peer, upstream) = self.peer(&connection).await?;
        upstream.inflight.fetch_add(1, Ordering::Relaxed);
        let _guard = InflightGuard(upstream);
        Ok(peer
            .read_resource(ReadResourceRequestParams::new(upstream_uri))
            .await?)
    }

    pub fn recent_calls(&self, limit: u32, before_id: Option<i64>) -> Result<Vec<CallLogEntry>> {
        self.0.store.recent_calls(limit, before_id)
    }

    pub(crate) fn secrets(&self) -> &Arc<dyn SecretStore> {
        &self.0.secrets
    }

    /// `PATH` from the login shell, for spawning user tools from a GUI app.
    pub fn shell_path(&self) -> Option<&str> {
        self.0.shell_path.as_deref()
    }

    pub(crate) fn oauth_flows(&self) -> &crate::oauth::PendingFlows {
        &self.0.oauth
    }
}

fn initial_status(c: &Connection) -> ConnectionStatus {
    if c.enabled {
        ConnectionStatus::Idle
    } else {
        ConnectionStatus::Disabled
    }
}

fn is_auth_error(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("401")
        || m.contains("unauthorized")
        || m.contains("auth required")
        || m.contains("needs authorization")
}

pub fn error_result(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

fn result_text(r: &CallToolResult) -> String {
    let text = r
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    text.chars().take(500).collect()
}

fn generate_token() -> String {
    use base64::Engine;
    let bytes: [u8; 32] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The `PATH` a login shell would have; GUI apps on macOS start with a minimal one.
fn login_shell_path() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = std::process::Command::new(shell)
        .args(["-ilc", "printf '__PLEXUS__%s__PLEXUS__' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout.find("__PLEXUS__")? + "__PLEXUS__".len();
    let end = stdout[start..].find("__PLEXUS__")? + start;
    Some(stdout[start..end].to_string()).filter(|p| !p.is_empty())
}
