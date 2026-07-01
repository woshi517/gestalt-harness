use gestalt_runtime::unstable::lifecycle::protocol::{
    negotiate_protocol_version, PROTOCOL_V2_METHODS, PROTOCOL_V2_METHOD_CANCEL,
    PROTOCOL_V2_METHOD_DESCRIBE_CAPABILITIES, PROTOCOL_V2_METHOD_INITIALIZE,
    PROTOCOL_V2_METHOD_INVOKE, PROTOCOL_V2_METHOD_SHUTDOWN,
};

#[test]
fn protocol_v2_declares_minimal_method_set() {
    assert_eq!(
        PROTOCOL_V2_METHODS,
        &[
            PROTOCOL_V2_METHOD_INITIALIZE,
            PROTOCOL_V2_METHOD_DESCRIBE_CAPABILITIES,
            PROTOCOL_V2_METHOD_INVOKE,
            PROTOCOL_V2_METHOD_SHUTDOWN,
            PROTOCOL_V2_METHOD_CANCEL,
        ]
    );
}

#[test]
fn protocol_version_negotiation_prefers_v2_and_rejects_unknown_versions() {
    assert_eq!(
        negotiate_protocol_version(&["1.0".to_string(), "2.0".to_string()]),
        Some("2.0".to_string())
    );
    assert_eq!(negotiate_protocol_version(&["1.0".to_string()]), None);
    assert_eq!(negotiate_protocol_version(&["3.0".to_string()]), None);
}
