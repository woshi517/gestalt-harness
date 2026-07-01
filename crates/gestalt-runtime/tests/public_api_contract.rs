use gestalt_runtime::api::v1::{
    AgentRuntimeBuilder, ControlHostOptions, InMemoryArtifactStore, InMemoryControlHost,
    RuntimeBackedControlHost, RuntimeConfig, StartSessionRequestV1,
};

#[test]
fn stable_namespace_exposes_embedding_and_control_contracts() {
    let builder = AgentRuntimeBuilder::new().config(RuntimeConfig::default());
    let _host =
        RuntimeBackedControlHost::new(builder, std::sync::Arc::new(InMemoryArtifactStore::new()))
            .expect("stable runtime host construction should succeed");
    let _in_memory = InMemoryControlHost::with_options(ControlHostOptions::default());
    let request = StartSessionRequestV1 {
        session_id: None,
        idempotency_key: None,
        config_override: None,
    };

    let encoded = serde_json::to_value(&request).unwrap();
    let decoded: StartSessionRequestV1 = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, request);
}
