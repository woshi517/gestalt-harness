use std::fs;
use std::path::PathBuf;

use gestalt_core::event::AgentEvent;
use gestalt_runtime::event_bus::RuntimeEventBus;
use gestalt_runtime::workspace_context::{
    apply_memory_proposal, select_memory_entries, MemoryContextConfig, MemoryEntry,
    MemoryOperation, MemoryProposal, MemoryProposalDecision, MemorySelectionStrategy,
    MemoryWriteMode, WorkspaceContextError,
};

#[tokio::test]
async fn test_memory_proposal_lifecycle() {
    let temp_dir = std::env::temp_dir().join(format!(
        "gestalt-test-memory-proposal-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp_dir).unwrap();

    let gestalt_dir = temp_dir.join(".gestalt");
    fs::create_dir_all(&gestalt_dir).unwrap();

    let memory_file_path = gestalt_dir.join("memory.md");
    let initial_content =
        "# Memory\n\n## Facts\n\n- <!-- gestalt-memory-id: mem_1 --> initial entry\n";
    fs::write(&memory_file_path, initial_content).unwrap();

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(initial_content.as_bytes());
    let base_hash = format!("{:x}", hasher.finalize());

    let memory_config = MemoryContextConfig {
        enabled: Some(true),
        path: Some(PathBuf::from(".gestalt/memory.md")),
        required: Some(false),
        strategy: None,
        max_tokens: None,
        max_bytes: None,
        pinned_section: Some("Facts".to_string()),
        snapshot: None,
        write_mode: None,
    };

    let event_bus = RuntimeEventBus::new();
    let mut receiver = event_bus.subscribe();

    // 1. Create a proposal to add a new memory entry
    let proposal = MemoryProposal {
        proposal_id: "prop_1".to_string(),
        source_session_id: "sess_123".to_string(),
        base_hash: base_hash.clone(),
        operations: vec![MemoryOperation::Add {
            section: "Facts".to_string(),
            content: "new proposed memory entry".to_string(),
        }],
        rationale: Some("Testing".to_string()),
    };

    // Apply AcceptAll decision
    let res = apply_memory_proposal(
        &temp_dir,
        &memory_config,
        &proposal,
        &MemoryProposalDecision::AcceptAll,
        &event_bus,
        None,
    )
    .await;

    assert!(res.is_ok(), "apply_memory_proposal failed: {:?}", res.err());

    // Verify file contents updated
    let updated_content = fs::read_to_string(&memory_file_path).unwrap();
    assert!(updated_content.contains("initial entry"));
    assert!(updated_content.contains("new proposed memory entry"));
    assert!(updated_content.contains("<!-- gestalt-memory-id: mem_"));

    // Check events emitted
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        if let gestalt_runtime::event_bus::RuntimeEvent::Agent {
            event: agent_ev, ..
        } = &*event
        {
            events.push(agent_ev.clone());
        }
    }

    assert!(events.iter().any(|e| matches!(e, AgentEvent::MemoryProposalCreated { proposal_id, .. } if proposal_id == "prop_1")));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::MemoryProposalDecisionRecorded { proposal_id, decision, .. } if proposal_id == "prop_1" && decision == "accepted")));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::MemoryWriteSucceeded { .. })));

    // 2. Test Conflict: apply again with original base_hash
    let res_conflict = apply_memory_proposal(
        &temp_dir,
        &memory_config,
        &proposal,
        &MemoryProposalDecision::AcceptAll,
        &event_bus,
        None,
    )
    .await;

    assert!(res_conflict.is_err());
    assert!(matches!(
        res_conflict.err().unwrap(),
        WorkspaceContextError::MemoryWriteConflict { .. }
    ));

    // Check conflict event emitted
    let mut events_conflict = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        if let gestalt_runtime::event_bus::RuntimeEvent::Agent {
            event: agent_ev, ..
        } = &*event
        {
            events_conflict.push(agent_ev.clone());
        }
    }
    assert!(events_conflict
        .iter()
        .any(|e| matches!(e, AgentEvent::MemoryWriteConflict { .. })));

    // 3. Test Reject: apply with fresh base_hash but Reject decision
    let mut hasher2 = Sha256::new();
    hasher2.update(updated_content.as_bytes());
    let base_hash2 = format!("{:x}", hasher2.finalize());

    let proposal2 = MemoryProposal {
        proposal_id: "prop_2".to_string(),
        source_session_id: "sess_123".to_string(),
        base_hash: base_hash2,
        operations: vec![MemoryOperation::Add {
            section: "Facts".to_string(),
            content: "another entry".to_string(),
        }],
        rationale: Some("Testing reject".to_string()),
    };

    let res_reject = apply_memory_proposal(
        &temp_dir,
        &memory_config,
        &proposal2,
        &MemoryProposalDecision::Reject,
        &event_bus,
        None,
    )
    .await;

    assert!(res_reject.is_ok());

    // File content should remain exactly the same
    let content_after_reject = fs::read_to_string(&memory_file_path).unwrap();
    assert_eq!(updated_content, content_after_reject);

    // Decision recorded rejected
    let mut events_reject = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        if let gestalt_runtime::event_bus::RuntimeEvent::Agent {
            event: agent_ev, ..
        } = &*event
        {
            events_reject.push(agent_ev.clone());
        }
    }
    assert!(events_reject.iter().any(|e| matches!(e, AgentEvent::MemoryProposalDecisionRecorded { proposal_id, decision, .. } if proposal_id == "prop_2" && decision == "rejected")));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_memory_write_mode_disabled() {
    let temp_dir = std::env::temp_dir().join(format!(
        "gestalt-test-memory-disabled-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp_dir).unwrap();

    let memory_config = MemoryContextConfig {
        enabled: Some(true),
        path: Some(PathBuf::from(".gestalt/memory.md")),
        required: Some(false),
        strategy: None,
        max_tokens: None,
        max_bytes: None,
        pinned_section: Some("Facts".to_string()),
        snapshot: None,
        write_mode: Some(MemoryWriteMode::Disabled),
    };

    let proposal = MemoryProposal {
        proposal_id: "prop_disabled".to_string(),
        source_session_id: "sess_123".to_string(),
        base_hash: String::new(),
        operations: vec![MemoryOperation::Add {
            section: "Facts".to_string(),
            content: "should not be written".to_string(),
        }],
        rationale: None,
    };

    let event_bus = RuntimeEventBus::new();
    let res = apply_memory_proposal(
        &temp_dir,
        &memory_config,
        &proposal,
        &MemoryProposalDecision::AcceptAll,
        &event_bus,
        None,
    )
    .await;

    assert!(res.is_err());
    assert!(matches!(
        res.err().unwrap(),
        WorkspaceContextError::MemoryWriteDisabled
    ));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_memory_path_escape() {
    let temp_dir = std::env::temp_dir().join(format!(
        "gestalt-test-memory-escape-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp_dir).unwrap();

    // Memory path escapes the workspace root!
    let memory_config = MemoryContextConfig {
        enabled: Some(true),
        path: Some(PathBuf::from("../escaped_memory.md")),
        required: Some(false),
        strategy: None,
        max_tokens: None,
        max_bytes: None,
        pinned_section: Some("Facts".to_string()),
        snapshot: None,
        write_mode: Some(MemoryWriteMode::Proposal),
    };

    let proposal = MemoryProposal {
        proposal_id: "prop_escape".to_string(),
        source_session_id: "sess_123".to_string(),
        base_hash: String::new(),
        operations: vec![MemoryOperation::Add {
            section: "Facts".to_string(),
            content: "escaped content".to_string(),
        }],
        rationale: None,
    };

    let event_bus = RuntimeEventBus::new();
    let res = apply_memory_proposal(
        &temp_dir,
        &memory_config,
        &proposal,
        &MemoryProposalDecision::AcceptAll,
        &event_bus,
        None,
    )
    .await;

    assert!(res.is_err());
    // Since no policy engine is passed, escaping path should result in PathEscape error.
    assert!(matches!(
        res.err().unwrap(),
        WorkspaceContextError::PathEscape { .. }
    ));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_memory_selection_strategies() {
    // 1. Budgeted: pinned survive trimming, unpinned trimmed when over budget.
    let entries = vec![
        MemoryEntry {
            id: "1".to_string(),
            section: "Facts".to_string(),
            content: "pinned entry 1".to_string(),
            pinned: true,
            source_order: 0,
            content_hash: "h1".to_string(),
        },
        MemoryEntry {
            id: "2".to_string(),
            section: "Facts".to_string(),
            content: "pinned entry 2".to_string(),
            pinned: true,
            source_order: 1,
            content_hash: "h2".to_string(),
        },
        MemoryEntry {
            id: "3".to_string(),
            section: "General".to_string(),
            content: "unpinned entry 1".to_string(),
            pinned: false,
            source_order: 2,
            content_hash: "h3".to_string(),
        },
    ];

    // Very small budget. In the old code, pinned entries would be dropped if they exceeded max_tokens.
    // In our new code, pinned entries survive trimming and are always included.
    let (selected, omissions, _total_tokens) =
        select_memory_entries(&entries, MemorySelectionStrategy::Budgeted, 5);

    // Both pinned entries must be included.
    assert_eq!(selected.len(), 2);
    assert!(selected.iter().any(|e| e.id == "1"));
    assert!(selected.iter().any(|e| e.id == "2"));

    // The unpinned entry must be omitted.
    assert_eq!(omissions.len(), 1);
    assert_eq!(omissions[0].path_or_label, "mem_id:3");
    assert_eq!(omissions[0].reason, "budget_exhausted");

    // 2. Full Strategy: budget is bypassed.
    let (selected_full, omissions_full, _) =
        select_memory_entries(&entries, MemorySelectionStrategy::Full, 5);
    assert_eq!(selected_full.len(), 3);
    assert!(omissions_full.is_empty());
}
