use std::{
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use encoding_rs::Encoding;
use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};
use gestalt_exec::{ExecRequest, ExecutionSandbox, NetworkPolicy, NoSandbox};
use glob::Pattern;
use schemars::{schema_for, JsonSchema};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::path::{validate_child_dir, validate_existing_path, validate_write_path};

const DEFAULT_MAX_TOKENS: usize = 4_000;
const WEB_RESPONSE_CAP_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BashInput {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadInput {
    pub path: String,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteInput {
    pub path: String,
    pub content: String,
    #[serde(default = "default_true")]
    pub show_diff: bool,
    #[serde(default = "default_true")]
    pub create_dirs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PatchInput {
    pub path: String,
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchInput {
    pub url: String,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub raw: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchInput {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub case_insensitive: Option<bool>,
    #[serde(default)]
    pub max_results: Option<usize>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReadTool;

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a workspace file with optional line-range and output limits."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<ReadInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<ReadInput>(self.name(), input)?;
        let path = validate_existing_path(&input.path, ctx)?;
        let bytes = std::fs::read(&path).map_err(ToolError::ExecutionFailed)?;
        let content = decode_text(self.name(), &bytes)?;
        let selected = select_line_range(&content, input.start_line, input.end_line)?;
        let output = limit_tokens(&selected, input.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS));
        Ok(ToolOutput::Text { content: output })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchTool;

#[async_trait::async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search workspace text files with local, path-scoped semantics."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<SearchInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<SearchInput>(self.name(), input)?;
        let root = validate_child_dir(input.path.as_deref(), ctx)?;
        let glob = input
            .file_glob
            .as_deref()
            .map(Pattern::new)
            .transpose()
            .map_err(|err| invalid_input(self.name(), err.to_string()))?;
        let needle = if input.case_insensitive.unwrap_or(false) {
            input.pattern.to_ascii_lowercase()
        } else {
            input.pattern.clone()
        };
        let max_results = input.max_results.unwrap_or(100);
        let mut results = Vec::new();
        search_dir(
            &root,
            &root,
            glob.as_ref(),
            &needle,
            input.case_insensitive.unwrap_or(false),
            max_results,
            &mut results,
        )?;

        Ok(ToolOutput::Text {
            content: results.join("\n"),
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write full replacement content to a workspace file."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<WriteInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<WriteInput>(self.name(), input)?;
        let path = validate_write_path(&input.path, ctx)?;
        let parent = path
            .parent()
            .ok_or_else(|| invalid_input(self.name(), "write path has no parent"))?;
        if !parent.exists() {
            if input.create_dirs {
                std::fs::create_dir_all(parent).map_err(ToolError::ExecutionFailed)?;
            } else {
                return Err(invalid_input(
                    self.name(),
                    "parent directory does not exist",
                ));
            }
        }

        let old = if path.exists() {
            std::fs::read_to_string(&path).map_err(ToolError::ExecutionFailed)?
        } else {
            String::new()
        };
        std::fs::write(&path, input.content.as_bytes()).map_err(ToolError::ExecutionFailed)?;

        let diff = if input.show_diff {
            make_diff(&input.path, &old, &input.content)
        } else {
            String::new()
        };
        Ok(ToolOutput::Text {
            content: json!({
                "path": input.path,
                "bytes_written": input.content.len(),
                "diff": diff,
            })
            .to_string(),
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PatchTool;

#[async_trait::async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a workspace file."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<PatchInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<PatchInput>(self.name(), input)?;
        let path = validate_existing_path(&input.path, ctx)?;
        let old = std::fs::read_to_string(&path).map_err(ToolError::ExecutionFailed)?;
        let patched = apply_unified_patch(&old, &input.patch)?;
        std::fs::write(&path, patched.as_bytes()).map_err(ToolError::ExecutionFailed)?;
        Ok(ToolOutput::Text {
            content: format!("patch applied: {}", input.path),
        })
    }
}

#[derive(Clone)]
pub struct BashTool {
    sandbox: Arc<dyn ExecutionSandbox>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            sandbox: Arc::new(NoSandbox),
        }
    }
}

impl BashTool {
    #[must_use]
    pub fn new(sandbox: Arc<dyn ExecutionSandbox>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command as a fresh subprocess."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<BashInput>(self.name(), self.description())
    }

    fn risk(&self, input: &Value) -> RiskLevel {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        classify_bash(command)
    }

    fn can_run_in_parallel(&self, input: &Value) -> bool {
        self.risk(input) == RiskLevel::Low
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<BashInput>(self.name(), input)?;
        let working_dir = validate_child_dir(input.cwd.as_deref(), ctx)?;
        let result = self
            .sandbox
            .run(ExecRequest {
                command: vec!["bash".to_string(), "-lc".to_string(), input.command],
                working_dir,
                workspace_root: ctx.workspace_root.clone(),
                env: ctx.environment.clone(),
                timeout: input.timeout_secs.map_or(ctx.timeout, Duration::from_secs),
                max_output_bytes: ctx.max_output_bytes,
                network_policy: if ctx.allow_network {
                    NetworkPolicy::Full
                } else {
                    NetworkPolicy::None
                },
                mounts: Vec::new(),
                artifact_dir: ctx.artifact_dir.clone(),
                tool_call_id: ctx.current_tool_call_id.clone(),
            })
            .await
            .map_err(|err| match err {
                gestalt_core::HarnessError::Tool(err) => err,
                other => ToolError::InvalidInput {
                    tool_name: self.name().to_string(),
                    reason: other.to_string(),
                },
            })?;

        Ok(ToolOutput::Text {
            content: result.combined_text(),
        })
    }
}

#[derive(Clone, Default)]
pub struct WebFetchTool;

#[async_trait::async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch an HTTP(S) URL and return untrusted markdown-like content."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<WebFetchInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if !ctx.allow_network {
            return Err(ToolError::NetworkDenied(
                "network disabled in tool context".to_string(),
            ));
        }

        let input = parse_input::<WebFetchInput>(self.name(), input)?;
        let url = validate_public_http_url(&input.url).await?;
        let (response, redirects) = fetch_with_redirects(url).await?;
        let final_url = response.url().to_string();

        if response
            .content_length()
            .is_some_and(|length| length > WEB_RESPONSE_CAP_BYTES as u64)
        {
            return Err(ToolError::OutputTooLarge {
                tool_name: self.name().to_string(),
                limit: WEB_RESPONSE_CAP_BYTES,
            });
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|err| invalid_input(self.name(), err.to_string()))?;
        if bytes.len() > WEB_RESPONSE_CAP_BYTES {
            return Err(ToolError::OutputTooLarge {
                tool_name: self.name().to_string(),
                limit: WEB_RESPONSE_CAP_BYTES,
            });
        }

        let text = decode_text(self.name(), &bytes)?;
        let body = if input.raw {
            text
        } else {
            html_to_markdownish(&text)
        };
        let body = limit_tokens(&body, input.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS));
        let redirect_header = if redirects.is_empty() {
            String::new()
        } else {
            format!(" redirects=\"{}\"", redirects.join(" -> "))
        };
        Ok(ToolOutput::Text {
            content: format!(
                "<source id=\"{final_url}\" trust=\"external_untrusted\"{redirect_header}>\n{body}\n</source>"
            ),
        })
    }
}

fn tool_schema<T>(name: &str, description: &str) -> ToolSchema
where
    T: JsonSchema,
{
    json!({
        "name": name,
        "description": description,
        "input_schema": serde_json::to_value(schema_for!(T)).unwrap_or(Value::Null),
    })
}

fn parse_input<T>(tool_name: &str, input: Value) -> Result<T, ToolError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput {
        tool_name: tool_name.to_string(),
        reason: err.to_string(),
    })
}

fn invalid_input(tool_name: &str, reason: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool_name: tool_name.to_string(),
        reason: reason.into(),
    }
}

fn decode_text(tool_name: &str, bytes: &[u8]) -> Result<String, ToolError> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_string());
    }

    let (encoding, bom_len) = Encoding::for_bom(bytes).unwrap_or((encoding_rs::UTF_8, 0));
    let (text, _, had_errors) = encoding.decode(&bytes[bom_len..]);
    if had_errors {
        return Err(invalid_input(tool_name, "file is not valid text"));
    }
    Ok(text.into_owned())
}

fn select_line_range(
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, ToolError> {
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or(usize::MAX);
    if start == 0 || end < start {
        return Err(invalid_input("read", "invalid line range"));
    }

    let selected = content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_no = index.saturating_add(1);
            (line_no >= start && line_no <= end).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(selected)
}

fn limit_tokens(content: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(4);
    if content.chars().count() <= max_chars {
        return content.to_string();
    }

    let truncated = content.chars().take(max_chars).collect::<String>();
    format!(
        "{truncated}\n[Output truncated. Original: {} bytes.]",
        content.len()
    )
}

fn search_dir(
    root: &Path,
    current: &Path,
    glob: Option<&Pattern>,
    needle: &str,
    case_insensitive: bool,
    max_results: usize,
    results: &mut Vec<String>,
) -> Result<(), ToolError> {
    if results.len() >= max_results {
        return Ok(());
    }

    let entries = std::fs::read_dir(current).map_err(ToolError::ExecutionFailed)?;
    for entry in entries {
        let entry = entry.map_err(ToolError::ExecutionFailed)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(ToolError::ExecutionFailed)?;
        if file_type.is_dir() {
            search_dir(
                root,
                &path,
                glob,
                needle,
                case_insensitive,
                max_results,
                results,
            )?;
        } else if file_type.is_file() && glob_matches(root, &path, glob) {
            search_file(root, &path, needle, case_insensitive, max_results, results)?;
        }
        if results.len() >= max_results {
            break;
        }
    }
    Ok(())
}

fn glob_matches(root: &Path, path: &Path, glob: Option<&Pattern>) -> bool {
    glob.map_or(true, |pattern| {
        path.strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .is_some_and(|relative| pattern.matches(relative))
    })
}

fn search_file(
    root: &Path,
    path: &Path,
    needle: &str,
    case_insensitive: bool,
    max_results: usize,
    results: &mut Vec<String>,
) -> Result<(), ToolError> {
    let bytes = std::fs::read(path).map_err(ToolError::ExecutionFailed)?;
    let Ok(content) = decode_text("search", &bytes) else {
        return Ok(());
    };
    let relative = path.strip_prefix(root).unwrap_or(path);
    for (line_index, line) in content.lines().enumerate() {
        let haystack = if case_insensitive {
            line.to_ascii_lowercase()
        } else {
            line.to_string()
        };
        if haystack.contains(needle) {
            results.push(format!(
                "{}:{}:{}",
                relative.display(),
                line_index + 1,
                line
            ));
        }
        if results.len() >= max_results {
            break;
        }
    }
    Ok(())
}

fn make_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let mut diff = format!("--- {path}\n+++ {path}\n");
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn apply_unified_patch(original: &str, patch: &str) -> Result<String, ToolError> {
    let mut lines = original.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let patch_lines = patch.lines().collect::<Vec<_>>();
    let mut index = 0;

    while index < patch_lines.len() {
        let line = patch_lines[index];
        if !line.starts_with("@@") {
            index += 1;
            continue;
        }

        let old_start = parse_hunk_start(line)?;
        let mut cursor = old_start.saturating_sub(1);
        index += 1;

        while index < patch_lines.len() && !patch_lines[index].starts_with("@@") {
            let patch_line = patch_lines[index];
            if patch_line.starts_with("---") || patch_line.starts_with("+++") {
                index += 1;
                continue;
            }
            let (prefix, value) = patch_line.split_at(1);
            match prefix {
                " " => {
                    ensure_line_matches(&lines, cursor, value)?;
                    cursor = cursor.saturating_add(1);
                }
                "-" => {
                    ensure_line_matches(&lines, cursor, value)?;
                    lines.remove(cursor);
                }
                "+" => {
                    lines.insert(cursor, value.to_string());
                    cursor = cursor.saturating_add(1);
                }
                _ => return Err(invalid_input("patch", "invalid unified diff line")),
            }
            index += 1;
        }
    }

    let mut output = lines.join("\n");
    if original.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn parse_hunk_start(header: &str) -> Result<usize, ToolError> {
    let start = header
        .split_whitespace()
        .find(|part| part.starts_with('-'))
        .ok_or_else(|| invalid_input("patch", "missing hunk header"))?;
    let number = start
        .trim_start_matches('-')
        .split(',')
        .next()
        .unwrap_or_default();
    number
        .parse::<usize>()
        .map_err(|err| invalid_input("patch", err.to_string()))
}

fn ensure_line_matches(lines: &[String], cursor: usize, expected: &str) -> Result<(), ToolError> {
    if lines.get(cursor).is_some_and(|line| line == expected) {
        return Ok(());
    }
    Err(invalid_input("patch", "patch context mismatch"))
}

fn classify_bash(command: &str) -> RiskLevel {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
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

fn starts_with_any(command: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|prefix| command == *prefix || command.starts_with(&format!("{prefix} ")))
}

async fn validate_public_http_url(input: &str) -> Result<Url, ToolError> {
    let url = Url::parse(input).map_err(|err| invalid_input("web_fetch", err.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(ToolError::NetworkDenied(format!(
                "unsupported scheme: {scheme}"
            )))
        }
    }

    let host = url
        .host_str()
        .ok_or_else(|| invalid_input("web_fetch", "URL must include host"))?;
    if host == "localhost" {
        return Err(ToolError::NetworkDenied("localhost denied".to_string()));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        reject_private_ip(ip)?;
        return Ok(url);
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
    for addr in addrs {
        reject_private_ip(addr.ip())?;
    }
    Ok(url)
}

async fn fetch_with_redirects(mut url: Url) -> Result<(reqwest::Response, Vec<String>), ToolError> {
    let mut redirects = Vec::new();
    for _ in 0..10 {
        let client = pinned_client_for(&url).await?;
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
        if !response.status().is_redirection() {
            return Ok((response, redirects));
        }

        let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
            return Err(invalid_input(
                "web_fetch",
                "redirect missing Location header",
            ));
        };
        let location = location
            .to_str()
            .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
        let next_url = url
            .join(location)
            .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
        validate_public_http_url(next_url.as_str()).await?;
        redirects.push(format!("{url} -> {next_url}"));
        url = next_url;
    }

    Err(invalid_input("web_fetch", "too many redirects"))
}

async fn pinned_client_for(url: &Url) -> Result<reqwest::Client, ToolError> {
    let url = validate_public_http_url(url.as_str()).await?;
    let host = url
        .host_str()
        .ok_or_else(|| invalid_input("web_fetch", "URL must include host"))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addr = resolve_public_socket_addr(host, port).await?;

    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, addr)
        .build()
        .map_err(|err| invalid_input("web_fetch", err.to_string()))
}

async fn resolve_public_socket_addr(host: &str, port: u16) -> Result<SocketAddr, ToolError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        reject_private_ip(ip)?;
        return Ok(SocketAddr::new(ip, port));
    }

    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| invalid_input("web_fetch", err.to_string()))?;
    if let Some(addr) = addrs.next() {
        reject_private_ip(addr.ip())?;
        return Ok(addr);
    }

    Err(ToolError::NetworkDenied(format!(
        "no public addresses resolved for host: {host}"
    )))
}

fn reject_private_ip(ip: IpAddr) -> Result<(), ToolError> {
    let denied = match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.octets()[0] == 0
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.segments()[0] & 0xfe00 == 0xfc00
                || ip.segments()[0] & 0xffc0 == 0xfe80
        }
    };

    if denied {
        return Err(ToolError::NetworkDenied(format!("private IP denied: {ip}")));
    }
    Ok(())
}

fn html_to_markdownish(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                output.push(' ');
            }
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn default_registry() -> Result<crate::ToolRegistry, ToolError> {
    let mut registry = crate::ToolRegistry::new();
    registry.register(Arc::new(ReadTool))?;
    registry.register(Arc::new(SearchTool))?;
    registry.register(Arc::new(WriteTool))?;
    registry.register(Arc::new(PatchTool))?;
    registry.register(Arc::new(BashTool::default()))?;
    registry.register(Arc::new(WebFetchTool::default()))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::{ToolCatalog, ToolContext};
    use serde_json::json;
    use std::{collections::HashMap, fs, path::PathBuf};

    fn temp_workspace(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("gestalt-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp workspace");
        root
    }

    fn ctx(root: &Path) -> ToolContext {
        ToolContext {
            working_dir: root.to_path_buf(),
            workspace_root: Some(root.to_path_buf()),
            timeout: Duration::from_secs(2),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 128,
            artifact_dir: None,
            current_tool_call_id: None,
        }
    }

    #[test]
    fn schemas_should_include_public_contracts() {
        let registry = default_registry().expect("registry builds");
        let schemas = registry.schemas();

        assert_eq!(schemas.len(), 6);
    }

    #[tokio::test]
    async fn read_should_honor_line_ranges() {
        let root = temp_workspace("read-range");
        fs::write(root.join("file.txt"), "one\ntwo\nthree\n").expect("write fixture");

        let output = ReadTool
            .execute(
                json!({"path": "file.txt", "start_line": 2, "end_line": 2}),
                &ctx(&root),
            )
            .await
            .expect("read succeeds");

        assert_eq!(
            output,
            ToolOutput::Text {
                content: "two".to_string()
            }
        );
    }

    #[tokio::test]
    async fn read_should_reject_path_traversal() {
        let root = temp_workspace("read-traversal");
        let result = ReadTool
            .execute(json!({"path": "../outside.txt"}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::ExecutionFailed(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_should_reject_symlink_escape() {
        let root = temp_workspace("read-symlink");
        let outside = temp_workspace("outside");
        fs::write(outside.join("secret.txt"), "secret").expect("write outside");
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt"))
            .expect("create symlink");

        let result = ReadTool
            .execute(json!({"path": "link.txt"}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::PathNotAllowed(_))));
    }

    #[tokio::test]
    async fn search_should_find_matches_with_glob() {
        let root = temp_workspace("search");
        fs::write(root.join("a.md"), "Alpha\nBeta").expect("write md");
        fs::write(root.join("a.txt"), "Alpha").expect("write txt");

        let output = SearchTool
            .execute(
                json!({"pattern": "alpha", "file_glob": "*.md", "case_insensitive": true}),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        assert!(matches!(output, ToolOutput::Text { content } if content == "a.md:1:Alpha"));
    }

    #[tokio::test]
    async fn write_should_create_parent_dirs_and_return_diff() {
        let root = temp_workspace("write");
        let output = WriteTool
            .execute(
                json!({"path": "docs/a.md", "content": "new\n", "show_diff": true}),
                &ctx(&root),
            )
            .await
            .expect("write succeeds");

        assert!(root.join("docs/a.md").exists());
        assert!(matches!(output, ToolOutput::Text { content } if content.contains("\"diff\"")));
    }

    #[tokio::test]
    async fn write_should_fail_when_parent_missing_and_create_dirs_false() {
        let root = temp_workspace("write-no-dirs");
        let result = WriteTool
            .execute(
                json!({"path": "docs/a.md", "content": "new\n", "create_dirs": false}),
                &ctx(&root),
            )
            .await;

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn patch_should_apply_unified_diff() {
        let root = temp_workspace("patch");
        fs::write(root.join("a.txt"), "one\ntwo\nthree\n").expect("write fixture");
        let patch = "--- a.txt\n+++ a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("patch succeeds");

        assert_eq!(
            fs::read_to_string(root.join("a.txt")).expect("read patched"),
            "one\nTWO\nthree\n"
        );
    }

    #[tokio::test]
    async fn patch_should_fail_on_context_mismatch() {
        let root = temp_workspace("patch-fail");
        fs::write(root.join("a.txt"), "one\ntwo\n").expect("write fixture");
        let patch = "--- a.txt\n+++ a.txt\n@@ -1,2 +1,2 @@\n missing\n-two\n+TWO";

        let result = PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn bash_should_use_subprocess_timeout_and_output_cap() {
        let root = temp_workspace("bash");
        let mut ctx = ctx(&root);
        ctx.max_output_bytes = 4;
        let output = BashTool::default()
            .execute(json!({"command": "printf 123456789"}), &ctx)
            .await
            .expect("bash succeeds");

        assert!(
            matches!(output, ToolOutput::Text { content } if content.contains("truncated: true"))
        );
    }

    #[test]
    fn bash_should_treat_shell_metacharacters_as_high_risk() {
        assert_eq!(
            BashTool::default().risk(&json!({"command": "cat foo.txt ; ls"})),
            RiskLevel::High
        );
        assert_eq!(
            BashTool::default().risk(&json!({"command": "cat /dev/tcp/127.0.0.1/80"})),
            RiskLevel::High
        );
        assert_eq!(
            BashTool::default().risk(&json!({"command": "grep secret docs.md"})),
            RiskLevel::Low
        );
    }

    #[tokio::test]
    async fn bash_should_restrict_cwd() {
        let root = temp_workspace("bash-cwd");
        let result = BashTool::default()
            .execute(json!({"command": "pwd", "cwd": "/tmp"}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::PathNotAllowed(_))));
    }

    #[tokio::test]
    async fn web_fetch_should_reject_non_http_scheme() {
        let root = temp_workspace("web-scheme");
        let mut ctx = ctx(&root);
        ctx.allow_network = true;
        let result = WebFetchTool::default()
            .execute(json!({"url": "file:///etc/passwd"}), &ctx)
            .await;

        assert!(matches!(result, Err(ToolError::NetworkDenied(_))));
    }

    #[tokio::test]
    async fn web_fetch_should_reject_private_ip() {
        let result = validate_public_http_url("http://127.0.0.1").await;

        assert!(matches!(result, Err(ToolError::NetworkDenied(_))));
    }

    #[test]
    fn html_extraction_should_strip_tags() {
        assert_eq!(
            html_to_markdownish("<h1>Hello</h1><p>world</p>"),
            "Hello world"
        );
    }
}
