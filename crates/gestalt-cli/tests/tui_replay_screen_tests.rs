#[cfg(feature = "tui")]
mod tests {
    use gestalt_cli::config::EffectiveConfig;
    use gestalt_cli::tui::screens::chat::draw_chat_screen;
    use gestalt_cli::tui::services::{LineageNode, LineageTreeModel};
    use gestalt_cli::tui::state::{TuiAppState, TuiFocus, TuiModal};
    use gestalt_cli::tui::widgets::status_bar::draw_status_bar;

    use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};
    use std::path::PathBuf;

    fn buffer_to_string(buffer: &Buffer) -> String {
        let mut res = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                let sym = cell.symbol();
                if sym.is_empty() {
                    res.push(' ');
                } else {
                    res.push_str(sym);
                }
            }
            res.push('\n');
        }
        res
    }

    fn create_dummy_config() -> EffectiveConfig {
        EffectiveConfig {
            workspace_root: PathBuf::from("/dummy"),
            config_path: PathBuf::from("/dummy/gestalt.json"),
            defaults: Default::default(),
            tools: Default::default(),
            context: Default::default(),
            observe: Default::default(),
            providers: Default::default(),
            profiles: Default::default(),
            prompt: Default::default(),
            policies: Default::default(),
            provider_override: None,
            model_override: None,
            tui: Default::default(),
            extensions: Default::default(),
            skills: Default::default(),
        }
    }

    #[test]
    fn test_status_bar_rendering() {
        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_status_bar(
                    f,
                    Rect::new(0, 0, 80, 3),
                    Rect::new(0, 3, 80, 3),
                    "Idle",
                    false,
                    "hello user",
                    TuiFocus::ChatPrompt,
                    false,
                    80,
                    0,
                    "confirm",
                    "Untitled Session",
                    true,
                );
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());
        assert!(rendered.contains("System Status"));
        assert!(rendered.contains("Idle"));
        assert!(rendered.contains("hello user"));
        assert!(rendered.contains("Prompt Input"));
    }

    #[test]
    fn test_fresh_tui_shows_new_session_mode() {
        let config = create_dummy_config();
        let state = TuiAppState::new(config, "session-123".to_string(), None);
        let backend = TestBackend::new(100, 15);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_chat_screen(f, &state);
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());
        assert!(rendered.contains("NEW SESSION MODE"));
        assert!(rendered.contains("New session"));
        assert!(!rendered.contains("CONTINUE CHAT MODE"));
    }

    #[test]
    fn test_viewport_width_policy_narrow() {
        let config = create_dummy_config();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        state.chrome.sidebar_open = true; // Request sidebar
        state.chrome.terminal_width = 79; // Narrow viewport
        state.chrome.terminal_height = 15;

        // Load dummy lineage model
        state.lineage.model = Some(LineageTreeModel {
            session_id: "session-123".to_string(),
            nodes: vec![LineageNode {
                run_id: "run-1".to_string(),
                parent_run_id: None,
                created_at: chrono::Utc::now(),
                lifecycle_state: "completed".to_string(),
                turns: 1,
                depth: 0,
                is_last_child: true,
                prefix: String::new(),
            }],
        });

        let backend = TestBackend::new(79, 15);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_chat_screen(f, &state);
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());

        // Lineage tree should be hidden in narrow view (< 80 columns)
        assert!(!rendered.contains("Session Lineage"));
        // Status bar should hint lineage unavailable
        assert!(rendered.contains("lineage unavailable"));
    }

    #[test]
    fn test_viewport_width_policy_wide() {
        let config = create_dummy_config();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        state.chrome.sidebar_open = true; // Request sidebar
        state.chrome.terminal_width = 100; // Wide viewport
        state.chrome.terminal_height = 15;

        state.lineage.model = Some(LineageTreeModel {
            session_id: "session-123".to_string(),
            nodes: vec![LineageNode {
                run_id: "run-1".to_string(),
                parent_run_id: None,
                created_at: chrono::Utc::now(),
                lifecycle_state: "completed".to_string(),
                turns: 1,
                depth: 0,
                is_last_child: true,
                prefix: String::new(),
            }],
        });

        let backend = TestBackend::new(100, 15);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_chat_screen(f, &state);
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());

        // Lineage tree should be visible in wide view (>= 80 columns)
        assert!(rendered.contains("Session Lineage"));
        assert!(rendered.contains("run-1"));
    }

    #[test]
    fn test_help_modal_rendering() {
        let config = create_dummy_config();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        state.chrome.active_modal = TuiModal::Help;
        state.chrome.terminal_width = 80;
        state.chrome.terminal_height = 20;

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_chat_screen(f, &state);
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());

        // Help modal elements must render
        assert!(rendered.contains("Keyboard Shortcuts Guide"));
        assert!(rendered.contains("F1 / Ctrl+H"));
        assert!(rendered.contains("F2 / Ctrl+S"));
        assert!(rendered.contains("F3 / Ctrl+O"));
        assert!(rendered.contains("F4 / Ctrl+L"));
    }

    #[test]
    fn test_logo_splash_rendering() {
        let config = create_dummy_config();
        let state = TuiAppState::new(config, "session-123".to_string(), None);

        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_chat_screen(f, &state);
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());

        // Center splash screen showing ASCII art should contain block symbols or portions of the logo
        assert!(rendered.contains("█"));
    }

    #[test]
    fn test_golden_snapshot_layout() {
        let config = create_dummy_config();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        state.chrome.sidebar_open = true;
        state.chrome.terminal_width = 90;
        state.chrome.terminal_height = 20;

        state.lineage.model = Some(LineageTreeModel {
            session_id: "session-123".to_string(),
            nodes: vec![
                LineageNode {
                    run_id: "run-root".to_string(),
                    parent_run_id: None,
                    created_at: chrono::Utc::now(),
                    lifecycle_state: "completed".to_string(),
                    turns: 2,
                    depth: 0,
                    is_last_child: true,
                    prefix: String::new(),
                },
                LineageNode {
                    run_id: "run-child".to_string(),
                    parent_run_id: Some("run-root".to_string()),
                    created_at: chrono::Utc::now(),
                    lifecycle_state: "running".to_string(),
                    turns: 0,
                    depth: 1,
                    is_last_child: true,
                    prefix: "   ".to_string(),
                },
            ],
        });

        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_chat_screen(f, &state);
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());

        // Golden snapshot test using insta
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn test_slash_command_chatbox_expansion() {
        let config = create_dummy_config();
        let mut state = TuiAppState::new(config, "session-123".to_string(), None);
        state.chat.input_buffer = "/".to_string();
        state.chrome.terminal_width = 80;
        state.chrome.terminal_height = 20;

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw_chat_screen(f, &state);
            })
            .unwrap();

        let rendered = buffer_to_string(terminal.backend().buffer());

        assert!(rendered.contains("Prompt Input"));
        assert!(rendered.contains("/help"));
        assert!(rendered.contains("/quit"));
        assert!(rendered.contains("/mode"));
        assert!(rendered.contains("Show keyboard shortcuts and general help"));
    }
}
