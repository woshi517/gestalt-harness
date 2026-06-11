use serde::{Deserialize, Serialize};

use gestalt_core::ContextStability;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: String, // e.g., "stdio"
    pub entrypoint: Entrypoint,
    pub capabilities: Capabilities,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub hooks: bool,
    #[serde(default)]
    pub context: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
        if self.id.trim().is_empty() {
            return Err("Extension ID cannot be empty".to_string());
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
