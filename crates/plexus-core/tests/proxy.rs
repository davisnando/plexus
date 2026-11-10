use std::{collections::BTreeMap, sync::Arc};

use futures::future::BoxFuture;
use plexus_core::{
    ApprovalBroker, ApprovalDecision, ApprovalRequest, Hub, HubOptions,
    model::{CallStatus, ConnectionInput, Profile, ProfileInput, ProfileMode, ToolPolicy, Transport},
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
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        secret_env: secret_env.iter().map(|s| s.to_string()).collect(),
    }
}

async fn client(port: u16, token: &str) -> RunningService<rmcp::RoleClient, ()> {
    let config =
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header(token);
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap()
}

fn set_mode(hub: &Hub, mode: ProfileMode) -> Profile {
    let e = hub.endpoint().unwrap();
    hub.update_profile(
        &e.id,
        ProfileInput {
            slug: e.slug.clone(),
            name: e.name.clone(),
            mode,
        },
    )
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
            scope: None,
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
            scope: None,
            secrets: BTreeMap::from([("env:FAKE_NAME".into(), "bravo".into())]),
        })
        .await
        .unwrap();
    let endpoint = hub.endpoint().unwrap();
    let token = hub.profile_token(&endpoint.id).unwrap();

    let server = plexus_core::http::serve(hub.clone(), 0).await.unwrap();
    let port = server.addr.port();

    // Wrong token is rejected before reaching MCP.
    let status = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .bearer_auth("nope")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);

    // Direct mode: namespaced tools from both instances.
    let c = client(port, &token).await;
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
        [
            "fake_alpha__echo",
            "fake_alpha__whoami",
            "fake_bravo__echo",
            "fake_bravo__whoami"
        ]
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
    hub.set_tool_policy(&endpoint.id, &b.id, "echo", ToolPolicy::Hidden)
        .unwrap();
    hub.set_tool_policy(&endpoint.id, &a.id, "echo", ToolPolicy::Approval)
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

    // Meta mode: six tools, search + call through.
    hub.set_tool_policy(&endpoint.id, &b.id, "echo", ToolPolicy::Allow)
        .unwrap();
    set_mode(&hub, ProfileMode::Meta);
    let m = client(port, &token).await;
    let meta_tools = m.list_all_tools().await.unwrap();
    assert_eq!(meta_tools.len(), 6);
    let r = m
        .call_tool(
            CallToolRequestParams::new("search_tools").with_arguments(
                json!({"query": "account name"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
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

/// Records approval requests and allows them.
#[derive(Default)]
struct Record(std::sync::Mutex<Vec<ApprovalRequest>>);
impl ApprovalBroker for Record {
    fn request(&self, req: ApprovalRequest) -> BoxFuture<'static, ApprovalDecision> {
        self.0.lock().unwrap().push(req);
        Box::pin(async { ApprovalDecision::AllowOnce })
    }
}

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

fn all_text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn meta_mode_batches_fans_out_and_shapes_results() {
    let hub = hub().await;
    let mut ids = vec![];
    for name in ["alpha", "bravo", "charlie"] {
        let c = hub
            .create_connection(ConnectionInput {
                alias: format!("fake_{name}"),
                name: format!("Fake {name}"),
                template_id: Some("fake".into()),
                transport: fake(&[("FAKE_NAME", name)], &[]),
                enabled: true,
                idle_timeout_secs: None,
                scope: None,
                secrets: BTreeMap::new(),
            })
            .await
            .unwrap();
        ids.push(c.id);
    }
    let meta = set_mode(&hub, ProfileMode::Meta);
    let server = plexus_core::http::serve(hub.clone(), 0).await.unwrap();
    let m = client(server.addr.port(), &hub.profile_token(&meta.id).unwrap()).await;
    let call = |name: &str, a: serde_json::Value| {
        m.call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(args(a)))
    };

    // Search can return schemas inline; describe takes several names.
    let r = call(
        "search_tools",
        json!({"query": "echo", "include_schema": true}),
    )
    .await
    .unwrap();
    assert!(text(&r).contains("input_schema"));
    let r = call(
        "describe_tool",
        json!({"names": ["fake_alpha__echo", "nope"]}),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text(&r)).unwrap();
    assert_eq!(v["tools"][0]["name"], "fake_alpha__echo");
    assert_eq!(v["unknown"], json!(["nope"]));

    // Fan-out: one tool on every connection of the same kind.
    let r = call(
        "call_tool",
        json!({"name": "fake_alpha__whoami", "connections": "*"}),
    )
    .await
    .unwrap();
    let out = all_text(&r);
    for name in ["alpha", "bravo", "charlie"] {
        assert!(out.contains(&format!("fake_{name}__whoami (ok)")), "{out}");
        assert!(out.contains(name));
    }
    let r = call(
        "call_tool",
        json!({"name": "whoami", "connections": ["fake_bravo"]}),
    )
    .await
    .unwrap();
    assert_eq!(text(&r), "bravo");
    let r = call(
        "call_tool",
        json!({"name": "whoami", "connections": ["fake_zulu"]}),
    )
    .await
    .unwrap();
    assert_eq!(r.is_error, Some(true));

    // Batch: independent calls, results in order.
    let r = call(
        "call_tools",
        json!({"calls": [
            {"name": "fake_alpha__echo", "arguments": {"text": "one"}},
            {"name": "fake_charlie__whoami"}
        ]}),
    )
    .await
    .unwrap();
    let out = all_text(&r);
    assert!(
        out.find("one").unwrap() < out.find("charlie").unwrap(),
        "{out}"
    );

    // Shaping: keep selected JSON fields, cap the size.
    let payload = json!({"items": [{"id": 1, "body": "x"}, {"id": 2, "body": "y"}]}).to_string();
    let r = call(
        "call_tool",
        json!({"name": "fake_alpha__echo", "arguments": {"text": payload}, "fields": ["items[].id"]}),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text(&r)).unwrap();
    assert_eq!(v, json!({"items": [{"id": 1}, {"id": 2}]}));
    let r = call(
        "call_tool",
        json!({"name": "fake_alpha__echo", "arguments": {"text": "z".repeat(500)}, "max_chars": 100}),
    )
    .await
    .unwrap();
    assert_eq!(text(&r).len(), 100);
    assert!(all_text(&r).contains("truncated"));

    // The reason reaches the approval prompt.
    let broker = Arc::new(Record::default());
    hub.set_approval_broker(broker.clone());
    hub.set_tool_policy(&meta.id, &ids[0], "whoami", ToolPolicy::Approval)
        .unwrap();
    let r = call(
        "call_tool",
        json!({"name": "fake_alpha__whoami", "reason": "checking the account"}),
    )
    .await
    .unwrap();
    assert_eq!(text(&r), "alpha");
    assert_eq!(
        broker.0.lock().unwrap()[0].reason.as_deref(),
        Some("checking the account")
    );

    // connect restarts a stdio connection and reports its tools.
    let r = call("connect", json!({"alias": "fake_bravo"}))
        .await
        .unwrap();
    assert!(text(&r).contains("2 tools"), "{}", text(&r));

    m.cancel().await.unwrap();
    hub.stop_all().await;
    server.shutdown().await;
}
