use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entrypoint {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
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
