//! Parse SSH session commands and construct literal OpenSSH arguments.
//!
//! Authentication and host verification stay with the system SSH client. The
//! remote process speaks ztty's mux protocol and owns the persistent session.

use alloc::ffi::CString;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::net::Ipv6Addr;

const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshAction {
    Attach,
    List,
    Close,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshOptions {
    /// OpenSSH destination, with URI brackets and port removed.
    pub destination: String,
    pub port: Option<u16>,
    pub identity: Option<String>,
    pub ssh_config: Option<String>,
    /// Executable name or literal path on the remote machine.
    pub remote_bin: String,
    pub session: String,
    pub action: SshAction,
    pub observe: bool,
    pub takeover: bool,
    pub terminal: bool,
    /// Local target session, then layout mode: `tab`, `right`, or `down`.
    pub target: Option<(String, String)>,
    /// Command used only when `mux attach` creates a missing session.
    pub command: Vec<String>,
}

impl SshOptions {
    /// Parse `DEST [SESSION] [-- COMMAND...]` and its SSH/view options.
    pub fn parse(args: &[&str]) -> Result<Self, String> {
        validate_arguments(args.iter().copied())?;
        let mut result = Self {
            destination: String::new(),
            port: None,
            identity: None,
            ssh_config: None,
            remote_bin: "ztty".into(),
            session: "main".into(),
            action: SshAction::Attach,
            observe: false,
            takeover: false,
            terminal: false,
            target: None,
            command: Vec::new(),
        };
        let mut endpoint = None;
        let mut explicit_session = false;
        let mut explicit_action = false;
        let mut explicit_remote_bin = false;
        let mut index = 0;
        while index < args.len() {
            let arg = args[index];
            match arg {
                "--" => {
                    if endpoint.is_none() {
                        return Err("SSH destination is required before -- COMMAND".into());
                    }
                    result.command = args[index + 1..].iter().map(|arg| (*arg).into()).collect();
                    if result.command.is_empty() || result.command[0].is_empty() {
                        return Err("-- needs a remote command".into());
                    }
                    break;
                }
                "-p" | "--port" => {
                    if result.port.is_some() {
                        return Err("SSH port was specified more than once".into());
                    }
                    result.port = Some(parse_port(next_value(args, &mut index, arg)?)?);
                }
                "-i" | "--identity" => {
                    if result.identity.is_some() {
                        return Err("SSH identity was specified more than once".into());
                    }
                    result.identity = Some(next_value(args, &mut index, arg)?.into());
                }
                "-F" | "--ssh-config" => {
                    if result.ssh_config.is_some() {
                        return Err("SSH config was specified more than once".into());
                    }
                    result.ssh_config = Some(next_value(args, &mut index, arg)?.into());
                }
                "--remote-bin" => {
                    if explicit_remote_bin {
                        return Err("remote executable was specified more than once".into());
                    }
                    result.remote_bin = next_value(args, &mut index, arg)?.into();
                    explicit_remote_bin = true;
                }
                "--list" | "--close" => {
                    if explicit_action {
                        return Err("choose only one of --list and --close".into());
                    }
                    result.action = if arg == "--list" {
                        SshAction::List
                    } else {
                        SshAction::Close
                    };
                    explicit_action = true;
                }
                "--observe" => result.observe = true,
                "--takeover" => result.takeover = true,
                "--terminal" => result.terminal = true,
                "--tab" | "--split-right" | "--split-down" => {
                    if result.target.is_some() {
                        return Err("choose only one SSH window layout target".into());
                    }
                    let target = next_value(args, &mut index, arg)?;
                    let mode = match arg {
                        "--tab" => "tab",
                        "--split-right" => "right",
                        _ => "down",
                    };
                    result.target = Some((target.into(), mode.into()));
                }
                value if value.starts_with('-') => {
                    return Err(format!("unknown SSH option: {value}"));
                }
                value if endpoint.is_none() => endpoint = Some(value),
                value if !explicit_session => {
                    result.session = value.into();
                    explicit_session = true;
                }
                _ => return Err("put the remote command after --".into()),
            }
            index += 1;
        }

        let (destination, uri_port) =
            parse_destination(endpoint.ok_or("SSH destination is required")?)?;
        result.destination = destination;
        if uri_port.is_some() && result.port.is_some() {
            return Err("choose the SSH port in the URI or with --port, not both".into());
        }
        result.port = result.port.or(uri_port);
        if result.action == SshAction::List && explicit_session {
            return Err("--list does not take a session name".into());
        }
        result.validate()?;
        Ok(result)
    }

    /// Return argv including `ssh` itself. The caller owns the child lifetime.
    ///
    /// `control_path` must point inside a private per-user directory. Supplying
    /// it enables reuse of the SSH connection without changing SSH config.
    pub fn ssh_argv(&self, control_path: Option<&str>) -> Result<Vec<CString>, String> {
        self.validate()?;
        let mut argv = Vec::with_capacity(20);
        push(&mut argv, "ssh")?;
        push(
            &mut argv,
            if self.action == SshAction::Attach {
                "-tt"
            } else {
                "-T"
            },
        )?;
        for option in ["ServerAliveInterval=15", "ServerAliveCountMax=3"] {
            push(&mut argv, "-o")?;
            push(&mut argv, option)?;
        }
        if let Some(path) = control_path {
            validate_value(path, "SSH control path")?;
            for option in ["ControlMaster=auto", "ControlPersist=60"] {
                push(&mut argv, "-o")?;
                push(&mut argv, option)?;
            }
            // -S is OpenSSH's literal-argv spelling of ControlPath; unlike an
            // -o configuration line it also preserves spaces and quotes.
            push(&mut argv, "-S")?;
            push(&mut argv, path)?;
        }
        if let Some(port) = self.port {
            push(&mut argv, "-p")?;
            push(&mut argv, &port.to_string())?;
        }
        if let Some(identity) = &self.identity {
            push(&mut argv, "-i")?;
            push(&mut argv, identity)?;
        }
        if let Some(config) = &self.ssh_config {
            push(&mut argv, "-F")?;
            push(&mut argv, config)?;
        }
        push(&mut argv, "--")?;
        push(&mut argv, &self.destination)?;
        push(&mut argv, &self.remote_command()?)?;
        Ok(argv)
    }

    fn validate(&self) -> Result<(), String> {
        validate_destination(&self.destination)?;
        if self.port == Some(0) {
            return Err("SSH port must be between 1 and 65535".into());
        }
        validate_value(&self.remote_bin, "remote executable")?;
        if self.remote_bin.starts_with('-') {
            return Err("remote executable must not start with '-'".into());
        }
        for (value, name) in [
            (self.identity.as_deref(), "SSH identity"),
            (self.ssh_config.as_deref(), "SSH config"),
        ] {
            if let Some(value) = value {
                validate_value(value, name)?;
            }
        }
        if self.action != SshAction::List && !valid_session(&self.session) {
            return Err("session names use 1..64 ASCII letters, digits, dot, underscore or hyphen, and cannot start with '-' or be '.' or '..'".into());
        }
        if self.observe && self.takeover {
            return Err("--observe and --takeover cannot be combined".into());
        }
        if self.terminal && self.target.is_some() {
            return Err("--terminal cannot be combined with a window layout target".into());
        }
        if self.action != SshAction::Attach
            && (self.observe
                || self.takeover
                || self.terminal
                || self.target.is_some()
                || !self.command.is_empty())
        {
            return Err("view options and remote commands require an SSH attach".into());
        }
        if self.observe && !self.command.is_empty() {
            return Err("--observe cannot create a session with a command".into());
        }
        if let Some((target, mode)) = &self.target {
            validate_value(target, "window layout target")?;
            if !matches!(mode.as_str(), "tab" | "right" | "down") {
                return Err("SSH layout mode must be tab, right or down".into());
            }
        }
        validate_arguments(self.command.iter().map(String::as_str))?;
        if self.command.first().is_some_and(String::is_empty) {
            return Err("remote command executable is empty".into());
        }
        Ok(())
    }

    fn remote_command(&self) -> Result<String, String> {
        let mut command = if self.remote_bin == "ztty" {
            // Use a fixed POSIX bootstrap even when the login shell is fish.
            // User-controlled arguments are appended as literal shell words.
            let bootstrap = "if command -v ztty >/dev/null 2>&1; then exec ztty \"$@\"; else exec \"$HOME/.local/bin/ztty\" \"$@\"; fi";
            let mut text = String::from("sh -c ");
            shell_word(&mut text, bootstrap);
            text.push_str(" 'ztty-ssh'");
            text
        } else {
            let mut text = String::from("exec ");
            shell_word(&mut text, &self.remote_bin);
            text
        };
        for token in [
            "mux",
            match self.action {
                SshAction::Attach => "attach",
                SshAction::List => "list",
                SshAction::Close => "close",
            },
        ] {
            command.push(' ');
            shell_word(&mut command, token);
        }
        if self.observe {
            command.push_str(" '--observe'");
        }
        if self.takeover {
            command.push_str(" '--takeover'");
        }
        if self.action != SshAction::List {
            command.push(' ');
            shell_word(&mut command, &self.session);
        }
        if !self.command.is_empty() {
            command.push_str(" '--'");
            for token in &self.command {
                command.push(' ');
                shell_word(&mut command, token);
            }
        }
        if command.len() > MAX_TOTAL_BYTES {
            return Err("quoted SSH remote command exceeds 64 KiB".into());
        }
        Ok(command)
    }
}

fn push(argv: &mut Vec<CString>, value: &str) -> Result<(), String> {
    argv.push(CString::new(value).map_err(|_| "SSH argument contains NUL")?);
    Ok(())
}

fn next_value<'a>(args: &'a [&str], index: &mut usize, name: &str) -> Result<&'a str, String> {
    *index += 1;
    let value = args
        .get(*index)
        .copied()
        .ok_or_else(|| format!("{name} needs a value"))?;
    if value.is_empty() {
        return Err(format!("{name} needs a non-empty value"));
    }
    Ok(value)
}

fn validate_arguments<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let mut total = 0;
    for (index, arg) in args.into_iter().enumerate() {
        if index >= MAX_ARGUMENTS {
            return Err("SSH command exceeds 256 arguments".into());
        }
        if arg.len() > MAX_ARGUMENT_BYTES || arg.chars().any(char::is_control) {
            return Err(
                "SSH arguments must not contain control characters or exceed 16 KiB".into(),
            );
        }
        total += arg.len();
        if total > MAX_TOTAL_BYTES {
            return Err("SSH arguments exceed 64 KiB".into());
        }
    }
    Ok(())
}

fn validate_value(value: &str, name: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_ARGUMENT_BYTES || value.chars().any(char::is_control) {
        return Err(format!(
            "{name} must be non-empty, contain no control characters and fit within 16 KiB"
        ));
    }
    Ok(())
}

fn parse_port(port: &str) -> Result<u16, String> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("SSH port must be between 1 and 65535".into());
    }
    port.parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "SSH port must be between 1 and 65535".into())
}

fn parse_destination(value: &str) -> Result<(String, Option<u16>), String> {
    let (authority, uri) = match value.strip_prefix("ssh://") {
        Some(authority) => (authority, true),
        None => (value, false),
    };
    if authority.contains('/') {
        return Err("SSH socket paths are not supported; use ssh://[user@]host[:port]".into());
    }
    let (user, host) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    let (host, port) = if let Some(bracketed) = host.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or("IPv6 SSH addresses need a closing ']'")?;
        if host.parse::<Ipv6Addr>().is_err() {
            return Err("invalid IPv6 SSH address".into());
        }
        let port = if suffix.is_empty() {
            None
        } else if uri {
            Some(parse_port(
                suffix
                    .strip_prefix(':')
                    .ok_or("unexpected text after IPv6 SSH address")?,
            )?)
        } else {
            return Err("use --port or an ssh:// URI to set the SSH port".into());
        };
        (host, port)
    } else if uri {
        match host.split_once(':') {
            Some((host, port)) => {
                if port.contains(':') {
                    return Err("IPv6 SSH URIs need brackets: ssh://[::1]:22".into());
                }
                (host, Some(parse_port(port)?))
            }
            None => (host, None),
        }
    } else {
        (host, None)
    };
    let destination = match user {
        Some(user) => format!("{user}@{host}"),
        None => host.into(),
    };
    validate_destination(&destination)?;
    Ok((destination, port))
}

fn validate_destination(destination: &str) -> Result<(), String> {
    if destination.is_empty() || destination.len() > 1024 || destination.starts_with('-') {
        return Err("invalid SSH destination".into());
    }
    let host = match destination.split_once('@') {
        Some((user, host)) => {
            if !valid_host_word(user) {
                return Err("invalid SSH user name".into());
            }
            host
        }
        None => destination,
    };
    if !valid_host_word(host) && host.parse::<Ipv6Addr>().is_err() {
        return Err("invalid SSH host; use a host name, SSH alias or IPv6 address".into());
    }
    Ok(())
}

fn valid_host_word(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn valid_session(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn shell_word(output: &mut String, value: &str) {
    output.push('\'');
    for character in value.chars() {
        if character == '\'' {
            output.push_str("'\"'\"'");
        } else {
            output.push(character);
        }
    }
    output.push('\'');
}
