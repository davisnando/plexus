use std::{collections::BTreeMap, sync::Arc};

use plexus_core::{
    Hub, HubOptions,
    model::{ConnectionInput, Transport},
    secrets::{MemorySecrets, SecretStore},
};

#[tokio::test]
async fn deleting_a_connection_cleans_up_its_secrets() {
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
            scope: None,
            secrets: BTreeMap::from([("env:FAKE_NAME".into(), "x".into())]),
        })
        .await
        .unwrap();
    // A running upstream must not block deletion.
    hub.test_connection(&c.id).await.unwrap();

    hub.delete_connection(&c.id).await.unwrap();
    assert!(hub.list_connections().unwrap().is_empty());
    assert!(hub.session_profile(None).unwrap().connection_ids.is_empty());
    assert!(secrets.get(&format!("conn:{}:env:FAKE_NAME", c.id)).unwrap().is_none());
}
