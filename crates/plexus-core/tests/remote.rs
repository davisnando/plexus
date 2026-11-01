//! OAuth sign-in for remote clients (the flow claude.ai connectors use), run
//! against the local endpoint.

use std::{collections::BTreeMap, sync::Arc};

use base64::Engine;
use futures::future::BoxFuture;
use plexus_core::{
    ApprovalBroker, ApprovalDecision, ApprovalRequest, GrantRequest, Hub, HubOptions,
    model::{ConnectionInput, ProfileInput, ProfileMode, Transport},
    secrets::MemorySecrets,
};
use reqwest::{StatusCode, header};
use rmcp::{
    ServiceExt,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use sha2::{Digest, Sha256};

struct Consent(bool);
impl ApprovalBroker for Consent {
    fn request(&self, _req: ApprovalRequest) -> BoxFuture<'static, ApprovalDecision> {
        Box::pin(async { ApprovalDecision::Deny })
    }
    fn request_grant(&self, req: GrantRequest) -> BoxFuture<'static, bool> {
        assert_eq!(req.client_name, "Test Connector");
        assert_eq!(req.redirect_host, "client.example");
        let allow = self.0;
        Box::pin(async move { allow })
    }
}

async fn setup(allow: bool) -> (Hub, plexus_core::http::HttpServer) {
    let hub = Hub::new(HubOptions {
        db_path: None,
        secrets: Arc::new(MemorySecrets::default()),
        resolve_shell_path: false,
    })
    .await
    .unwrap();
    hub.set_approval_broker(Arc::new(Consent(allow)));
    let c = hub
        .create_connection(ConnectionInput {
            alias: "fake".into(),
            name: "Fake".into(),
            template_id: None,
            transport: Transport::Stdio {
                command: env!("CARGO_BIN_EXE_fake-mcp").into(),
                args: vec![],
                env: Default::default(),
                secret_env: vec![],
            },
            enabled: true,
            idle_timeout_secs: None,
            secrets: BTreeMap::new(),
        })
        .await
        .unwrap();
    hub.create_profile(ProfileInput {
        slug: "work".into(),
        name: "Work".into(),
        mode: ProfileMode::Direct,
        connection_ids: vec![c.id],
    })
    .unwrap();
    let server = plexus_core::http::serve(hub.clone(), 0).await.unwrap();
    (hub, server)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn json(r: reqwest::Response) -> Value {
    serde_json::from_slice(&r.bytes().await.unwrap()).unwrap()
}

fn location(r: &reqwest::Response) -> reqwest::Url {
    reqwest::Url::parse(r.headers()[header::LOCATION].to_str().unwrap()).unwrap()
}

fn param(url: &reqwest::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut s = reqwest::Url::parse("http://x/").unwrap();
    s.query_pairs_mut().extend_pairs(pairs);
    s.query().unwrap().to_string()
}

/// Runs discovery, registration and the browser leg; returns (client_id, code, verifier).
async fn sign_in(base: &str) -> (String, Result<String, String>, String) {
    let http = http();

    let r = http.post(format!("{base}/mcp/work")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let challenge = r.headers()[header::WWW_AUTHENTICATE].to_str().unwrap().to_string();
    let prm_url = challenge
        .split("resource_metadata=\"")
        .nth(1)
        .unwrap()
        .trim_end_matches('"')
        .to_string();
    assert_eq!(prm_url, format!("{base}/.well-known/oauth-protected-resource/mcp/work"));

    let prm = json(http.get(&prm_url).send().await.unwrap()).await;
    assert_eq!(prm["resource"], format!("{base}/mcp/work"));
    let issuer = prm["authorization_servers"][0].as_str().unwrap().to_string();
    assert_eq!(issuer, format!("{base}/auth/work"));

    let asm = json(
        http.get(format!("{base}/.well-known/oauth-authorization-server/auth/work"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(asm["issuer"], issuer);

    let reg = http
        .post(asm["registration_endpoint"].as_str().unwrap())
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"client_name":"Test Connector","redirect_uris":["https://client.example/cb"]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);
    let client_id = json(reg).await["client_id"].as_str().unwrap().to_string();

    let verifier = "a-sufficiently-long-code-verifier-for-pkce-0123456789";
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let mut authorize = reqwest::Url::parse(asm["authorization_endpoint"].as_str().unwrap()).unwrap();
    authorize.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", "https://client.example/cb"),
        ("state", "xyz"),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("resource", &format!("{base}/mcp/work")),
    ]);
    let r = http.get(authorize).send().await.unwrap();
    assert!(r.status().is_redirection());
    let wait = location(&r);

    // The page polls until the popup is answered.
    let outcome = loop {
        let r = http.get(wait.clone()).send().await.unwrap();
        if r.status().is_redirection() {
            break location(&r);
        }
        assert!(r.text().await.unwrap().contains("Waiting for approval"));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(outcome.host_str(), Some("client.example"));
    assert_eq!(param(&outcome, "state").as_deref(), Some("xyz"));
    assert_eq!(param(&outcome, "iss"), Some(issuer));
    let code = param(&outcome, "code").ok_or_else(|| param(&outcome, "error").unwrap());
    (client_id, code, verifier.to_string())
}

#[tokio::test]
async fn web_connector_signs_in_with_oauth_and_calls_tools() {
    let (hub, server) = setup(true).await;
    let base = format!("http://{}", server.addr);
    let (client_id, code, verifier) = sign_in(&base).await;
    let code = code.unwrap();
    let http = http();
    let token_url = format!("{base}/auth/work/token");

    let bad = http
        .post(&token_url)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            ("code_verifier", "wrong"),
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    // A failed attempt burns the code.
    let (client_id, code, _) = sign_in(&base).await;
    let code = code.unwrap();

    let tokens = json(
        http.post(&token_url)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", &client_id),
                ("redirect_uri", "https://client.example/cb"),
                ("code_verifier", &verifier),
            ]))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let access = tokens["access_token"].as_str().unwrap().to_string();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_string();

    let profile = hub.profile_by_slug("work").unwrap();
    let grants = hub.remote_grants(&profile.id).unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].client_name, "Test Connector");

    let config =
        StreamableHttpClientTransportConfig::with_uri(format!("{base}/mcp/work")).auth_header(&access);
    let mcp = ().serve(StreamableHttpClientTransport::from_config(config)).await.unwrap();
    let tools = mcp.list_all_tools().await.unwrap();
    assert!(tools.iter().any(|t| t.name.starts_with("fake__")));
    mcp.cancel().await.unwrap();

    let rotated = json(
        http.post(&token_url)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", &client_id),
            ]))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let access2 = rotated["access_token"].as_str().unwrap().to_string();
    assert!(hub.authenticate("work", &access2).is_some());
    // Refresh tokens are single use.
    let reuse = http
        .post(&token_url)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client_id),
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(reuse.status(), StatusCode::BAD_REQUEST);

    hub.revoke_grant(&grants[0].id).unwrap();
    assert!(hub.authenticate("work", &access2).is_none());
    assert!(hub.remote_grants(&profile.id).unwrap().is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn denied_consent_redirects_with_access_denied() {
    let (_hub, server) = setup(false).await;
    let (_, code, _) = sign_in(&format!("http://{}", server.addr)).await;
    assert_eq!(code, Err("access_denied".to_string()));
    server.shutdown().await;
}

#[tokio::test]
async fn foreign_hosts_are_refused() {
    let (_hub, server) = setup(true).await;
    let r = http()
        .get(format!("http://{}/health", server.addr))
        .header(header::HOST, "attacker.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    // Cloudflare-forwarded requests with a loopback Host are not local.
    let r = http()
        .get(format!("http://{}/health", server.addr))
        .header("cf-ray", "abc")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    server.shutdown().await;
}
