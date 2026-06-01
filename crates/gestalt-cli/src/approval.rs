use std::io::{self, Write as _};

use gestalt_core::{ApprovalDecision, ApprovalProvider, ApprovalRequest};

#[derive(Debug, Default)]
pub struct CliApprovalProvider;

#[async_trait::async_trait]
impl ApprovalProvider for CliApprovalProvider {
    async fn approve(&self, request: ApprovalRequest) -> ApprovalDecision {
        print_request(&request);
        loop {
            print!("Approve? [y]es/[n]o/[e]dit/[a]lways: ");
            if io::stdout().flush().is_err() {
                return ApprovalDecision::Deny;
            }

            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_err() {
                return ApprovalDecision::Deny;
            }

            match input.trim() {
                "y" | "yes" => return ApprovalDecision::Approve,
                "n" | "no" => return ApprovalDecision::Deny,
                "a" | "always" => return ApprovalDecision::AlwaysAllowForSession,
                "e" | "edit" => {
                    println!("Enter replacement JSON input on one line:");
                    let mut edited = String::new();
                    if io::stdin().read_line(&mut edited).is_err() {
                        return ApprovalDecision::Deny;
                    }
                    match serde_json::from_str(edited.trim()) {
                        Ok(value) => return ApprovalDecision::Edit(value),
                        Err(err) => eprintln!("Invalid JSON: {err}"),
                    }
                }
                _ => eprintln!("Unknown response"),
            }
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
}
