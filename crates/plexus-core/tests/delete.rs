use std::{collections::BTreeMap, sync::Arc};

use plexus_core::{
    Hub, HubOptions,
    model::{ConnectionInput, ProfileInput, ProfileMode, Transport},
    secrets::{MemorySecrets, SecretStore},
};

#[tokio::test]
async fn deleting_cleans_up_connections_profiles_and_secrets() {
    let secrets = Arc::new(MemorySecrets::default());
    let hub = Hub::new(HubOptions {
        db_path: None,
        secrets: secrets.clone(),
        resolve_shell_path: false,
    })
    .await
    .unwrap();
    let c = hub
        .create_connection(ConnectionInput {
            alias: "fake".into(),
            name: "Fake".into(),
            template_id: None,
            transport: Transport::Stdio {
                command: env!("CARGO_BIN_EXE_fake-mcp").into(),
                args: vec![],
                env: BTreeMap::new(),
                secret_env: vec!["FAKE_NAME".into()],
            },
            enabled: true,
            idle_timeout_secs: None,
            secrets: BTreeMap::from([("env:FAKE_NAME".into(), "x".into())]),
        })
        .await
        .unwrap();
    let p = hub
        .create_profile(ProfileInput {
            slug: "work".into(),
            name: "Work".into(),
            mode: ProfileMode::Direct,
            connection_ids: vec![c.id.clone()],
        })
        .unwrap();
    // A running upstream must not block deletion.
    hub.test_connection(&c.id).await.unwrap();

    hub.delete_connection(&c.id).await.unwrap();
    assert!(hub.list_connections().unwrap().is_empty());
    assert!(hub.profile(&p.id).unwrap().connection_ids.is_empty());
    assert!(secrets.get(&format!("conn:{}:env:FAKE_NAME", c.id)).unwrap().is_none());

    let token = hub.profile_token(&p.id).unwrap();
    hub.delete_profile(&p.id).unwrap();
    assert!(hub.list_profiles().unwrap().is_empty());
    assert!(hub.authenticate("work", &token).is_none());
}
