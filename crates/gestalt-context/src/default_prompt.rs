pub const DEFAULT_SYSTEM_PROMPT: &str = "\
# Identity
You are the gestalt-harness local agent, a powerful AI assistant designed for local workspace task execution.

# Environment
You run in a local shell execution environment with access to a workspace root, an execution mode, turn limits, and a specific catalog of tools.

# Tool-Use Policy
Read-only tools may run in parallel. Writing tools and network calls are subject to policies and will prompt for human confirmation by default unless pre-approved or allowlisted.

# Output Rules
Be concise. Always quote exact file paths and line numbers when referencing codebase elements. Never claim a tool result you did not see in the environment.
";

pub fn get_default_prompt(
    workspace_root: Option<&std::path::Path>,
    mode: Option<&str>,
    max_turns: Option<usize>,
    available_tools: Option<&[String]>,
) -> String {
    let root_str = workspace_root.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "unknown".to_string());
    let mode_str = mode.unwrap_or("confirm");
    let turns_str = max_turns.map(|t| t.to_string()).unwrap_or_else(|| "unlimited".to_string());
    let tools_str = available_tools.map(|t| t.join(", ")).unwrap_or_else(|| "none".to_string());

    format!(
        "# Identity\n\
         You are the gestalt-harness local agent, a powerful AI assistant designed for local workspace task execution.\n\n\
         # Environment\n\
         - Workspace root: {}\n\
         - Execution mode: {}\n\
         - Max turns: {}\n\
         - Available tools: {}\n\n\
         # Tool-Use Policy\n\
         Read-only tools may run in parallel. Writing tools and network calls are subject to policies and will prompt for human confirmation by default unless pre-approved or allowlisted.\n\n\
         # Output Rules\n\
         Be concise. Always quote exact file paths and line numbers when referencing codebase elements. Never claim a tool result you did not see in the environment.\n",
        root_str, mode_str, turns_str, tools_str
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_system_prompt_non_empty() {
        assert!(!DEFAULT_SYSTEM_PROMPT.is_empty());
        assert!(DEFAULT_SYSTEM_PROMPT.contains("gestalt-harness local agent"));
        assert!(DEFAULT_SYSTEM_PROMPT.contains("# Identity"));
        assert!(DEFAULT_SYSTEM_PROMPT.contains("# Environment"));
        assert!(DEFAULT_SYSTEM_PROMPT.contains("# Tool-Use Policy"));
        assert!(DEFAULT_SYSTEM_PROMPT.contains("# Output Rules"));
    }

    #[test]
    fn test_get_default_prompt_interpolation() {
        let root = std::path::Path::new("/tmp/test-ws");
        let prompt = get_default_prompt(
            Some(root),
            Some("yolo"),
            Some(10),
            Some(&["read".to_string(), "write".to_string()]),
        );
        assert!(prompt.contains("- Workspace root: /tmp/test-ws"));
        assert!(prompt.contains("- Execution mode: yolo"));
        assert!(prompt.contains("- Max turns: 10"));
        assert!(prompt.contains("- Available tools: read, write"));
    }
}
