//! Minimal stdio MCP server used by the integration tests. Its identity comes
//! from `FAKE_NAME`, so two instances behave like two accounts of one service.

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
        ListResourcesResult, ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
        ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::json;

#[derive(Clone)]
struct Fake {
    name: String,
}

fn schema(v: serde_json::Value) -> Arc<JsonObject> {
    Arc::new(v.as_object().cloned().unwrap_or_default())
}

impl ServerHandler for Fake {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: vec![
                Tool::new(
                    "echo",
                    "Echo the given text back",
                    schema(json!({"type": "object", "properties": {"text": {"type": "string"}}})),
                ),
                Tool::new(
                    "whoami",
                    "Return the account name of this instance",
                    schema(json!({"type": "object", "properties": {}})),
                ),
            ],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let text = match request.name.as_ref() {
            "echo" => request
                .arguments
                .and_then(|a| a.get("text").and_then(|t| t.as_str()).map(String::from))
                .unwrap_or_default(),
            "whoami" => self.name.clone(),
            other => return Ok(CallToolResult::error(vec![ContentBlock::text(format!("no tool {other}"))]).into()),
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![Resource::new("fake://info", "info")],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            format!("info for {}", self.name),
            request.uri,
        )])
        .into())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let name = std::env::var("FAKE_NAME").unwrap_or_else(|_| "anonymous".into());
    eprintln!("fake-mcp starting as {name}");
    let service = Fake { name }.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
