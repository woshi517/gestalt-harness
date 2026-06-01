use gestalt_core::AgentEvent;

pub fn render_event(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::UserMessage { content } => Some(format!("user> {content}")),
        AgentEvent::ContextBuilt {
            token_estimate,
            packet_hash,
            ..
        } => {
            let mut extra = String::new();
            if let Some(h) = packet_hash {
                extra.push_str(&format!(" packet_hash={}", &h[..8.min(h.len())]));
            }
            Some(format!("context> {token_estimate} tokens{extra}"))
        }
        AgentEvent::ModelRequest {
            provider,
            model,
            packet_hash,
            temperature,
            max_tokens,
            provider_request_hash,
            ..
        } => {
            let mut extra = String::new();
            if let Some(h) = packet_hash {
                extra.push_str(&format!(" packet_hash={}", &h[..8.min(h.len())]));
            }
            if let Some(t) = temperature {
                extra.push_str(&format!(" temp={}", t));
            }
            if let Some(m) = max_tokens {
                extra.push_str(&format!(" max_tokens={}", m));
            }
            if let Some(h) = provider_request_hash {
                extra.push_str(&format!(" request_hash={}", &h[..8.min(h.len())]));
            }
            Some(format!("model> {provider}/{model}{extra}"))
        }
        AgentEvent::Text { delta } => Some(format!("assistant> {delta}")),
        AgentEvent::Thinking { delta } => Some(format!("thinking> {delta}")),
        AgentEvent::ToolCallStreamed { .. } => None,
        AgentEvent::ToolCallProposed { id, name, input } => {
            Some(format!("tool> {name}#{id} {input}"))
        }
        AgentEvent::PolicyDecision {
            tool_call_id,
            tool_name,
            input_hash,
            risk,
            mode,
            matched_rule,
            decision,
            reason,
            policy_source,
        } => {
            let mut extra = String::new();
            if let Some(name) = tool_name {
                extra.push_str(&format!(" tool={name}"));
            }
            if let Some(level) = risk {
                extra.push_str(&format!(" risk={level:?}"));
            }
            if let Some(m) = mode {
                extra.push_str(&format!(" mode={m:?}"));
            }
            if let Some(rule) = matched_rule {
                extra.push_str(&format!(" rule={rule}"));
            }
            if let Some(hash) = input_hash {
                extra.push_str(&format!(" input={}", &hash[..8.min(hash.len())]));
            }
            Some(format!(
                "policy> {tool_call_id} {decision:?} source={policy_source}{extra} {}",
                reason.clone().unwrap_or_default()
            ))
        }
        AgentEvent::ApprovalDecision {
            tool_call_id,
            decision,
            original_input_hash,
            edited_input_hash,
            grant_terms,
        } => {
            let short = |s: &str| s.chars().take(8).collect::<String>();
            let grant = grant_terms
                .as_ref()
                .map(|g| format!(" grant={}#{}", g.tool_name, short(&g.input_hash)))
                .unwrap_or_default();
            let edited = edited_input_hash
                .as_ref()
                .map(|h| format!(" edited={}", short(h)))
                .unwrap_or_default();
            Some(format!(
                "approval> {tool_call_id} {decision:?} orig={}{}{}",
                short(original_input_hash),
                edited,
                grant
            ))
        }
        AgentEvent::ToolResult {
            id,
            output,
            is_error,
            truncated,
            tool_name,
            working_dir,
            duration_ms,
            output_hash,
            artifact_refs,
            policy_source,
        } => {
            let mut extra = String::new();
            if let Some(name) = tool_name {
                extra.push_str(&format!(" name={}", name));
            }
            if let Some(dir) = working_dir {
                extra.push_str(&format!(" dir={}", dir));
            }
            if let Some(ms) = duration_ms {
                extra.push_str(&format!(" duration={}ms", ms));
            }
            if let Some(h) = output_hash {
                extra.push_str(&format!(" hash={}", &h[..8.min(h.len())]));
            }
            if let Some(refs) = artifact_refs {
                if !refs.is_empty() {
                    extra.push_str(&format!(" artifacts={}", refs.join(",")));
                }
            }
            if let Some(src) = policy_source {
                extra.push_str(&format!(" policy_source={}", src));
            }
            Some(format!(
                "tool-result> {id} error={is_error} truncated={truncated}{extra} {output}"
            ))
        }
        AgentEvent::ArtifactCreated {
            path,
            size_bytes,
            mime_type,
            hash,
        } => Some(format!(
            "artifact-created> {path} size={size_bytes} mime={mime_type} hash={}",
            &hash[..8.min(hash.len())]
        )),
        AgentEvent::PolicyViolation {
            tool_call_id,
            tool_name,
            reason,
        } => Some(format!(
            "policy-violation> {tool_call_id} tool={tool_name} reason={reason}"
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
