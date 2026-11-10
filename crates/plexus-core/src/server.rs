//! The MCP server the endpoint speaks, one instance per client session.
//! Each request sees the connections whose scope matches the folder the
//! client works in: the `X-Plexus-Folder` header if the client sends one
//! (Claude Code fills it from `${PWD}`), otherwise the session's MCP roots.
//! The header matters because clients on protocol 2026-07-28 have no session
//! to ask roots of when they list tools.

use std::{path::PathBuf, sync::Arc, time::Duration};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
        SubscriptionFilter, Tool,
    },
    service::{NotificationContext, Peer, RequestContext, SubscriptionContext},
};
use serde_json::{Value, json};

use crate::{
    hub::{ExposedTool, Hub, ReconnectOutcome, error_result},
    model::{Profile, ProfileMode, ToolPolicy},
    shape::{self, Shape},
};

const META_LIST_CONNECTIONS: &str = "list_connections";
const META_SEARCH: &str = "search_tools";
const META_DESCRIBE: &str = "describe_tool";
const META_CALL: &str = "call_tool";
const META_CALL_MANY: &str = "call_tools";
const META_CONNECT: &str = "connect";

/// Default cap on the text a meta call returns, shared across fan-out results.
const DEFAULT_MAX_CHARS: usize = 50_000;
/// Most upstream calls one `call_tool` / `call_tools` request may expand to.
const MAX_CALLS: usize = 20;

/// Request header carrying the client's working folder.
pub const FOLDER_HEADER: &str = "x-plexus-folder";

/// How long to wait for a client to answer `roots/list`.
const ROOTS_TIMEOUT: Duration = Duration::from_secs(5);

/// The folders a session's client reported: `None` if it reports none.
type Roots = Option<Vec<PathBuf>>;

#[derive(Clone)]
pub struct HubServer {
    hub: Hub,
    profile_id: String,
    /// Fetched on first use; cleared when the client says its roots changed.
    roots: Arc<tokio::sync::Mutex<Option<Roots>>>,
}

impl HubServer {
    pub fn new(hub: Hub, profile_id: String) -> Self {
        Self {
            hub,
            profile_id,
            roots: Default::default(),
        }
    }

    /// The endpoint as this request sees it.
    async fn profile(&self, context: &RequestContext<RoleServer>) -> Result<Profile, McpError> {
        let roots = match header_folder(context) {
            Some(folder) => Some(vec![folder]),
            None => self.roots(&context.peer).await,
        };
        self.hub
            .session_profile(roots.as_deref())
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    async fn roots(&self, peer: &Peer<RoleServer>) -> Roots {
        let mut cached = self.roots.lock().await;
        if let Some(roots) = cached.as_ref() {
            return roots.clone();
        }
        let roots = fetch_roots(peer).await;
        *cached = Some(roots.clone());
        roots
    }

    fn direct_tools(&self, profile: &Profile) -> Result<Vec<Tool>, McpError> {
        let tools = self
            .hub
            .exposed_tools(profile)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(tools
            .into_iter()
            .map(|t| {
                let mut tool = t.tool.clone();
                tool.name = t.name.into();
                let description = tool.description.as_deref().unwrap_or_default();
                tool.description = Some(format!("[{}] {description}", t.connection.name).into());
                tool
            })
            .collect())
    }

    async fn call_meta(&self, profile: &Profile, name: &str, args: JsonObject) -> CallToolResult {
        match name {
            META_LIST_CONNECTIONS => {
                let _ = self.hub.ensure_discovered(profile).await;
                let tools = self.hub.exposed_tools(profile).unwrap_or_default();
                let connections: Vec<Value> = self
                    .hub
                    .list_connections()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| c.enabled && profile.connection_ids.contains(&c.id))
                    .map(|c| {
                        let count = tools.iter().filter(|t| t.connection.id == c.id).count();
                        json!({
                            "alias": c.alias,
                            "name": c.name,
                            "template": c.template_id,
                            "status": self.hub.status(&c.id),
                            "tool_count": count,
                        })
                    })
                    .collect();
                json_result(json!({ "connections": connections }))
            }
            META_SEARCH => {
                let _ = self.hub.ensure_discovered(profile).await;
                let query = args
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let connection = args.get("connection").and_then(Value::as_str);
                let include_schema = args
                    .get("include_schema")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(10)
                    .clamp(1, 50) as usize;
                match self.hub.search_tools(profile, query, connection, limit) {
                    Ok(tools) => {
                        let hits: Vec<Value> = tools
                            .into_iter()
                            .map(|t| {
                                let description: String = t
                                    .tool
                                    .description
                                    .as_deref()
                                    .unwrap_or_default()
                                    .chars()
                                    .take(300)
                                    .collect();
                                let mut hit = json!({
                                    "name": t.name,
                                    "connection": t.connection.alias,
                                    "description": description,
                                });
                                if include_schema {
                                    hit["input_schema"] = json!(t.tool.input_schema);
                                    hit["requires_approval"] =
                                        json!(t.policy == ToolPolicy::Approval);
                                }
                                hit
                            })
                            .collect();
                        json_result(json!({ "tools": hits }))
                    }
                    Err(e) => error_result(e.to_string()),
                }
            }
            META_DESCRIBE => {
                let tools = self.hub.exposed_tools(profile).unwrap_or_default();
                let describe = |t: &ExposedTool| {
                    json!({
                        "name": t.name,
                        "connection": t.connection.alias,
                        "description": t.tool.description,
                        "input_schema": t.tool.input_schema,
                        "annotations": t.tool.annotations,
                        "requires_approval": t.policy == ToolPolicy::Approval,
                    })
                };
                if let Some(name) = args.get("name").and_then(Value::as_str) {
                    return match tools.iter().find(|t| t.name == name) {
                        Some(t) => json_result(describe(t)),
                        None => error_result(format!(
                            "Unknown tool `{name}`. Use {META_SEARCH} to find tools."
                        )),
                    };
                }
                let names = match strings_of(args.get("names"), "names") {
                    Ok(n) if !n.is_empty() => n,
                    Ok(_) => return error_result("`name` or `names` is required"),
                    Err(e) => return error_result(e),
                };
                let (found, unknown): (Vec<_>, Vec<_>) = names
                    .iter()
                    .map(|n| (n, tools.iter().find(|t| &t.name == n)))
                    .partition(|(_, t)| t.is_some());
                let mut out = json!({
                    "tools": found.into_iter().filter_map(|(_, t)| t.map(describe)).collect::<Vec<_>>(),
                });
                if !unknown.is_empty() {
                    out["unknown"] = json!(unknown.into_iter().map(|(n, _)| n).collect::<Vec<_>>());
                }
                json_result(out)
            }
            META_CALL => {
                let tools = self.hub.exposed_tools(profile).unwrap_or_default();
                match plan(&tools, &args) {
                    Ok(planned) => {
                        let (max_chars, reason) = budget_and_reason(&args);
                        self.run(profile, planned, max_chars, reason).await
                    }
                    Err(e) => error_result(e),
                }
            }
            META_CALL_MANY => {
                let Some(calls) = args.get("calls").and_then(Value::as_array) else {
                    return error_result("`calls` must be an array of {name, arguments}");
                };
                let tools = self.hub.exposed_tools(profile).unwrap_or_default();
                let mut planned = vec![];
                for (i, call) in calls.iter().enumerate() {
                    let Some(call) = call.as_object() else {
                        return error_result(format!("calls[{i}] must be an object"));
                    };
                    match plan(&tools, call) {
                        Ok(p) => planned.extend(p),
                        Err(e) => return error_result(format!("calls[{i}]: {e}")),
                    }
                }
                let (max_chars, reason) = budget_and_reason(&args);
                self.run(profile, planned, max_chars, reason).await
            }
            META_CONNECT => {
                let Some(alias) = args.get("alias").and_then(Value::as_str) else {
                    return error_result("`alias` is required");
                };
                let connection = self
                    .hub
                    .list_connections()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|c| c.alias == alias && profile.connection_ids.contains(&c.id));
                let Some(connection) = connection else {
                    return error_result(format!(
                        "Unknown connection `{alias}`. Use {META_LIST_CONNECTIONS} to see them."
                    ));
                };
                match self.hub.reconnect(&connection.id).await {
                    Ok(ReconnectOutcome::Connected { tool_count }) => {
                        text_result(format!("`{alias}` is connected ({tool_count} tools)."))
                    }
                    Ok(ReconnectOutcome::SignInStarted { url, opened: true }) => {
                        text_result(format!(
                            "Opened a sign-in page for `{alias}` in the user's browser. Ask the \
                             user to finish signing in, then retry. If no page opened, give them \
                             this link: {url}"
                        ))
                    }
                    Ok(ReconnectOutcome::SignInStarted { url, opened: false }) => {
                        text_result(format!(
                            "`{alias}` needs a sign-in. Ask the user to open this link and \
                             approve access, then retry: {url}"
                        ))
                    }
                    Ok(ReconnectOutcome::Failed { message }) => {
                        error_result(format!("Could not connect `{alias}`: {message}"))
                    }
                    Err(e) => error_result(format!("Could not connect `{alias}`: {e:#}")),
                }
            }
            other => error_result(format!("Unknown tool `{other}`")),
        }
    }

    /// Run planned calls concurrently. A single call returns its own result;
    /// several are merged under one `## [n] name (status)` header each.
    async fn run(
        &self,
        profile: &Profile,
        planned: Vec<Planned>,
        max_chars: usize,
        reason: Option<String>,
    ) -> CallToolResult {
        if planned.is_empty() {
            return error_result("Nothing to call");
        }
        if planned.len() > MAX_CALLS {
            return error_result(format!(
                "{} calls requested; at most {MAX_CALLS} per request",
                planned.len()
            ));
        }
        // The character budget is shared, so ten fan-out results get a tenth each.
        let per_result = max_chars.div_ceil(planned.len());
        let results = futures::future::join_all(planned.iter().map(|p| {
            let reason = reason.clone();
            async move {
                let result = self
                    .hub
                    .call_tool(profile, &p.name, p.arguments.clone(), reason)
                    .await;
                shape::apply(
                    result,
                    &Shape {
                        fields: p.fields.clone(),
                        max_chars: per_result,
                    },
                )
            }
        }))
        .await;
        if planned.len() == 1 {
            return results.into_iter().next().unwrap_or_default();
        }
        let failed = results.iter().filter(|r| r.is_error == Some(true)).count();
        let mut content = vec![];
        for (i, (p, r)) in planned.iter().zip(results).enumerate() {
            let status = if r.is_error == Some(true) {
                "error"
            } else {
                "ok"
            };
            // Clients often join text blocks without a separator.
            let gap = if i == 0 { "" } else { "\n\n" };
            content.push(ContentBlock::text(format!(
                "{gap}## [{}] {} ({status})\n",
                i + 1,
                p.name
            )));
            content.extend(r.content);
        }
        if failed == planned.len() {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        }
    }
}

/// One upstream call planned from a `call_tool` / `call_tools` request.
struct Planned {
    name: String,
    arguments: Option<JsonObject>,
    fields: Vec<String>,
}

/// Turn `{name, arguments, connections?, fields?}` into the calls to make.
fn plan(tools: &[ExposedTool], call: &JsonObject) -> Result<Vec<Planned>, String> {
    let name = call
        .get("name")
        .and_then(Value::as_str)
        .ok_or("`name` is required")?;
    let arguments = match call.get("arguments") {
        None | Some(Value::Null) => None,
        Some(Value::Object(o)) => Some(o.clone()),
        Some(_) => return Err("`arguments` must be an object".into()),
    };
    let fields = strings_of(call.get("fields"), "fields")?;
    let names = match call.get("connections") {
        None | Some(Value::Null) => vec![name.to_string()],
        Some(c) => fan_out(tools, name, &strings_of(Some(c), "connections")?)?,
    };
    Ok(names
        .into_iter()
        .map(|name| Planned {
            name,
            arguments: arguments.clone(),
            fields: fields.clone(),
        })
        .collect())
}

/// The exposed names of `name`'s tool on each of `connections`. `name` is an
/// exposed name (`jira_acme__search`) or a bare upstream name (`search`);
/// `"*"` means every connection of the same kind that has the tool.
fn fan_out(
    tools: &[ExposedTool],
    name: &str,
    connections: &[String],
) -> Result<Vec<String>, String> {
    let reference = tools.iter().find(|t| t.name == name);
    let upstream = reference.map_or(name, |t| t.tool.name.as_ref());
    let candidates: Vec<&ExposedTool> = tools.iter().filter(|t| t.tool.name == upstream).collect();
    if candidates.is_empty() {
        return Err(format!(
            "Unknown tool `{name}`. Use {META_SEARCH} to find tools."
        ));
    }
    if connections.iter().any(|c| c == "*") {
        let template = reference.and_then(|t| t.connection.template_id.as_ref());
        return Ok(candidates
            .iter()
            .filter(|t| template.is_none() || t.connection.template_id.as_ref() == template)
            .map(|t| t.name.clone())
            .collect());
    }
    connections
        .iter()
        .map(|alias| {
            candidates
                .iter()
                .find(|t| &t.connection.alias == alias)
                .map(|t| t.name.clone())
                .ok_or_else(|| format!("Connection `{alias}` has no tool `{upstream}`"))
        })
        .collect()
}

/// A string or an array of strings.
fn strings_of(value: Option<&Value>, key: &str) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("`{key}` must be a list of strings")),
        Some(_) => Err(format!("`{key}` must be a list of strings")),
    }
}

fn budget_and_reason(args: &JsonObject) -> (usize, Option<String>) {
    let max_chars = args
        .get("max_chars")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_MAX_CHARS, |n| n as usize);
    let reason = args.get("reason").and_then(Value::as_str).map(String::from);
    (max_chars, reason)
}

impl ServerHandler for HubServer {
    fn get_info(&self) -> ServerConfig {
        let instructions = match self.hub.endpoint().map(|p| p.mode) {
            Ok(ProfileMode::Meta) => format!(
                "Plexus gives access to many connected services. Use `{META_SEARCH}` to find a \
                 tool, `{META_DESCRIBE}` to get its input schema, then `{META_CALL}` to run it. \
                 Use `{META_CALL_MANY}` for independent calls, `connections` to run one tool on \
                 several instances (e.g. every Jira), and `fields` to keep large results small. \
                 If a connection shows `needs_auth` or `error`, call `{META_CONNECT}`."
            ),
            _ => "Plexus aggregates several MCP servers. Tool names are prefixed with the \
                  connection alias (`{alias}__{tool}`); descriptions start with the connection name."
                .to_string(),
        };
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_resources()
                .enable_resources_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new("plexus", env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let profile = self.profile(&context).await?;
        let tools = match profile.mode {
            ProfileMode::Direct => {
                let _ = self.hub.ensure_discovered(&profile).await;
                self.direct_tools(&profile)?
            }
            ProfileMode::Meta => meta_tools(),
        };
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let profile = self.profile(&context).await?;
        let result = match profile.mode {
            ProfileMode::Direct => {
                self.hub
                    .call_tool(&profile, &request.name, request.arguments, None)
                    .await
            }
            ProfileMode::Meta => {
                self.call_meta(
                    &profile,
                    &request.name,
                    request.arguments.unwrap_or_default(),
                )
                .await
            }
        };
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let profile = self.profile(&context).await?;
        let _ = self.hub.ensure_discovered(&profile).await;
        let resources = self
            .hub
            .exposed_resources(&profile)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(ListResourcesResult {
            resources,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let profile = self.profile(&context).await?;
        self.hub
            .read_resource(&profile, &request.uri)
            .await
            .map(Into::into)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        self.hub
            .register_legacy_peer(self.profile_id.clone(), context.peer.clone());
    }

    async fn on_roots_list_changed(&self, context: NotificationContext<RoleServer>) {
        let old = self.roots.lock().await.take();
        let new = self.roots(&context.peer).await;
        if old.as_ref() != Some(&new) {
            let _ = context.peer.notify_tool_list_changed().await;
            let _ = context.peer.notify_resource_list_changed().await;
        }
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.supported_by(&self.get_info().capabilities))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let mut rx = self.hub.subscribe_list_changed();
        loop {
            tokio::select! {
                _ = context.cancelled() => return Ok(()),
                msg = rx.recv() => match msg {
                    Ok(profile_id) if profile_id == self.profile_id => {
                        let _ = context.sink().notify_tool_list_changed().await;
                        let _ = context.sink().notify_resource_list_changed().await;
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                },
            }
        }
    }
}

/// The absolute folder in the request's `X-Plexus-Folder` header. An
/// unexpanded placeholder such as `${PWD}` is not absolute and is ignored.
fn header_folder(context: &RequestContext<RoleServer>) -> Option<PathBuf> {
    let parts = context.extensions.get::<http::request::Parts>()?;
    let value = parts.headers.get(FOLDER_HEADER)?;
    let folder = PathBuf::from(String::from_utf8_lossy(value.as_bytes()).trim());
    folder.is_absolute().then_some(folder)
}

/// Asks the client for its folders, if it supports roots. Roots are
/// deprecated by SEP-2577 (2026-07-28) but stay functional for at least a
/// year, and Claude Code, VS Code and Cursor send them.
#[allow(deprecated)]
async fn fetch_roots(peer: &Peer<RoleServer>) -> Roots {
    let supported = peer
        .peer_info()
        .is_some_and(|info| info.capabilities.roots.is_some());
    if !supported {
        return None;
    }
    match tokio::time::timeout(ROOTS_TIMEOUT, peer.list_roots()).await {
        Ok(Ok(result)) => Some(
            result
                .roots
                .iter()
                .filter_map(|r| crate::folders::root_path(&r.uri))
                .collect(),
        ),
        Ok(Err(e)) => {
            tracing::warn!("roots/list failed: {e}");
            None
        }
        Err(_) => {
            tracing::warn!("roots/list timed out");
            None
        }
    }
}

fn text_result(text: String) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text)])
}

fn json_result(value: Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )])
}

fn schema(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(o) => Arc::new(o),
        _ => Arc::new(JsonObject::new()),
    }
}

fn meta_tools() -> Vec<Tool> {
    let fields = json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Keep only these JSON fields of the result, e.g. [\"issues[].key\", \"issues[].fields.summary\"]. `[]` steps into arrays."
    });
    let connections = json!({
        "type": ["array", "string"],
        "items": { "type": "string" },
        "description": "Run the same tool on several connections of the same kind: a list of aliases, or \"*\" for all of them. Results come back per connection."
    });
    let max_chars = json!({
        "type": "integer",
        "minimum": 0,
        "default": DEFAULT_MAX_CHARS,
        "description": "Cap on the returned text, shared across all results; 0 means no limit."
    });
    let reason = json!({
        "type": "string",
        "description": "One sentence on why you are making this call. Shown to the user when the tool needs approval."
    });
    vec![
        Tool::new(
            META_LIST_CONNECTIONS,
            "List the connected services (e.g. several Jira or Sentry instances) with their alias, kind (template), status and tool count.",
            schema(json!({ "type": "object", "properties": {} })),
        ),
        Tool::new(
            META_SEARCH,
            "Search available tools across all connections by keywords. Returns tool names to pass to describe_tool / call_tool; set include_schema to get input schemas right away.",
            schema(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Keywords, e.g. 'create jira issue'" },
                    "connection": { "type": "string", "description": "Optional connection alias to restrict the search to" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 },
                    "include_schema": { "type": "boolean", "default": false, "description": "Include each tool's input schema" }
                },
                "required": ["query"]
            })),
        ),
        Tool::new(
            META_DESCRIBE,
            "Get the full description and JSON input schema of one tool (`name`) or several (`names`) returned by search_tools.",
            schema(json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "names": { "type": "array", "items": { "type": "string" } }
                }
            })),
        ),
        Tool::new(
            META_CALL,
            "Call a tool by name with arguments matching its input schema (see describe_tool). Large results are truncated; use `fields` to keep only what you need, and `connections` to run it on several instances at once.",
            schema(json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "arguments": { "type": "object" },
                    "connections": connections,
                    "fields": fields,
                    "max_chars": max_chars,
                    "reason": reason
                },
                "required": ["name"]
            })),
        ),
        Tool::new(
            META_CALL_MANY,
            format!(
                "Run several independent tool calls concurrently (at most {MAX_CALLS}, counting fan-out). Each call takes the same options as call_tool; results come back in order."
            ),
            schema(json!({
                "type": "object",
                "properties": {
                    "calls": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "arguments": { "type": "object" },
                                "connections": connections,
                                "fields": fields
                            },
                            "required": ["name"]
                        }
                    },
                    "max_chars": max_chars,
                    "reason": reason
                },
                "required": ["calls"]
            })),
        ),
        Tool::new(
            META_CONNECT,
            "Reconnect a connection whose status is error or needs_auth. Restarts it, or opens a sign-in page in the user's browser when the service needs one.",
            schema(json!({
                "type": "object",
                "properties": { "alias": { "type": "string" } },
                "required": ["alias"]
            })),
        ),
    ]
}
