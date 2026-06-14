use serde::{Deserialize, Serialize};

use gestalt_core::ContextStability;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub manifest_version: Option<String>,
    #[serde(default)]
    pub protocol_version: Option<String>,
    pub runtime: String, // e.g., "stdio"
    pub entrypoint: Entrypoint,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub tools: Vec<ToolDeclaration>,
    #[serde(default)]
    pub hooks: Vec<HookDeclaration>,
    #[serde(default)]
    pub context_injectors: Vec<ContextInjectorDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entrypoint {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub hooks: bool,
    #[serde(default)]
    pub context: bool,
    #[serde(default)]
    pub supports_cancellation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Permissions {
    #[serde(default)]
    pub allow_network: Vec<String>,
    #[serde(default)]
    pub allow_workspace_read: bool,
    #[serde(default)]
    pub allow_workspace_write: bool,
    #[serde(default)]
    pub allow_shell: bool,
    #[serde(default)]
    pub allow_all_paths: bool,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub risk: Option<String>,
    /// Extension-declared `read_only` hint. This is *advisory* and
    /// must be downgraded to `ExtensionDeclared` annotation source
    /// in the descriptor; only `BuiltInTrusted` should ever be able
    /// to enable automatic retry.
    #[serde(default)]
    pub read_only: Option<bool>,
    /// Extension-declared `idempotent` hint. Same trust caveats as
    /// `read_only`: surfaced for visibility, but not blindly trusted.
    #[serde(default)]
    pub idempotent: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookDeclaration {
    pub name: String,
    pub lifecycle_point: String,
    #[serde(default)]
    pub failure_mode: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextInjectorDeclaration {
    pub name: String,
    #[serde(default)]
    pub stability: Option<ContextStability>,
}

impl ExtensionManifest {
    pub fn parse(content: &str) -> std::result::Result<Self, String> {
        toml::from_str(content).map_err(|e| format!("TOML parse error: {}", e))
    }

    pub fn validate(&self, _deny_unknown_permissions: bool) -> std::result::Result<(), String> {
        if self.id.is_empty() || self.id.len() > 64 {
            return Err("Extension ID must be between 1 and 64 characters".to_string());
        }
        let mut chars = self.id.chars();
        let Some(first) = chars.next() else {
            return Err("Extension ID cannot be empty".to_string());
        };
        if !first.is_ascii_lowercase() {
            return Err("Extension ID must start with a lowercase letter".to_string());
        }
        for c in chars {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '.' && c != '-' {
                return Err(format!("Extension ID '{}' contains invalid characters. Only lowercase alphanumeric, dots, and hyphens are allowed.", self.id));
            }
        }
        if self.id.starts_with("gestalt") || self.id.starts_with("harness") {
            return Err(format!("Extension ID '{}' starts with a reserved namespace ('gestalt' or 'harness')", self.id));
        }

        if self.name.trim().is_empty() {
            return Err("Extension Name cannot be empty".to_string());
        }
        if self.runtime != "stdio" {
            return Err(format!(
                "Unsupported runtime: '{}'. Only 'stdio' is supported in MVP",
                self.runtime
            ));
        }
        if self.entrypoint.command.trim().is_empty() {
            return Err("Entrypoint command cannot be empty".to_string());
        }

        if let Some(ref mv) = self.manifest_version {
            if mv.trim().is_empty() {
                return Err("manifest_version cannot be empty".to_string());
            }
        }
        if let Some(ref pv) = self.protocol_version {
            if pv.trim().is_empty() {
                return Err("protocol_version cannot be empty".to_string());
            }
        }

        if self.manifest_version.is_none() || self.protocol_version.is_none() {
            eprintln!("Warning: Extension '{}' manifest is missing manifest_version or protocol_version. Falling back to protocol 1.0 compatibility mode.", self.id);
        }

        // Validate duplicates
        let mut seen_tools = std::collections::HashSet::new();
        for tool in &self.tools {
            if !seen_tools.insert(&tool.name) {
                return Err(format!("Duplicate tool name '{}' declared in manifest", tool.name));
            }
            if tool.description.trim().is_empty() {
                return Err(format!("Tool '{}' must have a non-empty description", tool.name));
            }
            if let Some(ref risk) = tool.risk {
                match risk.as_str() {
                    "low" | "medium" | "high" | "critical" => {}
                    other => return Err(format!("Invalid risk level '{}' for tool '{}'", other, tool.name)),
                }
            }
            if !tool.input_schema.is_object() {
                return Err(format!("Tool '{}' input_schema must be a valid JSON Schema object", tool.name));
            }
        }

        let mut seen_hooks = std::collections::HashSet::new();
        for hook in &self.hooks {
            if !seen_hooks.insert(&hook.name) {
                return Err(format!("Duplicate hook name '{}' declared in manifest", hook.name));
            }
            match hook.lifecycle_point.as_str() {
                "before_context_build" | "after_context_build" | "before_tool_policy" | "after_tool_result" | "prepare_next_turn" | "on_event" => {}
                other => return Err(format!("Invalid lifecycle point '{}' for hook '{}'", other, hook.name)),
            }
        }

        let mut seen_injectors = std::collections::HashSet::new();
        for inj in &self.context_injectors {
            if !seen_injectors.insert(&inj.name) {
                return Err(format!("Duplicate context injector name '{}' declared in manifest", inj.name));
            }
        }

        // Capability check: tools declared but capabilities.tools is false
        if !self.tools.is_empty() && !self.capabilities.tools {
            return Err("Extension declares tools but capabilities.tools is false".to_string());
        }

        // Capability check: hooks declared but capabilities.hooks is false
        if !self.hooks.is_empty() && !self.capabilities.hooks {
            return Err("Extension declares hooks but capabilities.hooks is false".to_string());
        }

        // Capability check: context injectors declared but capabilities.context is false
        if !self.context_injectors.is_empty() && !self.capabilities.context {
            return Err(
                "Extension declares context injectors but capabilities.context is false"
                    .to_string(),
            );
        }

        if let Some(injector) = self
            .context_injectors
            .iter()
            .find(|injector| injector.stability.is_none())
        {
            return Err(format!(
                "Context injector '{}' must declare stability",
                injector.name
            ));
        }

        for host in &self.permissions.allow_network {
            if host.trim().is_empty() {
                return Err("allow_network contains empty host".to_string());
            }
        }
        for path in &self.permissions.allowed_paths {
            if path.trim().is_empty() {
                return Err("allowed_paths contains empty path".to_string());
            }
        }

        validate_shell_entrypoint(&self.entrypoint, self.permissions.allow_shell)?;

        Ok(())
    }
}

pub(crate) fn validate_shell_entrypoint(
    entrypoint: &Entrypoint,
    allow_shell: bool,
) -> std::result::Result<(), String> {
    if allow_shell {
        return Ok(());
    }

    let cmd = &entrypoint.command;
    if cmd.contains(' ')
        || cmd.contains('|')
        || cmd.contains('&')
        || cmd.contains(';')
        || cmd.contains('>')
        || cmd.contains('<')
    {
        return Err(
            "Entrypoint command requires shell interpretation but allow_shell permission is false"
                .to_string(),
        );
    }

    let Some(resolved_command) = resolve_invoked_command(entrypoint) else {
        return Ok(());
    };

    if is_shell_name(resolved_command) {
        return Err(
            "Entrypoint command is a shell executable but allow_shell permission is false"
                .to_string(),
        );
    }

    Ok(())
}

fn resolve_invoked_command(entrypoint: &Entrypoint) -> Option<&str> {
    let mut command = entrypoint.command.as_str();
    let mut args = entrypoint.args.as_slice();

    loop {
        if !is_wrapper_command(command) {
            return Some(command);
        }

        let (next_command, remaining_args) = unwrap_wrapper_command(command, args)?;

        command = next_command;
        args = remaining_args;
    }
}

fn unwrap_wrapper_command<'a>(
    command: &str,
    args: &'a [String],
) -> Option<(&'a str, &'a [String])> {
    if wrapper_name(command) == Some("env") {
        let mut index = 0;
        while index < args.len() {
            let arg = args[index].as_str();
            if arg == "-" || (arg.starts_with('-') && !arg.contains('=')) {
                index += 1;
                continue;
            }
            if arg.contains('=') {
                index += 1;
                continue;
            }
            return Some((arg, &args[index + 1..]));
        }
        return None;
    }

    let mut index = 0;
    while index < args.len() && args[index].starts_with('-') {
        index += 1;
    }

    args.get(index)
        .map(String::as_str)
        .map(|next| (next, &args[index + 1..]))
}

fn is_wrapper_command(command: &str) -> bool {
    matches!(wrapper_name(command), Some("env" | "command"))
}

fn wrapper_name(command: &str) -> Option<&str> {
    std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
}

fn is_shell_name(command: &str) -> bool {
    wrapper_name(command).is_some_and(|name| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "sh" | "bash"
                | "zsh"
                | "ksh"
                | "csh"
                | "tcsh"
                | "cmd"
                | "cmd.exe"
                | "powershell"
                | "powershell.exe"
                | "pwsh"
                | "pwsh.exe"
                | "fish"
        )
    })
}
