use std::{collections::BTreeMap, sync::Arc};

use futures::future::BoxFuture;
use plexus_core::{
    ApprovalBroker, ApprovalDecision, ApprovalRequest, Hub, HubOptions,
    model::{CallStatus, ConnectionInput, ProfileInput, ProfileMode, ToolPolicy, Transport},
    secrets::MemorySecrets,
};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ReadResourceRequestParams},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::json;

async fn hub() -> Hub {
    Hub::new(HubOptions {
        db_path: None,
        secrets: Arc::new(MemorySecrets::default()),
        resolve_shell_path: false,
    })
    .await
    .unwrap()
}

fn fake(env: &[(&str, &str)], secret_env: &[&str]) -> Transport {
    Transport::Stdio {
        command: env!("CARGO_BIN_EXE_fake-mcp").into(),
        args: vec![],
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        secret_env: secret_env.iter().map(|s| s.to_string()).collect(),
    }
}

async fn client(port: u16, slug: &str, token: &str) -> RunningService<rmcp::RoleClient, ()> {
    let config = StreamableHttpClientTransportConfig::with_uri(format!(
        "http://127.0.0.1:{port}/mcp/{slug}"
    ))
    .auth_header(token);
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap()
}

fn text(r: &CallToolResult) -> String {
    r.content[0].as_text().unwrap().text.clone()
}

struct Deny;
impl ApprovalBroker for Deny {
    fn request(&self, _req: ApprovalRequest) -> BoxFuture<'static, ApprovalDecision> {
        Box::pin(async { ApprovalDecision::Deny })
    }
}

#[tokio::test]
async fn aggregates_two_instances_of_the_same_server() {
    let hub = hub().await;
    let a = hub
        .create_connection(ConnectionInput {
            alias: "fake_alpha".into(),
            name: "Fake Alpha".into(),
            template_id: None,
            transport: fake(&[("FAKE_NAME", "alpha")], &[]),
            enabled: true,
            idle_timeout_secs: None,
            secrets: BTreeMap::new(),
        })
        .await
        .unwrap();
    let b = hub
        .create_connection(ConnectionInput {
            alias: "fake_bravo".into(),
            name: "Fake Bravo".into(),
            template_id: None,
            transport: fake(&[], &["FAKE_NAME"]),
            enabled: true,
            idle_timeout_secs: None,
            secrets: BTreeMap::from([("env:FAKE_NAME".into(), "bravo".into())]),
        })
        .await
        .unwrap();
    let work = hub
        .create_profile(ProfileInput {
            slug: "work".into(),
            name: "Work".into(),
            mode: ProfileMode::Direct,
            connection_ids: vec![a.id.clone(), b.id.clone()],
        })
        .unwrap();
    let meta = hub
        .create_profile(ProfileInput {
            slug: "meta".into(),
            name: "Meta".into(),
            mode: ProfileMode::Meta,
            connection_ids: vec![a.id.clone(), b.id.clone()],
        })
        .unwrap();

    let server = plexus_core::http::serve(hub.clone(), 0).await.unwrap();
    let port = server.addr.port();

    // Wrong token is rejected before reaching MCP.
    let status = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/mcp/work"))
        .bearer_auth("nope")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);

    // Direct mode: namespaced tools from both instances.
    let c = client(port, "work", &hub.profile_token(&work.id).unwrap()).await;
    let mut names: Vec<String> = c
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["fake_alpha__echo", "fake_alpha__whoami", "fake_bravo__echo", "fake_bravo__whoami"]
    );
    let r = c
        .call_tool(CallToolRequestParams::new("fake_bravo__whoami"))
        .await
        .unwrap();
    assert_eq!(text(&r), "bravo");
    let r = c
        .call_tool(CallToolRequestParams::new("fake_alpha__whoami"))
        .await
        .unwrap();
    assert_eq!(text(&r), "alpha");

    // Resources are namespaced and routed.
    let resources = c.list_all_resources().await.unwrap();
    assert_eq!(resources.len(), 2);
    let bravo_info = resources
        .iter()
        .find(|r| r.uri.starts_with("plexus://fake_bravo/"))
        .unwrap();
    let read = c
        .read_resource(ReadResourceRequestParams::new(bravo_info.uri.clone()))
        .await
        .unwrap();
    assert!(format!("{:?}", read.contents).contains("info for bravo"));

    // Hidden tools disappear; approval tools ask the broker.
    hub.set_tool_policy(&work.id, &b.id, "echo", ToolPolicy::Hidden)
        .unwrap();
    hub.set_tool_policy(&work.id, &a.id, "echo", ToolPolicy::Approval)
        .unwrap();
    hub.set_approval_broker(Arc::new(Deny));
    let names: Vec<String> = c
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(!names.contains(&"fake_bravo__echo".to_string()));
    let r = c
        .call_tool(
            CallToolRequestParams::new("fake_alpha__echo")
                .with_arguments(json!({"text": "hi"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));

    // Meta mode: four tools, search + call through.
    let m = client(port, "meta", &hub.profile_token(&meta.id).unwrap()).await;
    let meta_tools = m.list_all_tools().await.unwrap();
    assert_eq!(meta_tools.len(), 4);
    let r = m
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(json!({"query": "account name"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(text(&r).contains("fake_alpha__whoami"));
    let r = m
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(
                json!({"name": "fake_bravo__echo", "arguments": {"text": "hello"}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(text(&r), "hello");

    // Every proxied call is logged with metadata only.
    let calls = hub.recent_calls(50, None).unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[1].status, CallStatus::Denied);

    c.cancel().await.unwrap();
    m.cancel().await.unwrap();
    hub.stop_all().await;
    server.shutdown().await;
}
