use gestalt_core::AgentEvent;

pub fn render_event(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::UserMessage { content } => Some(format!("user> {content}")),
        AgentEvent::ContextBuilt { token_estimate, .. } => {
            Some(format!("context> {token_estimate} tokens"))
        }
        AgentEvent::ModelRequest { provider, model } => Some(format!("model> {provider}/{model}")),
        AgentEvent::Text { delta } => Some(format!("assistant> {delta}")),
        AgentEvent::Thinking { delta } => Some(format!("thinking> {delta}")),
        AgentEvent::ToolCallStreamed { .. } => None,
        AgentEvent::ToolCallProposed { id, name, input } => Some(format!("tool> {name}#{id} {input}")),
        AgentEvent::PolicyDecision {
            tool_call_id,
            decision,
            reason,
            policy_source,
        } => Some(format!(
            "policy> {tool_call_id} {decision:?} source={policy_source} {}",
            reason.clone().unwrap_or_default()
        )),
        AgentEvent::ToolResult {
            id,
            output,
            is_error,
            truncated,
        } => Some(format!(
            "tool-result> {id} error={is_error} truncated={truncated} {output}"
        )),
        AgentEvent::MemoryProposal { diff } => Some(format!("memory> {diff}")),
        AgentEvent::VerificationResult { report, .. } => report.clone(),
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => Some(format!("usage> in={input_tokens} out={output_tokens}")),
        AgentEvent::Stop { reason } => Some(format!("stop> {reason:?}")),
        AgentEvent::Error {
            message,
            recoverable,
        } => Some(format!("error> recoverable={recoverable} {message}")),
    }
}
