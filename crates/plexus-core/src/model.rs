use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type Timestamp = i64;

pub fn now() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// How Plexus reaches an upstream MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        /// Non-secret environment variables.
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Names of environment variables whose values live in the keychain.
        #[serde(default)]
        secret_env: Vec<String>,
    },
    Http {
        url: String,
        #[serde(default)]
        auth: HttpAuth,
        /// Non-secret headers.
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// Names of headers whose values live in the keychain.
        #[serde(default)]
        secret_headers: Vec<String>,
    },
}

impl Transport {
    /// Keychain keys (relative to the connection) this transport reads.
    pub fn secret_keys(&self) -> Vec<String> {
        match self {
            Transport::Stdio { secret_env, .. } => {
                secret_env.iter().map(|k| format!("env:{k}")).collect()
            }
            Transport::Http {
                auth,
                secret_headers,
                ..
            } => {
                let mut keys: Vec<String> =
                    secret_headers.iter().map(|h| format!("header:{h}")).collect();
                match auth {
                    HttpAuth::Bearer => keys.push("bearer".into()),
                    HttpAuth::Oauth => keys.push("oauth".into()),
                    HttpAuth::None => {}
                }
                keys
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpAuth {
    #[default]
    None,
    /// Static bearer token stored in the keychain under `bearer`.
    Bearer,
    /// MCP OAuth; credentials stored in the keychain under `oauth`.
    Oauth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    /// Prefix for exposed tool names, e.g. `jira_acme`.
    pub alias: String,
    pub name: String,
    pub template_id: Option<String>,
    pub transport: Transport,
    pub enabled: bool,
    pub idle_timeout_secs: u64,
    pub created_at: Timestamp,
}

/// Payload for creating or updating a connection. `secrets` maps keychain keys
/// (`env:NAME`, `header:NAME`, `bearer`) to values; omitted keys keep their
/// current value, keys no longer referenced by the transport are deleted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInput {
    pub alias: String,
    pub name: String,
    #[serde(default)]
    pub template_id: Option<String>,
    pub transport: Transport,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProfileMode {
    /// Every upstream tool is re-exported as `{alias}__{tool}`.
    #[default]
    Direct,
    /// A handful of meta tools: search, describe, call.
    Meta,
}

impl ProfileMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileMode::Direct => "direct",
            ProfileMode::Meta => "meta",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "meta" => ProfileMode::Meta,
            _ => ProfileMode::Direct,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    /// URL segment: `/mcp/{slug}`.
    pub slug: String,
    pub name: String,
    pub mode: ProfileMode,
    pub connection_ids: Vec<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileInput {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub mode: ProfileMode,
    #[serde(default)]
    pub connection_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    #[default]
    Allow,
    Approval,
    Hidden,
}

impl ToolPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolPolicy::Allow => "allow",
            ToolPolicy::Approval => "approval",
            ToolPolicy::Hidden => "hidden",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "approval" => ToolPolicy::Approval,
            "hidden" => ToolPolicy::Hidden,
            _ => ToolPolicy::Allow,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionStatus {
    Disabled,
    Idle,
    Starting,
    Running,
    NeedsAuth,
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallStatus {
    Ok,
    Error,
    Denied,
    Timeout,
}

impl CallStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CallStatus::Ok => "ok",
            CallStatus::Error => "error",
            CallStatus::Denied => "denied",
            CallStatus::Timeout => "timeout",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "ok" => CallStatus::Ok,
            "denied" => CallStatus::Denied,
            "timeout" => CallStatus::Timeout,
            _ => CallStatus::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallLogEntry {
    pub id: i64,
    pub ts: Timestamp,
    pub profile_id: String,
    pub connection_id: String,
    pub tool: String,
    pub duration_ms: i64,
    pub status: CallStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub port: u16,
    pub default_idle_timeout_secs: u64,
    pub log_retention_days: u32,
    pub approval_timeout_secs: u64,
    /// Start the public tunnel together with the server.
    pub tunnel_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: 7420,
            default_idle_timeout_secs: 600,
            log_retention_days: 7,
            approval_timeout_secs: 60,
            tunnel_enabled: false,
        }
    }
}

/// The public tunnel: a Cloudflare quick tunnel to the local server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TunnelStatus {
    Off,
    Starting,
    Running { url: String },
    Error { message: String },
}

/// A remote client that signed in to a profile over OAuth.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteGrant {
    pub id: String,
    pub profile_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub created_at: Timestamp,
    pub last_used_at: Timestamp,
}
