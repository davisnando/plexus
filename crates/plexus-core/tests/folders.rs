//! Folder routing: each client session sees the connections whose scope
//! matches the folders (MCP roots) its client reports.
#![allow(deprecated)] // MCP roots, see `fetch_roots` in server.rs.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use plexus_core::{
    Hub, HubOptions,
    model::{ConnectionInput, Scope, Transport},
    secrets::MemorySecrets,
};
use rmcp::{
    ClientHandler, ErrorData as McpError, RoleClient, ServiceExt,
    model::{ClientCapabilities, ClientInfo, Implementation, ListRootsResult, Root},
    service::{RequestContext, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};

/// A client that works in the given folders (`None`: no roots support).
#[derive(Clone)]
struct InFolders(Option<Arc<Mutex<Vec<String>>>>);

impl ClientHandler for InFolders {
    fn get_info(&self) -> ClientInfo {
        let caps = match self.0 {
            Some(_) => ClientCapabilities::builder()
                .enable_roots()
                .enable_roots_list_changed()
                .build(),
            None => ClientCapabilities::default(),
        };
        ClientInfo::new(caps, Implementation::new("test", "0"))
    }

    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, McpError> {
        let folders = self.0.as_ref().map(|f| f.lock().unwrap().clone()).unwrap_or_default();
        Ok(ListRootsResult::new(
            folders.iter().map(|f| Root::new(format!("file://{f}"))).collect(),
        ))
    }
}

async fn connect(
    port: u16,
    token: &str,
    folders: Option<Arc<Mutex<Vec<String>>>>,
) -> RunningService<RoleClient, InFolders> {
    let config =
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header(token);
    InFolders(folders)
        .serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap()
}

async fn aliases(c: &RunningService<RoleClient, InFolders>) -> Vec<String> {
    let mut out: Vec<String> = c
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .filter_map(|t| t.name.split_once("__").map(|(a, _)| a.to_string()))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn folders(list: &[&str]) -> Arc<Mutex<Vec<String>>> {
    Arc::new(Mutex::new(list.iter().map(|s| s.to_string()).collect()))
}

#[tokio::test]
async fn sessions_get_the_connections_of_their_folder() {
    let hub = Hub::new(HubOptions {
        db_path: None,
        secrets: Arc::new(MemorySecrets::default()),
        resolve_shell_path: false,
    })
    .await
    .unwrap();
    let scoped = |rules: &[&str]| Scope::Folders {
        folders: rules.iter().map(|s| s.to_string()).collect(),
    };
    for (alias, scope) in [
        ("conn_a", scoped(&["/test-folder/testing"])),
        ("conn_b", Scope::Everywhere),
        ("conn_c", scoped(&["/test-folder/*"])),
        ("conn_d", scoped(&["/folder-a/*", "/folder-b/*"])),
    ] {
        hub.create_connection(ConnectionInput {
            alias: alias.into(),
            name: alias.into(),
            template_id: None,
            transport: Transport::Stdio {
                command: env!("CARGO_BIN_EXE_fake-mcp").into(),
                args: vec![],
                env: BTreeMap::from([("FAKE_NAME".into(), alias.into())]),
                secret_env: vec![],
            },
            enabled: true,
            idle_timeout_secs: None,
            scope: Some(scope),
            secrets: BTreeMap::new(),
        })
        .await
        .unwrap();
    }
    let server = plexus_core::http::serve(hub.clone(), 0).await.unwrap();
    let port = server.addr.port();
    let token = hub.profile_token(&hub.endpoint().unwrap().id).unwrap();

    let deep = connect(port, &token, Some(folders(&["/test-folder/testing/packages/api"]))).await;
    assert_eq!(aliases(&deep).await, ["conn_a", "conn_b", "conn_c"]);

    let parent = connect(port, &token, Some(folders(&["/test-folder"]))).await;
    assert_eq!(aliases(&parent).await, ["conn_b", "conn_c"]);

    let no_roots = connect(port, &token, None).await;
    assert_eq!(aliases(&no_roots).await, ["conn_b"]);

    // Several rules; and a session follows its client when the folder changes.
    let moving = folders(&["/folder-b/app"]);
    let c = connect(port, &token, Some(moving.clone())).await;
    assert_eq!(aliases(&c).await, ["conn_b", "conn_d"]);
    *moving.lock().unwrap() = vec!["/elsewhere".into()];
    c.notify_roots_list_changed().await.unwrap();
    let mut seen = vec![];
    for _ in 0..50 {
        seen = aliases(&c).await;
        if seen == ["conn_b"] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(seen, ["conn_b"]);

    // A folder header (Claude Code sends `${PWD}`) wins over roots, and works
    // without them.
    let mut config =
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header(&token);
    config.custom_headers.insert(
        http::HeaderName::from_static("x-plexus-folder"),
        http::HeaderValue::from_static("/test-folder/testing"),
    );
    let with_header = InFolders(None)
        .serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap();
    assert_eq!(aliases(&with_header).await, ["conn_a", "conn_b", "conn_c"]);
    with_header.cancel().await.unwrap();

    // A tool outside the session's folder can't be called either.
    let r = no_roots
        .call_tool(rmcp::model::CallToolRequestParams::new("conn_a__whoami"))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));

    for c in [deep, parent, no_roots, c] {
        c.cancel().await.unwrap();
    }
    hub.stop_all().await;
    server.shutdown().await;
}
