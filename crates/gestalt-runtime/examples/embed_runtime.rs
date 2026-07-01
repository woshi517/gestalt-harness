use gestalt_runtime::control::contract::{
    ApprovalControlV1, ApprovalDecisionV1, ApprovalIdV1, ApprovalProjectionV1, ArtifactAccessV1,
    CancelRunRequestV1, ControlErrorV1, CreateArtifactRequestV1, EventPayloadV1, EventSourceV1,
    IdempotencyKeyV1, PollEventsRequestV1, ReadArtifactRangeRequestV1, RespondToApprovalRequestV1,
    SessionControlV1, StartSessionRequestV1, SubmitMessageRequestV1, ToolCallIdV1,
};
use gestalt_runtime::control::LocalControlHost;

#[tokio::main]
async fn main() -> Result<(), ControlErrorV1> {
    let host = LocalControlHost::new();
    let started = host
        .start_session(StartSessionRequestV1 {
            session_id: None,
            idempotency_key: Some(IdempotencyKeyV1("example-start".to_string())),
            config_override: None,
        })
        .await?;

    let acknowledgement = host
        .submit_message(SubmitMessageRequestV1 {
            session_id: started.session_id.clone(),
            message: "hello".to_string(),
            idempotency_key: Some(IdempotencyKeyV1("example-message".to_string())),
        })
        .await?;
    assert!(acknowledgement.acknowledged);
    host.complete_run(&started.session_id, &started.run_id)
        .await?;

    let events = host
        .poll_events(PollEventsRequestV1 {
            session_id: started.session_id.clone(),
            cursor: None,
            limit: None,
            kinds: None,
        })
        .await?;
    assert!(events
        .events
        .iter()
        .any(|event| matches!(event.payload, EventPayloadV1::RunCompleted)));

    let approval_id = ApprovalIdV1("example-approval".to_string());
    host.add_approval(ApprovalProjectionV1 {
        approval_id: approval_id.clone(),
        tool_call_id: ToolCallIdV1("example-tool-call".to_string()),
        correlation_id: None,
        summary: "Run example tool".to_string(),
        editable_input_rules: None,
        original_hash: "example-input-hash".to_string(),
        edited_hash: None,
        expires_at: None,
        is_cancelled: false,
        session_grant_terms: None,
    });
    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id,
        decision: ApprovalDecisionV1::Approve,
    })
    .await?;

    let cancellable = host
        .start_session(StartSessionRequestV1 {
            session_id: None,
            idempotency_key: Some(IdempotencyKeyV1("example-cancel".to_string())),
            config_override: None,
        })
        .await?;
    assert!(
        host.cancel_run(CancelRunRequestV1 {
            session_id: cancellable.session_id,
            run_id: cancellable.run_id,
            correlation_id: None,
        })
        .await?
        .cancelled
    );

    let artifact = host
        .create_artifact(CreateArtifactRequestV1 {
            session_id: started.session_id.clone(),
            display_path: "result.txt".to_string(),
            data: b"done".to_vec(),
        })
        .await?;
    host.read_artifact_range(ReadArtifactRangeRequestV1 {
        session_id: started.session_id,
        artifact_id: artifact.metadata.logical_id,
        offset: 0,
        length: 4,
    })
    .await?;

    Ok(())
}
