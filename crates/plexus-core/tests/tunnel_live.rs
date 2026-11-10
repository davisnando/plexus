//! Live Cloudflare quick tunnel (needs `cloudflared` and network):
//! `cargo test -p plexus-core --test tunnel_live -- --ignored`

use std::{sync::Arc, time::Duration};

use plexus_core::{
    Hub, HubOptions,
    model::TunnelStatus,
    secrets::MemorySecrets,
};
use reqwest::{StatusCode, header};

#[tokio::test]
#[ignore]
async fn quick_tunnel_exposes_only_public_routes() {
    let hub = Hub::new(HubOptions {
        db_path: None,
        secrets: Arc::new(MemorySecrets::default()),
        resolve_shell_path: true,
    })
    .await
    .unwrap();
    let server = plexus_core::http::serve(hub.clone(), 0).await.unwrap();
    hub.start_tunnel().await.unwrap();

    let base = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match hub.tunnel_status() {
                TunnelStatus::Running { url } => break url,
                TunnelStatus::Error { message } => panic!("tunnel failed: {message}"),
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
    })
    .await
    .expect("tunnel URL");
    println!("tunnel: {base}");

    let http = reqwest::Client::new();
    // DNS for a fresh quick tunnel can take a few seconds to resolve.
    let mut health = None;
    for _ in 0..30 {
        if let Ok(r) = http.get(format!("{base}/health")).send().await
            && r.status() == StatusCode::OK
        {
            health = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(health.is_some(), "tunnel never became reachable");

    let r = http.post(format!("{base}/mcp")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let challenge = r.headers()[header::WWW_AUTHENTICATE].to_str().unwrap();
    assert!(
        challenge.contains(&format!("{base}/.well-known/oauth-protected-resource/mcp")),
        "{challenge}"
    );

    let token = hub.profile_token(&hub.endpoint().unwrap().id).unwrap();
    let r = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK, "{}", r.text().await.unwrap());

    let r = http.get(format!("{base}/oauth/callback")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    hub.stop_tunnel().await;
    assert_eq!(hub.tunnel_status(), TunnelStatus::Off);
    server.shutdown().await;
}
