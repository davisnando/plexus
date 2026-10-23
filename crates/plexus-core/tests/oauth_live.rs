//! Network test against real hosted MCP servers: discovery + dynamic client
//! registration up to the browser step. Run with `cargo test -- --ignored`.
use std::{collections::BTreeMap, sync::Arc};

use plexus_core::{
    Hub, HubOptions,
    model::{ConnectionInput, HttpAuth, Transport},
    secrets::MemorySecrets,
};

#[tokio::test]
#[ignore]
async fn oauth_flow_reaches_browser_step() {
    let hub = Hub::new(HubOptions {
        db_path: None,
        secrets: Arc::new(MemorySecrets::default()),
        resolve_shell_path: false,
    })
    .await
    .unwrap();
    for (alias, url) in [
        ("linear", "https://mcp.linear.app/mcp"),
        ("sentry", "https://mcp.sentry.dev/mcp"),
        ("atlassian", "https://mcp.atlassian.com/v1/mcp"),
    ] {
        let c = hub
            .create_connection(ConnectionInput {
                alias: alias.into(),
                name: alias.into(),
                template_id: None,
                transport: Transport::Http {
                    url: url.into(),
                    auth: HttpAuth::Oauth,
                    headers: BTreeMap::new(),
                    secret_headers: vec![],
                },
                enabled: true,
                idle_timeout_secs: None,
                secrets: BTreeMap::new(),
            })
            .await
            .unwrap();
        // Without tokens the connection reports needs_auth instead of erroring.
        assert!(hub.test_connection(&c.id).await.is_err());
        assert_eq!(
            hub.status(&c.id),
            plexus_core::model::ConnectionStatus::NeedsAuth
        );
        let auth_url = hub
            .start_oauth(&c.id, "http://127.0.0.1:7420/oauth/callback")
            .await
            .unwrap_or_else(|e| panic!("{alias}: {e:#}"));
        println!("{alias}: {}", &auth_url[..auth_url.len().min(90)]);
        assert!(auth_url.contains("code_challenge="));
    }
}

#[tokio::test]
#[ignore]
async fn unauthenticated_remote_reports_needs_auth() {
    let hub = Hub::new(HubOptions {
        db_path: None,
        secrets: Arc::new(MemorySecrets::default()),
        resolve_shell_path: false,
    })
    .await
    .unwrap();
    let c = hub
        .create_connection(ConnectionInput {
            alias: "atl".into(),
            name: "atl".into(),
            template_id: None,
            transport: Transport::Http {
                url: "https://mcp.atlassian.com/v1/mcp".into(),
                auth: HttpAuth::None,
                headers: BTreeMap::new(),
                secret_headers: vec![],
            },
            enabled: true,
            idle_timeout_secs: None,
            secrets: BTreeMap::new(),
        })
        .await
        .unwrap();
    let err = hub.test_connection(&c.id).await.unwrap_err();
    println!("error: {err:#}");
    assert_eq!(hub.status(&c.id), plexus_core::model::ConnectionStatus::NeedsAuth);
}
