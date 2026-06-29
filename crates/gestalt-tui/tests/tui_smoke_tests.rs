mod tests {
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::mpsc;

    use gestalt_core::{
        approval::{ApprovalDecision, ApprovalProvider, ApprovalRequest},
        cancel::CancelToken,
        error::HarnessError,
    };

    use gestalt_tui::tui::approval::TuiApprovalProvider;
    use gestalt_tui::tui::bridge::{
        get_diagnostics_logs, init_diagnostics_buffer, TuiBridgeMessage, TuiLogLayer,
    };

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

    #[tokio::test]
    async fn test_tui_log_layer_rolling() {
        // Initialize with max lines = 100
        init_diagnostics_buffer(100);

        use tracing_subscriber::prelude::*;
        let subscriber = tracing_subscriber::registry().with(TuiLogLayer);

        // Run tracing commands under a local dispatcher to avoid interfering with global state
        let dispatcher = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatcher, || {
            for i in 0..150 {
                tracing::info!("test log message {}", i);
            }
        });

        let logs = get_diagnostics_logs();
        // Should roll over and keep exactly 100 logs
        assert_eq!(logs.len(), 100);
        // The first elements should be the rolled elements (50 to 149)
        assert!(
            logs[0].contains("test log message 50"),
            "First log was: {}",
            logs[0]
        );
        assert!(
            logs[99].contains("test log message 149"),
            "Last log was: {}",
            logs[99]
        );
    }

    #[tokio::test]
    async fn test_tui_approval_provider_success() {
        let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel();
        let provider = TuiApprovalProvider::new(bridge_tx);

        let request = ApprovalRequest {
            tool_call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::json!({"arg": 42}),
            decision: gestalt_core::PolicyDecision::allowed(None),
            description: "Test run approval".to_string(),
        };

        // Simulate user decision in UI loop
        let handle = tokio::spawn(async move {
            if let Some(TuiBridgeMessage::ApprovalRequest {
                request,
                response_tx,
            }) = bridge_rx.recv().await
            {
                assert_eq!(request.tool_name, "test_tool");
                let _ = response_tx.send(ApprovalDecision::Approve);
            }
        });

        let decision = provider.approve(request).await.unwrap();
        assert_eq!(decision, ApprovalDecision::Approve);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tui_approval_provider_cancel_via_token() {
        let (bridge_tx, _bridge_rx) = mpsc::unbounded_channel();
        let provider = TuiApprovalProvider::new(bridge_tx);

        let request = ApprovalRequest {
            tool_call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::json!({"arg": 42}),
            decision: gestalt_core::PolicyDecision::allowed(None),
            description: "Test run approval".to_string(),
        };

        let cancel_token = CancelToken::new();
        let cancel_token_clone = cancel_token.clone();

        // Cancel token after 50ms
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_token_clone.cancel();
        });

        let res = provider.approve_cancellable(request, &cancel_token).await;
        assert!(matches!(res, Err(HarnessError::Cancelled)));
    }

    #[tokio::test]
    async fn test_tui_terminal_restore_on_early_error() {
        use gestalt_app::config::{validate_workspace_config, CliOverrides};
        use gestalt_tui::run_tui;

        let env_guard = ENV_MUTEX.lock().unwrap();
        let temp_dir =
            std::env::temp_dir().join(format!("gestalt-tui-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", temp_dir.as_os_str().to_str().unwrap());
        let skills_guard = EnvVarGuard::set("GESTALT_NO_GLOBAL_SKILLS", "1");
        let gestalt_dir = temp_dir.join(".gestalt");
        std::fs::create_dir_all(&gestalt_dir).unwrap();
        std::fs::write(
            gestalt_dir.join("config.toml"),
            "[defaults]\nprovider = \"mock\"\n",
        )
        .unwrap();
        std::fs::write(gestalt_dir.join("policies.toml"), "[policies]\n").unwrap();

        let config = validate_workspace_config(&CliOverrides {
            workspace: Some(temp_dir.clone()),
            ..CliOverrides::default()
        })
        .unwrap();

        drop((env_guard, xdg_guard, skills_guard));

        let res = run_tui(
            &config,
            Some("nonexistent-run-123".to_string()),
            None,
            None,
            CancelToken::new(),
        )
        .await;
        assert!(res.is_err());

        // Programmatically verify that raw mode was successfully disabled.
        // This validates that TerminalGuard's RAII Drop implementation successfully executed
        // on the early return/error path.
        let raw_mode_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap();
        assert!(
            !raw_mode_enabled,
            "Raw mode should be disabled after run_tui returns an error"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
