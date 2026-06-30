mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    use gestalt_app::config::{validate_workspace_config, CliOverrides, EffectiveConfig};
    use gestalt_core::approval::{ApprovalDecision, ApprovalRequest};
    use gestalt_core::event::AgentEvent;
    use gestalt_tui::tui::services::load_session_transcript;
    use gestalt_tui::tui::state::{push_event, TranscriptEntry, TuiAppState, TuiFocus, TuiModal};
    use gestalt_tui::tui::update::{handle_key_event, TuiUiAction};

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(ref value) = self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn press_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn press_ctrl_c() -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn create_temp_workspace() -> (
        MutexGuard<'static, ()>,
        EnvVarGuard,
        EnvVarGuard,
        PathBuf,
        EffectiveConfig,
    ) {
        let env_guard = ENV_MUTEX.lock().unwrap();
        let temp_dir =
            std::env::temp_dir().join(format!("gestalt-tui-reg-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", temp_dir.as_os_str().to_str().unwrap());
        let skills_guard = EnvVarGuard::set("GESTALT_NO_GLOBAL_SKILLS", "1");
        fs::write(
            temp_dir.join("gestalt.json"),
            r#"{"version":1,"defaults":{"provider":"mock"}}"#,
        )
        .unwrap();

        let config = validate_workspace_config(&CliOverrides {
            workspace: Some(temp_dir.clone()),
            ..CliOverrides::default()
        })
        .unwrap();

        (env_guard, xdg_guard, skills_guard, temp_dir, config)
    }

    #[test]
    fn test_push_event_aggregates_agent_text_and_thinking() {
        let mut events = Vec::new();

        // 1. Text delta aggregation
        push_event(
            &mut events,
            AgentEvent::Text {
                delta: "Hello ".to_string(),
            },
        );
        push_event(
            &mut events,
            AgentEvent::Text {
                delta: "world!".to_string(),
            },
        );
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TranscriptEntry::Agent("Hello world!".to_string())
        );

        // 2. Thinking delta aggregation
        push_event(
            &mut events,
            AgentEvent::Thinking {
                delta: "Hmm ".to_string(),
            },
        );
        push_event(
            &mut events,
            AgentEvent::Thinking {
                delta: "yes".to_string(),
            },
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[1], TranscriptEntry::Thinking("Hmm yes".to_string()));

        // 3. User message (should not aggregate)
        push_event(
            &mut events,
            AgentEvent::UserMessage {
                content: "ping".to_string(),
            },
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[2], TranscriptEntry::User("ping".to_string()));
    }

    #[test]
    fn test_load_session_transcript_chronologically_and_lineage() {
        let (_env_guard, _xdg_guard, _skills_guard, temp_dir, config) = create_temp_workspace();
        let runs_dir = temp_dir.join(".gestalt/runs");
        fs::create_dir_all(&runs_dir).unwrap();

        let session_id = "test-session-123";

        // Create Run 1
        let run_1_dir = runs_dir.join("run-1");
        fs::create_dir_all(&run_1_dir).unwrap();
        let run_1_manifest = serde_json::json!({
            "v": 1,
            "session_id": session_id,
            "run_id": "run-1",
            "parent_run_id": null,
            "base_checkpoint": null,
            "run_kind": "new",
            "created_at": "2026-06-04T12:00:00Z",
            "lifecycle_state": "completed",
            "finalized_at": "2026-06-04T12:05:00Z",
            "failure_kind": null,
            "interrupted_phase": null,
            "compatibility_fingerprint": {
                "context_pipeline_version": "1.0",
                "tool_schema_hash": "",
                "policy_fingerprint": "",
                "hook_contract_hash": "",
                "execution_mode": "confirm"
            }
        });
        fs::write(
            run_1_dir.join("run.json"),
            serde_json::to_string_pretty(&run_1_manifest).unwrap(),
        )
        .unwrap();

        let run_1_trace = r#"{"v":1,"session_id":"test-session-123","run_id":"run-1","turn_id":0,"seq":0,"ts":"2026-06-04T12:00:00Z","event":{"type":"user_message","content":"First turn"},"redacted":false}
{"v":1,"session_id":"test-session-123","run_id":"run-1","turn_id":0,"seq":1,"ts":"2026-06-04T12:00:01Z","event":{"type":"text","delta":"Response 1"},"redacted":false}
"#;
        fs::write(run_1_dir.join("trace.jsonl"), run_1_trace).unwrap();

        // Create Run 2 (continuation of Run 1)
        let run_2_dir = runs_dir.join("run-2");
        fs::create_dir_all(&run_2_dir).unwrap();
        let run_2_manifest = serde_json::json!({
            "v": 1,
            "session_id": session_id,
            "run_id": "run-2",
            "parent_run_id": "run-1",
            "base_checkpoint": null,
            "run_kind": "continue",
            "created_at": "2026-06-04T12:10:00Z",
            "lifecycle_state": "completed",
            "finalized_at": "2026-06-04T12:15:00Z",
            "failure_kind": null,
            "interrupted_phase": null,
            "compatibility_fingerprint": {
                "context_pipeline_version": "1.0",
                "tool_schema_hash": "",
                "policy_fingerprint": "",
                "hook_contract_hash": "",
                "execution_mode": "confirm"
            }
        });
        fs::write(
            run_2_dir.join("run.json"),
            serde_json::to_string_pretty(&run_2_manifest).unwrap(),
        )
        .unwrap();

        let run_2_trace = r#"{"v":1,"session_id":"test-session-123","run_id":"run-2","turn_id":0,"seq":0,"ts":"2026-06-04T12:10:00Z","event":{"type":"user_message","content":"Second turn"},"redacted":false}
{"v":1,"session_id":"test-session-123","run_id":"run-2","turn_id":0,"seq":1,"ts":"2026-06-04T12:10:01Z","event":{"type":"text","delta":"Response 2"},"redacted":false}
"#;
        fs::write(run_2_dir.join("trace.jsonl"), run_2_trace).unwrap();

        // Create Sibling Run 3 (branched from Run 1, should not collapse into Run 2's transcript)
        let run_3_dir = runs_dir.join("run-3");
        fs::create_dir_all(&run_3_dir).unwrap();
        let run_3_manifest = serde_json::json!({
            "v": 1,
            "session_id": session_id,
            "run_id": "run-3",
            "parent_run_id": "run-1",
            "base_checkpoint": null,
            "run_kind": "branch",
            "created_at": "2026-06-04T12:20:00Z",
            "lifecycle_state": "completed",
            "finalized_at": "2026-06-04T12:25:00Z",
            "failure_kind": null,
            "interrupted_phase": null,
            "compatibility_fingerprint": {
                "context_pipeline_version": "1.0",
                "tool_schema_hash": "",
                "policy_fingerprint": "",
                "hook_contract_hash": "",
                "execution_mode": "confirm"
            }
        });
        fs::write(
            run_3_dir.join("run.json"),
            serde_json::to_string_pretty(&run_3_manifest).unwrap(),
        )
        .unwrap();

        let run_3_trace = r#"{"v":1,"session_id":"test-session-123","run_id":"run-3","turn_id":0,"seq":0,"ts":"2026-06-04T12:20:00Z","event":{"type":"user_message","content":"Branched turn"},"redacted":false}
{"v":1,"session_id":"test-session-123","run_id":"run-3","turn_id":0,"seq":1,"ts":"2026-06-04T12:20:01Z","event":{"type":"text","delta":"Branched Response"},"redacted":false}
"#;
        fs::write(run_3_dir.join("trace.jsonl"), run_3_trace).unwrap();

        // 1. Tracing back from run-2 should show run-1 + run-2, but NOT run-3
        let transcript_2 = load_session_transcript(&config, session_id, Some("run-2")).unwrap();
        assert_eq!(transcript_2.len(), 4);
        assert_eq!(
            transcript_2[0],
            TranscriptEntry::User("First turn".to_string())
        );
        assert_eq!(
            transcript_2[1],
            TranscriptEntry::Agent("Response 1".to_string())
        );
        assert_eq!(
            transcript_2[2],
            TranscriptEntry::User("Second turn".to_string())
        );
        assert_eq!(
            transcript_2[3],
            TranscriptEntry::Agent("Response 2".to_string())
        );

        // 2. Tracing back from run-3 should show run-1 + run-3, but NOT run-2
        let transcript_3 = load_session_transcript(&config, session_id, Some("run-3")).unwrap();
        assert_eq!(transcript_3.len(), 4);
        assert_eq!(
            transcript_3[0],
            TranscriptEntry::User("First turn".to_string())
        );
        assert_eq!(
            transcript_3[1],
            TranscriptEntry::Agent("Response 1".to_string())
        );
        assert_eq!(
            transcript_3[2],
            TranscriptEntry::User("Branched turn".to_string())
        );
        assert_eq!(
            transcript_3[3],
            TranscriptEntry::Agent("Branched Response".to_string())
        );

        // 3. Tracing back from run-1 should show only run-1
        let transcript_1 = load_session_transcript(&config, session_id, Some("run-1")).unwrap();
        assert_eq!(transcript_1.len(), 2);
        assert_eq!(
            transcript_1[0],
            TranscriptEntry::User("First turn".to_string())
        );
        assert_eq!(
            transcript_1[1],
            TranscriptEntry::Agent("Response 1".to_string())
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_slash_command_parities_in_chat_prompt() {
        let (_env_guard, _xdg_guard, _skills_guard, _, config) = create_temp_workspace();
        let mut state =
            TuiAppState::new(config, "session-123".to_string(), Some("run-1".to_string()));

        // 1. Test /quit
        state.chat.input_buffer = "/quit".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TuiUiAction::Quit));
        state.is_running = false;

        // 2. Test /mode confirm
        state.chat.input_buffer = "/mode confirm".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        if let TuiUiAction::ChangeMode(mode) = &actions[0] {
            assert_eq!(mode, "confirm");
        } else {
            panic!("Expected ChangeMode action");
        }
        state.is_running = false;

        // 3. Test /cost
        state.chat.input_buffer = "/cost".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TuiUiAction::CalculateCost));
        state.is_running = false;

        // 4. Test /help
        state.chat.input_buffer = "/help".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert!(actions.is_empty());
        assert_eq!(state.chrome.active_modal, TuiModal::Help);
        state.chrome.active_modal = TuiModal::None; // reset
        state.is_running = false;

        // 5. Test /runs when terminal width is wide (>= 80)
        state.chrome.terminal_width = 80;
        state.chat.input_buffer = "/runs".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TuiUiAction::LoadLineage));
        assert!(state.chrome.sidebar_open);
        assert_eq!(state.chrome.active_focus, TuiFocus::LineageTree);
        state.chrome.sidebar_open = false; // reset
        state.chrome.active_focus = TuiFocus::ChatPrompt; // reset
        state.is_running = false;

        // 6. Test /runs when terminal width is narrow (< 80)
        state.chrome.terminal_width = 79;
        state.chat.input_buffer = "/runs".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert!(actions.is_empty());
        assert!(!state.chrome.sidebar_open);
        assert_eq!(state.chrome.active_focus, TuiFocus::ChatPrompt);
        assert!(!state.chat.events.is_empty());
        if let Some(TranscriptEntry::Error(msg)) = state.chat.events.last() {
            assert!(msg.contains("Lineage tree sidebar is unavailable"));
        } else {
            panic!("Expected Error transcript entry");
        }
        state.chat.events.clear(); // reset
        state.is_running = false;

        // 7. Test /config
        state.chrome.details_open = false;
        state.chat.input_buffer = "/config".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert!(actions.is_empty());
        assert!(state.chrome.details_open);
        assert_eq!(state.chrome.active_focus, TuiFocus::Details);
        state.chrome.details_open = false; // reset
        state.chrome.active_focus = TuiFocus::ChatPrompt; // reset
        state.is_running = false;

        // 8. Test /context
        state.chat.input_buffer = "/context".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        if let TuiUiAction::ExplainContext(parent) = &actions[0] {
            assert_eq!(parent, "run-1");
        } else {
            panic!("Expected ExplainContext action");
        }
        state.is_running = false;

        // 9. Test /branch my new branch
        state.chat.input_buffer = "/branch my new branch".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        if let TuiUiAction::BranchSession {
            parent_run_id,
            prompt,
        } = &actions[0]
        {
            assert_eq!(parent_run_id, "run-1");
            assert_eq!(prompt, "my new branch");
        } else {
            panic!("Expected BranchSession action");
        }
        state.is_running = false;

        // 10. Test /export jsonl
        state.chat.input_buffer = "/export jsonl".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        if let TuiUiAction::ExportRun {
            parent_run_id,
            format,
        } = &actions[0]
        {
            assert_eq!(parent_run_id, "run-1");
            assert_eq!(format, "jsonl");
        } else {
            panic!("Expected ExportRun action");
        }
        state.is_running = false;

        // 11. Test /verify
        state.chat.input_buffer = "/verify".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        if let TuiUiAction::VerifyRun(parent) = &actions[0] {
            assert_eq!(parent, "run-1");
        } else {
            panic!("Expected VerifyRun action");
        }
    }

    #[test]
    fn test_approval_keys_approve_and_deny() {
        let (_env_guard, _xdg_guard, _skills_guard, _, config) = create_temp_workspace();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        state.chrome.active_modal = TuiModal::Approval;
        state.approval.active_request = Some(ApprovalRequest {
            tool_call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::Value::Null,
            decision: gestalt_core::PolicyDecision::allowed(None),
            description: "testing key approvals".to_string(),
        });

        // 1. Pressing 'a' should approve
        let actions = handle_key_event(&mut state, press_key(KeyCode::Char('a')));
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            TuiUiAction::ApprovalDecision {
                decision: ApprovalDecision::Approve
            }
        ));
        assert_eq!(state.chrome.active_modal, TuiModal::None);
        assert!(state.approval.active_request.is_none());

        // 2. Pressing 'y' should approve
        state.chrome.active_modal = TuiModal::Approval;
        state.approval.active_request = Some(ApprovalRequest {
            tool_call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::Value::Null,
            decision: gestalt_core::PolicyDecision::allowed(None),
            description: "testing key approvals".to_string(),
        });
        let actions = handle_key_event(&mut state, press_key(KeyCode::Char('y')));
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            TuiUiAction::ApprovalDecision {
                decision: ApprovalDecision::Approve
            }
        ));
        assert_eq!(state.chrome.active_modal, TuiModal::None);
        assert!(state.approval.active_request.is_none());

        // 3. Pressing 'd' should deny
        state.chrome.active_modal = TuiModal::Approval;
        state.approval.active_request = Some(ApprovalRequest {
            tool_call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::Value::Null,
            decision: gestalt_core::PolicyDecision::allowed(None),
            description: "testing key approvals".to_string(),
        });
        let actions = handle_key_event(&mut state, press_key(KeyCode::Char('d')));
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            TuiUiAction::ApprovalDecision {
                decision: ApprovalDecision::Deny
            }
        ));
        assert_eq!(state.chrome.active_modal, TuiModal::None);
        assert!(state.approval.active_request.is_none());

        // 4. Pressing Esc should deny
        state.chrome.active_modal = TuiModal::Approval;
        state.approval.active_request = Some(ApprovalRequest {
            tool_call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::Value::Null,
            decision: gestalt_core::PolicyDecision::allowed(None),
            description: "testing key approvals".to_string(),
        });
        let actions = handle_key_event(&mut state, press_key(KeyCode::Esc));
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            TuiUiAction::ApprovalDecision {
                decision: ApprovalDecision::Deny
            }
        ));
        assert_eq!(state.chrome.active_modal, TuiModal::None);
        assert!(state.approval.active_request.is_none());
    }

    #[test]
    fn test_slash_autocomplete_key_navigation() {
        let (_env_guard, _xdg_guard, _skills_guard, _, config) = create_temp_workspace();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);

        // Start typing a slash command
        state.chat.input_buffer = "/mo".to_string();

        // Down arrow should increment autocomplete_index
        assert_eq!(state.chat.autocomplete_index, 0);
        let actions = handle_key_event(&mut state, press_key(KeyCode::Down));
        assert!(actions.is_empty());
        assert_eq!(state.chat.autocomplete_index, 0);

        // Let's test with just "/" which matches all 11 commands.
        state.chat.input_buffer = "/".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Down));
        assert!(actions.is_empty());
        assert_eq!(state.chat.autocomplete_index, 1);

        let actions = handle_key_event(&mut state, press_key(KeyCode::Down));
        assert!(actions.is_empty());
        assert_eq!(state.chat.autocomplete_index, 2);

        let actions = handle_key_event(&mut state, press_key(KeyCode::Up));
        assert!(actions.is_empty());
        assert_eq!(state.chat.autocomplete_index, 1);

        // Pressing Tab completes the selected option.
        // Index 1 of the commands is "/new".
        let actions = handle_key_event(&mut state, press_key(KeyCode::Tab));
        assert!(actions.is_empty());
        assert_eq!(state.chat.input_buffer, "/new");
    }

    #[test]
    fn test_new_session_lifecycle_and_new_command() {
        let (_env_guard, _xdg_guard, _skills_guard, _, config) = create_temp_workspace();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);

        assert!(!state.has_started_session());
        assert_eq!(state.status, "Ready");

        state.parent_run_id = Some("run-1".to_string());
        state
            .chat
            .events
            .push(TranscriptEntry::User("existing chat".to_string()));
        assert!(state.has_started_session());

        state.chat.input_buffer = "/new".to_string();
        let actions = handle_key_event(&mut state, press_key(KeyCode::Enter));
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TuiUiAction::StartNewSession));

        let previous_session_id = state.session_id.clone();
        state.start_new_session();
        assert_ne!(state.session_id, previous_session_id);
        assert!(state.parent_run_id.is_none());
        assert!(state.chat.events.is_empty());
        assert!(!state.has_started_session());
        assert_eq!(state.status, "Ready");
    }

    #[test]
    fn test_notification_modal_dismisses_without_status_overflow() {
        let (_env_guard, _xdg_guard, _skills_guard, _, config) = create_temp_workspace();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        let long_error = "policy error: policy denied: Continue rejected: Run status is FailedWithCheckpoint. Only completed head runs can be continued.";

        state.status = "Failed. Ready for prompt.".to_string();
        state.show_notification("Run failed", long_error, true);

        assert_eq!(state.chrome.active_modal, TuiModal::Notification);
        assert_eq!(state.status, "Failed. Ready for prompt.");
        assert_eq!(
            state.notification.as_ref().map(|n| n.message.as_str()),
            Some(long_error)
        );

        let actions = handle_key_event(&mut state, press_key(KeyCode::Esc));
        assert!(actions.is_empty());
        assert_eq!(state.chrome.active_modal, TuiModal::None);
        assert!(state.notification.is_none());
    }

    #[test]
    fn test_active_run_interrupt_keys() {
        let (_env_guard, _xdg_guard, _skills_guard, _, config) = create_temp_workspace();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        state.is_running = true;

        // 1. Esc during active run should interrupt, not focus
        let actions = handle_key_event(&mut state, press_key(KeyCode::Esc));
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TuiUiAction::InterruptRun));

        // 2. Ctrl+C during active run should interrupt, not quit
        let actions = handle_key_event(&mut state, press_ctrl_c());
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TuiUiAction::InterruptRun));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_end_to_end_slash_handlers_integration() {
        let (_env_guard, _xdg_guard, _skills_guard, temp_dir, config) = create_temp_workspace();
        let runs_dir = temp_dir.join(".gestalt/runs");
        fs::create_dir_all(&runs_dir).unwrap();

        let run_1_dir = runs_dir.join("run-1");
        fs::create_dir_all(&run_1_dir).unwrap();
        let run_1_manifest = serde_json::json!({
            "v": 1,
            "session_id": "session-123",
            "run_id": "run-1",
            "parent_run_id": null,
            "base_checkpoint": null,
            "run_kind": "new",
            "created_at": "2026-06-04T12:00:00Z",
            "lifecycle_state": "completed",
            "finalized_at": "2026-06-04T12:05:00Z",
            "failure_kind": null,
            "interrupted_phase": null,
            "compatibility_fingerprint": {
                "context_pipeline_version": "1.0",
                "tool_schema_hash": "",
                "policy_fingerprint": "",
                "hook_contract_hash": "",
                "execution_mode": "confirm"
            }
        });
        fs::write(
            run_1_dir.join("run.json"),
            serde_json::to_string_pretty(&run_1_manifest).unwrap(),
        )
        .unwrap();
        fs::write(run_1_dir.join("trace.jsonl"), "{\"v\":1,\"session_id\":\"session-123\",\"run_id\":\"run-1\",\"turn_id\":0,\"seq\":0,\"ts\":\"2026-06-04T12:00:00Z\",\"event\":{\"type\":\"context_built\",\"packet_id\":\"\",\"token_estimate\":0},\"redacted\":false}\n").unwrap();

        // 1. Verify Mode change handler logic
        let mut state = TuiAppState::new(config.clone(), "session-123".to_string(), None);
        let mode = "yolo".to_string();
        if let Ok(parsed_mode) = gestalt_app::config::mode_from_str(&mode) {
            state.config.defaults.mode = Some(parsed_mode);
            state.details.config = Some(state.config.clone());
        }
        assert_eq!(
            state.config.defaults.mode,
            Some(gestalt_core::ExecutionMode::Yolo)
        );

        // 2. Verify Explain Context handler logic
        let overrides = CliOverrides {
            workspace: Some(temp_dir.clone()),
            ..CliOverrides::default()
        };
        let explain_res =
            gestalt_app::context::explain_context(&overrides, None, Some("run-1")).await;
        assert!(
            explain_res.is_ok(),
            "explain_context failed: {:?}",
            explain_res.err()
        );

        // 3. Verify Export Run handler logic
        let export_res = gestalt_tui::export::export_run(
            &config,
            "run-1",
            gestalt_tui::output::ExportFormat::Jsonl,
        );
        assert!(
            export_res.is_ok(),
            "export_run failed: {:?}",
            export_res.err()
        );

        // 4. Verify Verify Run handler logic
        let verify_res = gestalt_app::verify::verify_run(&config, "run-1").await;
        assert!(
            verify_res.is_ok(),
            "verify_run failed: {:?}",
            verify_res.err()
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
