use std::io::{self, Write as _};

use gestalt_core::{hash_input_short, ApprovalDecision, ApprovalProvider, ApprovalRequest};

#[derive(Debug, Default)]
pub struct CliApprovalProvider;

#[async_trait::async_trait]
impl ApprovalProvider for CliApprovalProvider {
    async fn approve(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalDecision, gestalt_core::HarnessError> {
        let cancel = gestalt_core::cancel::CancelToken::new();
        self.approve_cancellable(request, &cancel).await
    }

    async fn approve_cancellable(
        &self,
        request: ApprovalRequest,
        cancel_token: &gestalt_core::cancel::CancelToken,
    ) -> Result<ApprovalDecision, gestalt_core::HarnessError> {
        print_request(&request);
        let mut reader = tokio::io::BufReader::new(tokio::io::stdin());

        let fut = async {
            use tokio::io::AsyncBufReadExt;
            loop {
                print!("Approve? [y]es/[n]o/[e]dit/[a]lways: ");
                if io::stdout().flush().is_err() {
                    return ApprovalDecision::Deny;
                }

                let mut input = String::new();
                match reader.read_line(&mut input).await {
                    Ok(0) => return ApprovalDecision::Deny,
                    Ok(_) => {}
                    Err(_) => return ApprovalDecision::Deny,
                }

                match input.trim() {
                    "y" | "yes" => {
                        return ApprovalDecision::Approve;
                    }
                    "n" | "no" => {
                        return ApprovalDecision::Deny;
                    }
                    "a" | "always" => {
                        print_session_grant_notice(&request);
                        return ApprovalDecision::AlwaysAllowForSession;
                    }
                    "e" | "edit" => {
                        println!("Enter replacement JSON input on one line:");
                        let mut edited = String::new();
                        match reader.read_line(&mut edited).await {
                            Ok(0) => return ApprovalDecision::Deny,
                            Ok(_) => {}
                            Err(_) => return ApprovalDecision::Deny,
                        }
                        match serde_json::from_str(edited.trim()) {
                            Ok(value) => {
                                println!(
                                    "Note: the edited input is re-evaluated by policy. \
                                     'always' is only honoured if the edited input is auto-allowed."
                                );
                                return ApprovalDecision::Edit(value);
                            }
                            Err(err) => eprintln!("Invalid JSON: {err}"),
                        }
                    }
                    _ => eprintln!("Unknown response"),
                }
            }
        };

        tokio::select! {
            res = fut => Ok(res),
            _ = cancel_token.cancelled() => Err(gestalt_core::HarnessError::Cancelled),
        }
    }
}

fn print_request(request: &ApprovalRequest) {
    println!("Tool requires approval: {}", request.tool_name);
    println!("Call id: {}", request.tool_call_id);
    println!(
        "Reason: {}",
        request.decision.reason.as_deref().unwrap_or("none")
    );
    println!("Source: {}", request.decision.policy_source);
    println!("Description: {}", request.description);
    println!("Input: {}", request.input);
    println!("Input fingerprint: {}", hash_input_short(&request.input));
}

fn print_session_grant_notice(request: &ApprovalRequest) {
    println!(
        "Session grant recorded for {} (matched rule: {}, fingerprint: {}). \
         The grant is bounded to the same input hash and the same risk ceiling; \
         policy is re-run on every future call and riskier calls are denied.",
        request.tool_name,
        request.decision.policy_source,
        hash_input_short(&request.input)
    );
}

pub struct CliInteractionProvider;

impl gestalt_app::InteractionProvider for CliInteractionProvider {
    fn prompt_password(&self, prompt: &str) -> Option<String> {
        println!("{}", prompt);
        rpassword::read_password()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn confirm(&self, prompt: &str) -> bool {
        print!("{} [y/N]: ", prompt);
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            let trimmed = input.trim().to_lowercase();
            trimmed == "y" || trimmed == "yes"
        } else {
            false
        }
    }
}
