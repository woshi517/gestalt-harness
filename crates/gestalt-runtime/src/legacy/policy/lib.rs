//! `gestalt-policy` — Policy engine + policies.toml parser
//!
//! This crate is part of the gestalt-harness workspace.
//! See the [architecture document](../../docs/gestalt-harness-architecture.md) for crate boundaries.

// Workspace lint configuration is inherited via Cargo.toml [lints] workspace = true

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gestalt_core::{
    event::PolicyStatus,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    session::ExecutionMode,
    tool::RiskLevel,
    PolicyError,
};
use glob::Pattern;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyConfig {
    pub paths: PathPolicy,
    pub bash: BashPolicy,
    pub network: NetworkPolicy,
    pub memory_paths: Vec<std::path::PathBuf>,
}

impl PolicyConfig {
    pub fn parse_toml(input: &str) -> Result<Self, PolicyError> {
        let raw = toml::from_str::<RawPolicyConfig>(input)
            .map_err(|err| PolicyError::InvalidPolicy(err.to_string()))?;
        Ok(raw.into_config())
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, PolicyError> {
        let input = std::fs::read_to_string(path)?;
        Self::parse_toml(&input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPolicy {
    pub allow_read: Vec<String>,
    pub allow_write: Vec<String>,
    pub deny_write: Vec<String>,
    pub deny_read: Vec<String>,
}

impl Default for PathPolicy {
    fn default() -> Self {
        Self {
            allow_read: vec![".".to_string()],
            allow_write: vec![
                "docs/".to_string(),
                "src/".to_string(),
                ".gestalt/".to_string(),
            ],
            deny_write: default_secret_patterns(),
            deny_read: default_secret_patterns(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashPolicy {
    pub default: PolicyAction,
    pub yolo_allow: Vec<String>,
    pub always_confirm: Vec<String>,
    pub always_deny: Vec<String>,
}

impl Default for BashPolicy {
    fn default() -> Self {
        Self {
            default: PolicyAction::Confirm,
            yolo_allow: vec![
                "cargo test".to_string(),
                "cargo check".to_string(),
                "cargo build".to_string(),
                "ls".to_string(),
                "grep".to_string(),
                "rg".to_string(),
                "find".to_string(),
                "cat".to_string(),
            ],
            always_confirm: vec![
                "rm".to_string(),
                "sudo".to_string(),
                "docker".to_string(),
                "git push".to_string(),
                "git reset".to_string(),
                "ssh".to_string(),
                "curl".to_string(),
                "wget".to_string(),
            ],
            always_deny: vec![
                "dd".to_string(),
                "mkfs".to_string(),
                "fdisk".to_string(),
                "chmod 777".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPolicy {
    pub default: PolicyAction,
    pub allow_domains: Vec<String>,
    pub deny_domains: Vec<String>,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            default: PolicyAction::Confirm,
            allow_domains: Vec::new(),
            deny_domains: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    Allow,
    Confirm,
    Deny,
}

impl PolicyAction {
    fn parse(input: Option<&str>, default: Self) -> Self {
        match input {
            Some("allow" | "allowed" | "auto") => Self::Allow,
            Some("deny" | "denied") => Self::Deny,
            _ => default,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawPolicyConfig {
    #[serde(default)]
    paths: RawPathPolicy,
    #[serde(default)]
    tools: RawToolsPolicy,
    #[serde(default)]
    network: RawNetworkPolicy,
}

impl RawPolicyConfig {
    fn into_config(self) -> PolicyConfig {
        let default_paths = PathPolicy::default();
        let default_bash = BashPolicy::default();

        PolicyConfig {
            paths: PathPolicy {
                allow_read: with_default(self.paths.allow_read, default_paths.allow_read),
                allow_write: with_default(self.paths.allow_write, default_paths.allow_write),
                deny_write: with_default(self.paths.deny_write, default_paths.deny_write),
                deny_read: default_paths.deny_read,
            },
            bash: BashPolicy {
                default: PolicyAction::parse(
                    self.tools.bash.default.as_deref(),
                    default_bash.default,
                ),
                yolo_allow: with_default(self.tools.bash.yolo_allow, default_bash.yolo_allow),
                always_confirm: with_default(
                    self.tools.bash.always_confirm,
                    default_bash.always_confirm,
                ),
                always_deny: with_default(self.tools.bash.always_deny, default_bash.always_deny),
            },
            network: NetworkPolicy {
                default: PolicyAction::parse(
                    self.network.default.as_deref(),
                    PolicyAction::Confirm,
                ),
                allow_domains: self.network.allow_domains.unwrap_or_default(),
                deny_domains: self.network.deny_domains.unwrap_or_default(),
            },
            memory_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawPathPolicy {
    allow_read: Option<Vec<String>>,
    allow_write: Option<Vec<String>>,
    deny_write: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct RawToolsPolicy {
    #[serde(default)]
    bash: RawBashPolicy,
}

#[derive(Debug, Default, Deserialize)]
struct RawBashPolicy {
    default: Option<String>,
    yolo_allow: Option<Vec<String>>,
    always_confirm: Option<Vec<String>>,
    always_deny: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct RawNetworkPolicy {
    default: Option<String>,
    allow_domains: Option<Vec<String>>,
    deny_domains: Option<Vec<String>>,
}

fn with_default<T>(value: Option<Vec<T>>, default: Vec<T>) -> Vec<T> {
    value.filter(|items| !items.is_empty()).unwrap_or(default)
}

fn default_secret_patterns() -> Vec<String> {
    vec![
        ".env".to_string(),
        ".env.*".to_string(),
        "*.key".to_string(),
        "*.pem".to_string(),
        "*secret*".to_string(),
        "*credential*".to_string(),
        ".git/".to_string(),
        "secrets/".to_string(),
    ]
}

#[derive(Debug, Clone)]
pub struct MinimalPolicyEngine {
    config: Arc<PolicyConfig>,
}

impl MinimalPolicyEngine {
    #[must_use]
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

impl Default for MinimalPolicyEngine {
    fn default() -> Self {
        Self::new(PolicyConfig::default())
    }
}

#[async_trait::async_trait]
impl PolicyEngine for MinimalPolicyEngine {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        if request.user_approved {
            return allow("user approved for session", "session:user_approved");
        }

        if let Some(decision) = self.evaluate_tool_policy(&request) {
            return decision;
        }

        matrix_decision(request.risk, request.mode, "default:risk_mode_matrix")
    }
}

impl MinimalPolicyEngine {
    fn evaluate_tool_policy(&self, request: &PolicyRequest) -> Option<PolicyDecision> {
        match request.tool_name.as_str() {
            "read" | "builtin:read" | "search" | "builtin:search" | "find_files"
            | "builtin:find_files" => Some(self.evaluate_path_tool(request, PathAccess::Read)),
            "write" | "builtin:write" | "patch" | "builtin:patch" => {
                Some(self.evaluate_path_tool(request, PathAccess::Write))
            }
            "bash" | "builtin:bash" => Some(self.evaluate_bash(request)),
            "web_fetch" | "builtin:web_fetch" => Some(self.evaluate_network(request)),
            _ => None,
        }
    }

    fn evaluate_path_tool(&self, request: &PolicyRequest, access: PathAccess) -> PolicyDecision {
        let path_buf;
        let path = if let Some(p) = extract_path(&request.input) {
            p
        } else {
            let name = request.tool_name.as_str();
            if name == "search"
                || name == "builtin:search"
                || name == "find_files"
                || name == "builtin:find_files"
            {
                path_buf = if let Some(ref ws_root) = request.workspace_root {
                    if let Ok(rel) = request.working_dir.strip_prefix(ws_root) {
                        let rel_str = rel.to_string_lossy().to_string();
                        if rel_str.is_empty() {
                            ".".to_string()
                        } else {
                            rel_str
                        }
                    } else {
                        request.working_dir.to_string_lossy().to_string()
                    }
                } else {
                    ".".to_string()
                };
                &path_buf
            } else {
                return deny(
                    "missing path in tool input",
                    "policies.toml:paths.invalid_input",
                );
            }
        };

        if access == PathAccess::Write {
            let path_obj = Path::new(path);
            let absolute_path = if path_obj.is_absolute() {
                path_obj.to_path_buf()
            } else {
                request.working_dir.join(path_obj)
            };
            let mut matches_memory = false;
            if let Ok(canonical_path) = absolute_path.canonicalize() {
                for mem_path in &self.config.memory_paths {
                    let canonical_mem = if mem_path.is_absolute() {
                        mem_path.clone()
                    } else if let Some(ref ws_root) = request.workspace_root {
                        ws_root.join(mem_path)
                    } else {
                        mem_path.clone()
                    };
                    if let Ok(canonical_mem) = canonical_mem.canonicalize() {
                        if canonical_path == canonical_mem {
                            matches_memory = true;
                            break;
                        }
                    } else if canonical_path.ends_with(mem_path)
                        || absolute_path.ends_with(mem_path)
                    {
                        matches_memory = true;
                        break;
                    }
                }
            } else {
                for mem_path in &self.config.memory_paths {
                    if absolute_path.ends_with(mem_path) || path_obj.ends_with(mem_path) {
                        matches_memory = true;
                        break;
                    }
                }
            }
            if matches_memory {
                return confirm(
                    format!("direct write to memory file is restricted; use memory proposal instead: {}", path),
                    "policies.toml:paths.memory_write_restricted",
                );
            }
        }

        if is_denied_secret_path(path) || self.path_matches_deny(path, access) {
            return deny(
                format!("path denied by policy: {path}"),
                "policies.toml:paths.deny",
            );
        }

        if !self.path_matches_allow(path, access) {
            return deny(
                format!("path not allowlisted: {path}"),
                "policies.toml:paths.allow",
            );
        }

        matrix_decision(request.risk, request.mode, "policies.toml:paths.allow")
    }

    fn path_matches_deny(&self, path: &str, access: PathAccess) -> bool {
        let patterns = match access {
            PathAccess::Read => &self.config.paths.deny_read,
            PathAccess::Write => &self.config.paths.deny_write,
        };
        matches_any_path(patterns, path)
    }

    fn path_matches_allow(&self, path: &str, access: PathAccess) -> bool {
        let patterns = match access {
            PathAccess::Read => &self.config.paths.allow_read,
            PathAccess::Write => &self.config.paths.allow_write,
        };
        matches_any_path(patterns, path)
    }

    fn evaluate_bash(&self, request: &PolicyRequest) -> PolicyDecision {
        let command = request
            .input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if matches_command(&self.config.bash.always_deny, command) {
            return deny(
                format!("bash command denied: {command}"),
                "policies.toml:tools.bash.always_deny",
            );
        }
        if matches_command(&self.config.bash.always_confirm, command) {
            return confirm(
                format!("bash command requires confirmation: {command}"),
                "policies.toml:tools.bash.always_confirm",
            );
        }

        if is_secret_command(command) {
            return deny(
                format!("bash command accesses secret paths: {command}"),
                "policies.toml:tools.bash.secret_paths",
            );
        }

        if request.mode == ExecutionMode::Yolo
            && matches_command(&self.config.bash.yolo_allow, command)
            && request.risk <= RiskLevel::Medium
        {
            return allow(
                format!("bash command yolo-allowlisted: {command}"),
                "policies.toml:tools.bash.yolo_allow",
            );
        }

        if request.mode == ExecutionMode::Yolo {
            return confirm(
                format!("bash command not yolo-allowlisted: {command}"),
                "policies.toml:tools.bash.default_confirm",
            );
        }

        match self.config.bash.default {
            PolicyAction::Allow => allow("bash default allow", "policies.toml:tools.bash.default"),
            PolicyAction::Confirm => matrix_decision(
                request.risk,
                request.mode,
                "policies.toml:tools.bash.default",
            ),
            PolicyAction::Deny => deny("bash default deny", "policies.toml:tools.bash.default"),
        }
    }

    fn evaluate_network(&self, request: &PolicyRequest) -> PolicyDecision {
        let host = request
            .input
            .get("url")
            .and_then(Value::as_str)
            .and_then(|url| url::Url::parse(url).ok())
            .and_then(|url| url.host_str().map(ToOwned::to_owned));

        let Some(host) = host else {
            return deny(
                "invalid or missing web_fetch URL",
                "policies.toml:network.invalid_input",
            );
        };

        if self
            .config
            .network
            .deny_domains
            .iter()
            .any(|domain| domain == &host)
        {
            return deny(
                format!("network domain denied: {host}"),
                "policies.toml:network.deny_domains",
            );
        }
        if self
            .config
            .network
            .allow_domains
            .iter()
            .any(|domain| domain == &host)
        {
            return matrix_decision(
                request.risk,
                request.mode,
                "policies.toml:network.allow_domains",
            );
        }

        match self.config.network.default {
            PolicyAction::Allow => {
                matrix_decision(request.risk, request.mode, "policies.toml:network.default")
            }
            PolicyAction::Confirm => confirm(
                format!("network access requires confirmation: {host}"),
                "policies.toml:network.default",
            ),
            PolicyAction::Deny => deny(
                format!("network domain not allowed: {host}"),
                "policies.toml:network.default",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathAccess {
    Read,
    Write,
}

pub fn classify_bash(command: &str) -> RiskLevel {
    let normalized = normalize_command(command);

    if normalized.contains("rm -rf /")
        || normalized.contains("mkfs")
        || normalized.contains("dd if=")
        || normalized.contains(":(){")
        || normalized.contains("chmod 777")
    {
        return RiskLevel::Critical;
    }

    if is_secret_command(command) {
        return RiskLevel::High;
    }

    if has_shell_metacharacters(&normalized)
        || normalized.contains("/dev/tcp")
        || normalized.contains("/dev/udp")
        || normalized.contains("python -c")
        || normalized.contains("python3 -c")
        || normalized.contains("sh -c")
        || normalized.contains("bash -c")
        || starts_with_any(&normalized, &["env", "xargs", "sudo -u"])
        || normalized.contains(" env ")
        || normalized.contains(" xargs ")
        || normalized.contains(" sudo -u ")
    {
        return RiskLevel::High;
    }

    if starts_with_any(
        &normalized,
        &["sudo", "docker", "git push", "ssh", "curl", "wget"],
    ) {
        return RiskLevel::High;
    }

    if starts_with_any(
        &normalized,
        &[
            "rm",
            "mv",
            "cp",
            "mkdir",
            "cargo install",
            "npm install",
            "pnpm install",
            "yarn add",
            "pip install",
        ],
    ) {
        return RiskLevel::Medium;
    }

    if starts_with_any(
        &normalized,
        &[
            "ls",
            "cat",
            "grep",
            "rg",
            "find",
            "cargo check",
            "git status",
            "git diff",
        ],
    ) {
        return RiskLevel::Low;
    }

    RiskLevel::Medium
}

fn is_secret_command(command: &str) -> bool {
    command.split_whitespace().any(|token| {
        let token = token
            .trim_matches(|c| c == '\'' || c == '"')
            .to_ascii_lowercase();
        token.contains(".env")
            || ends_with_ignore_ascii_case(&token, ".key")
            || ends_with_ignore_ascii_case(&token, ".pem")
            || token.starts_with("secrets/")
            || token.contains("/secrets/")
            || token.contains("/secret/")
            || token.starts_with("secret.")
            || ends_with_ignore_ascii_case(&token, ".secret")
    })
}

fn ends_with_ignore_ascii_case(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len() && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn has_shell_metacharacters(command: &str) -> bool {
    command.chars().any(|ch| {
        matches!(
            ch,
            '>' | '<' | '|' | '&' | ';' | '`' | '$' | '\\' | '\n' | '\r'
        )
    })
}

fn normalize_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn starts_with_any(command: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|prefix| command == *prefix || command.starts_with(&format!("{prefix} ")))
}

fn matches_command(patterns: &[String], command: &str) -> bool {
    let normalized = normalize_command(command);
    patterns.iter().any(|pattern| {
        normalized == *pattern
            || normalized.starts_with(&format!("{pattern} "))
            || normalized.contains(&format!(" {pattern} "))
    })
}

fn extract_path(input: &Value) -> Option<&str> {
    input
        .get("path")
        .or_else(|| input.get("cwd"))
        .and_then(Value::as_str)
}

fn is_denied_secret_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == ".env"
        || lower.ends_with("/.env")
        || lower.contains("/.env.")
        || has_extension(&lower, "key")
        || has_extension(&lower, "pem")
        || lower.contains("secret")
        || lower.contains("credential")
}

fn matches_any_path(patterns: &[String], path: &str) -> bool {
    let normalized = normalize_path(path);
    patterns.iter().any(|pattern| {
        let pattern = normalize_path(pattern);
        if pattern == "." {
            return true;
        }
        if pattern.ends_with('/') {
            return normalized.starts_with(&pattern);
        }
        if Pattern::new(&pattern).is_ok_and(|glob| glob.matches(&normalized))
            || normalized == pattern
            || normalized.starts_with(&format!("{pattern}/"))
        {
            return true;
        }

        // Handle file-shaped glob patterns for directory-scoped paths
        // Extract the prefix before any glob wildcard character
        let wildcard_pos = pattern.find(['*', '?', '[']);
        if let Some(pos) = wildcard_pos {
            let prefix = &pattern[..pos];
            let mut prefix_path = normalize_path(prefix);
            if prefix_path.ends_with('/') {
                prefix_path.pop();
            }
            if !prefix_path.is_empty() {
                if normalized == prefix_path || normalized.starts_with(&format!("{prefix_path}/")) {
                    return true;
                }
            }
        }

        false
    })
}

fn has_extension(path: &str, extension: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

fn normalize_path(path: &str) -> String {
    PathBuf::from(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn matrix_decision(risk: RiskLevel, mode: ExecutionMode, source: &str) -> PolicyDecision {
    match (risk, mode) {
        (RiskLevel::Low, ExecutionMode::Confirm | ExecutionMode::Yolo) => {
            allow("low-risk tool call", source)
        }
        (RiskLevel::Medium, ExecutionMode::Confirm) => confirm("medium-risk tool call", source),
        (RiskLevel::Medium, ExecutionMode::Yolo) => {
            allow("medium-risk policy-allowed call", source)
        }
        (RiskLevel::High, ExecutionMode::Confirm | ExecutionMode::Yolo) => {
            confirm("high-risk tool call", source)
        }
        (RiskLevel::Critical, ExecutionMode::Confirm | ExecutionMode::Yolo) => {
            deny("critical-risk tool call", source)
        }
        (_, ExecutionMode::Human) => deny("human mode proposes only", source),
        (_, ExecutionMode::DryRun) => deny("dry-run mode plans only", source),
        (_, ExecutionMode::Replay) => deny("replay mode does not execute live tools", source),
    }
}

fn allow(reason: impl Into<String>, source: impl Into<String>) -> PolicyDecision {
    PolicyDecision {
        status: PolicyStatus::Allowed,
        reason: Some(reason.into()),
        policy_source: source.into(),
    }
}

fn confirm(reason: impl Into<String>, source: impl Into<String>) -> PolicyDecision {
    PolicyDecision {
        status: PolicyStatus::Confirm,
        reason: Some(reason.into()),
        policy_source: source.into(),
    }
}

fn deny(reason: impl Into<String>, source: impl Into<String>) -> PolicyDecision {
    PolicyDecision {
        status: PolicyStatus::Denied,
        reason: Some(reason.into()),
        policy_source: source.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(
        tool_name: &str,
        input: Value,
        risk: RiskLevel,
        mode: ExecutionMode,
    ) -> PolicyRequest {
        PolicyRequest {
            tool_call_id: "call-1".to_string(),
            tool_name: tool_name.to_string(),
            namespace: gestalt_core::tool_descriptor::ToolNamespace::BuiltIn,
            annotations: gestalt_core::tool_descriptor::ToolAnnotations::default(),
            input,
            risk,
            mode,
            working_dir: PathBuf::from("/workspace"),
            workspace_root: Some(PathBuf::from("/workspace")),
            user_approved: false,
        }
    }

    #[test]
    fn policy_config_should_parse_minimal_toml() {
        let config = PolicyConfig::parse_toml(
            r#"
            [paths]
            allow_read = ["."]
            allow_write = ["docs/"]

            [tools.bash]
            default = "confirm"
            yolo_allow = ["cargo test"]

            [network]
            default = "deny"
            allow_domains = ["docs.rs"]
            "#,
        )
        .expect("policy parses");

        assert_eq!(config.network.default, PolicyAction::Deny);
    }

    #[test]
    fn bash_classifier_should_detect_critical_commands() {
        assert_eq!(classify_bash("rm -rf /"), RiskLevel::Critical);
    }

    #[test]
    fn bash_classifier_should_treat_unknown_as_medium() {
        assert_eq!(classify_bash("custom-command --flag"), RiskLevel::Medium);
    }

    #[tokio::test]
    async fn policy_should_apply_allow_deny_precedence() {
        let engine = MinimalPolicyEngine::default();
        let decision = engine
            .evaluate(request(
                "write",
                json!({"path": ".env", "content": "secret"}),
                RiskLevel::Medium,
                ExecutionMode::Yolo,
            ))
            .await;

        assert_eq!(decision.status, PolicyStatus::Denied);
    }

    #[tokio::test]
    async fn policy_should_route_modes_by_risk() {
        let engine = MinimalPolicyEngine::default();
        let decision = engine
            .evaluate(request(
                "write",
                json!({"path": "docs/a.md", "content": "x"}),
                RiskLevel::Medium,
                ExecutionMode::Confirm,
            ))
            .await;

        assert_eq!(decision.status, PolicyStatus::Confirm);
    }

    #[tokio::test]
    async fn policy_should_report_source() {
        let engine = MinimalPolicyEngine::default();
        let decision = engine
            .evaluate(request(
                "read",
                json!({"path": "README.md"}),
                RiskLevel::Low,
                ExecutionMode::Confirm,
            ))
            .await;

        assert!(!decision.policy_source.is_empty());
    }

    #[tokio::test]
    async fn policy_should_respect_session_approval() {
        let engine = MinimalPolicyEngine::default();
        let mut request = request(
            "bash",
            json!({"command": "docker ps"}),
            RiskLevel::High,
            ExecutionMode::Confirm,
        );
        request.user_approved = true;

        let decision = engine.evaluate(request).await;

        assert_eq!(decision.status, PolicyStatus::Allowed);
    }

    #[test]
    fn bash_classifier_should_detect_wrappers_and_metacharacters_and_secrets() {
        assert_eq!(
            classify_bash("python -c 'import sys; print(sys.version)'"),
            RiskLevel::High
        );
        assert_eq!(classify_bash("cat foo.txt | grep bar"), RiskLevel::High);
        assert_eq!(classify_bash("cat foo.txt ; ls"), RiskLevel::High);
        assert_eq!(classify_bash("cat .env.local"), RiskLevel::High);
    }

    #[tokio::test]
    async fn policy_should_deny_bash_secret_paths() {
        let engine = MinimalPolicyEngine::default();
        let decision = engine
            .evaluate(request(
                "bash",
                json!({"command": "cat .env.local"}),
                RiskLevel::High,
                ExecutionMode::Yolo,
            ))
            .await;

        assert_eq!(decision.status, PolicyStatus::Denied);
        assert!(decision.reason.as_ref().unwrap().contains("secret"));
    }

    #[tokio::test]
    async fn policy_should_confirm_non_allowlisted_bash_in_yolo() {
        let engine = MinimalPolicyEngine::default();
        let decision = engine
            .evaluate(request(
                "bash",
                json!({"command": "some-random-script.sh"}),
                RiskLevel::Medium,
                ExecutionMode::Yolo,
            ))
            .await;

        assert_eq!(decision.status, PolicyStatus::Confirm);
    }

    #[test]
    fn test_matches_any_path_file_shaped_globs() {
        let patterns = vec!["src/**/*.rs".to_string(), "crates/lib/src/*.rs".to_string()];

        assert!(matches_any_path(&patterns, "src/main.rs"));
        assert!(matches_any_path(&patterns, "crates/lib/src/lib.rs"));

        assert!(matches_any_path(&patterns, "src"));
        assert!(matches_any_path(&patterns, "src/utils"));
        assert!(matches_any_path(&patterns, "crates/lib/src"));

        assert!(!matches_any_path(&patterns, "."));
        assert!(!matches_any_path(&patterns, "crates/lib"));
        assert!(!matches_any_path(&patterns, "tests"));
    }
}
