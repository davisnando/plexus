//! The MCP server each profile endpoint speaks.

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
        SubscriptionFilter, Tool,
    },
    service::{NotificationContext, RequestContext, SubscriptionContext},
};
use serde_json::{Value, json};

use crate::{
    hub::{Hub, error_result},
    model::{Profile, ProfileMode},
};

const META_LIST_CONNECTIONS: &str = "list_connections";
const META_SEARCH: &str = "search_tools";
const META_DESCRIBE: &str = "describe_tool";
const META_CALL: &str = "call_tool";

#[derive(Clone)]
pub struct HubServer {
    hub: Hub,
    profile_id: String,
}

impl HubServer {
    pub fn new(hub: Hub, profile_id: String) -> Self {
        Self { hub, profile_id }
    }

    fn profile(&self) -> Result<Profile, McpError> {
        self.hub
            .profile(&self.profile_id)
            .map_err(|e| McpError::internal_error(e.to_string(), None))
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
                            "status": self.hub.status(&c.id),
                            "tool_count": count,
                        })
                    })
                    .collect();
                json_result(json!({ "connections": connections }))
            }
            META_SEARCH => {
                let _ = self.hub.ensure_discovered(profile).await;
                let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
                let connection = args.get("connection").and_then(Value::as_str);
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
                                json!({
                                    "name": t.name,
                                    "connection": t.connection.alias,
                                    "description": description,
                                })
                            })
                            .collect();
                        json_result(json!({ "tools": hits }))
                    }
                    Err(e) => error_result(e.to_string()),
                }
            }
            META_DESCRIBE => {
                let Some(name) = args.get("name").and_then(Value::as_str) else {
                    return error_result("`name` is required");
                };
                let tools = self.hub.exposed_tools(profile).unwrap_or_default();
                match tools.into_iter().find(|t| t.name == name) {
                    Some(t) => json_result(json!({
                        "name": t.name,
                        "connection": t.connection.alias,
                        "description": t.tool.description,
                        "input_schema": t.tool.input_schema,
                        "annotations": t.tool.annotations,
                        "requires_approval": t.policy == crate::model::ToolPolicy::Approval,
                    })),
                    None => error_result(format!(
                        "Unknown tool `{name}`. Use {META_SEARCH} to find tools."
                    )),
                }
            }
            META_CALL => {
                let Some(name) = args.get("name").and_then(Value::as_str) else {
                    return error_result("`name` is required");
                };
                let arguments = match args.get("arguments") {
                    None | Some(Value::Null) => None,
                    Some(Value::Object(o)) => Some(o.clone()),
                    Some(_) => return error_result("`arguments` must be an object"),
                };
                self.hub.call_tool(profile, name, arguments).await
            }
            other => error_result(format!("Unknown tool `{other}`")),
        }
    }
}

impl ServerHandler for HubServer {
    fn get_info(&self) -> ServerConfig {
        let instructions = match self.profile().map(|p| p.mode) {
            Ok(ProfileMode::Meta) => format!(
                "Plexus gives access to many connected services. Use `{META_SEARCH}` to find a \
                 tool, `{META_DESCRIBE}` to get its input schema, then `{META_CALL}` to run it."
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
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let profile = self.profile()?;
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
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let profile = self.profile()?;
        let result = match profile.mode {
            ProfileMode::Direct => {
                self.hub
                    .call_tool(&profile, &request.name, request.arguments)
                    .await
            }
            ProfileMode::Meta => {
                self.call_meta(&profile, &request.name, request.arguments.unwrap_or_default())
                    .await
            }
        };
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let profile = self.profile()?;
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
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let profile = self.profile()?;
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
    vec![
        Tool::new(
            META_LIST_CONNECTIONS,
            "List the connected services (e.g. several Jira or Sentry instances) with their alias, status and tool count.",
            schema(json!({ "type": "object", "properties": {} })),
        ),
        Tool::new(
            META_SEARCH,
            "Search available tools across all connections by keywords. Returns tool names to pass to describe_tool / call_tool.",
            schema(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Keywords, e.g. 'create jira issue'" },
                    "connection": { "type": "string", "description": "Optional connection alias to restrict the search to" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
                },
                "required": ["query"]
            })),
        ),
        Tool::new(
            META_DESCRIBE,
            "Get the full description and JSON input schema of a tool returned by search_tools.",
            schema(json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            })),
        ),
        Tool::new(
            META_CALL,
            "Call a tool by name with arguments matching its input schema (see describe_tool).",
            schema(json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["name"]
            })),
        ),
    ]
}
