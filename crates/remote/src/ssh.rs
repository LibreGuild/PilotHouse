//! `ssh` command lines.
//!
//! Every connection uses the system's `ssh`, so the user's keys, agent,
//! `~/.ssh/config` (aliases, jump hosts) and `known_hosts` all apply. To them
//! PilotHouse adds a `known_hosts` file of its own, for servers trusted in
//! Settings, and on macOS and Linux shares one connection per server between
//! commands (`ControlMaster`), so a Git refresh does not sign in each time.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::mapping::{Mapping, mappings, remote_of};

static STATE: OnceLock<PathBuf> = OnceLock::new();
static SOCKETS: OnceLock<PathBuf> = OnceLock::new();

/// Where PilotHouse keeps its SSH state (`state`: `<data dir>/ssh`) and the
/// shared connections' sockets (`sockets`: a private folder with a short
/// path). Call once at startup; without it, only the user's own
/// `known_hosts` is used and connections are not shared.
pub fn configure(state: PathBuf, sockets: PathBuf) {
    let _ = STATE.set(state);
    let _ = SOCKETS.set(sockets);
}

/// PilotHouse's `known_hosts`: servers trusted in Settings.
pub fn known_hosts_file() -> Option<PathBuf> {
    STATE.get().map(|dir| dir.join("known_hosts"))
}

/// The longest socket path a shared connection may have: a socket path is
/// at most 103 bytes on macOS, and `ssh` first makes the socket at this path
/// plus a 17-byte temporary suffix.
const MAX_CONTROL_PATH: usize = 84;

/// The socket of `mapping`'s shared connection, in a private folder.
/// `None` on Windows (its OpenSSH cannot share connections) or when the
/// path would be too long for a socket; commands then connect each time.
fn control_path(mapping: &Mapping) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let dir = SOCKETS.get()?;
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&mapping.host, mapping.port).hash(&mut hasher);
    let path = dir.join(format!("{:08x}", hasher.finish() as u32));
    if path.as_os_str().len() > MAX_CONTROL_PATH {
        return None;
    }
    private_dir(dir).ok()?;
    config_path(&path)
}

/// Creates `dir` readable by this user only (a shared connection's socket
/// lets whoever can reach it run commands as the user on the server).
pub(crate) fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The options for `mapping`. `interactive`: a shell in a terminal, which
/// may ask for a password or to accept a new server; otherwise a command,
/// which never asks and fails instead.
pub fn options(mapping: &Mapping, interactive: bool) -> Vec<String> {
    let mut settings: Vec<String> = Vec::new();
    if !interactive {
        settings.push("BatchMode=yes".into());
        settings.push("StrictHostKeyChecking=yes".into());
    }
    settings.push("ConnectTimeout=10".into());
    settings.push("ServerAliveInterval=15".into());
    settings.push("ServerAliveCountMax=3".into());
    if let Some(file) = known_hosts_file().and_then(|file| config_path(&file)) {
        settings.push(format!(
            "UserKnownHostsFile=\"~/.ssh/known_hosts\" \"{file}\""
        ));
    }
    if let Some(path) = control_path(mapping) {
        settings.push("ControlMaster=auto".into());
        settings.push(format!("ControlPath=\"{path}\""));
        settings.push("ControlPersist=10m".into());
    }
    let mut out = vec![if interactive { "-t" } else { "-T" }.to_string()];
    for setting in settings {
        out.push("-o".into());
        out.push(setting);
    }
    if let Some(port) = mapping.port {
        out.push("-p".into());
        out.push(port.to_string());
    }
    out
}

/// A path as an `ssh -o` value takes it: forward slashes (Windows' OpenSSH
/// reads them), and none with a quote, which could not be written.
fn config_path(path: &Path) -> Option<String> {
    let text = path.to_string_lossy().replace('\\', "/");
    (!text.contains('"')).then_some(text)
}

/// `ssh <options> -- <host> <script>`, a command on `mapping`'s server.
pub fn command(mapping: &Mapping, script: &str) -> Command {
    let mut command = ph_env::command("ssh");
    command
        .args(options(mapping, false))
        .arg("--")
        .arg(&mapping.host)
        .arg(script);
    command
}

/// The program and arguments for a shell on the server in `folder`, for a
/// terminal.
pub fn shell(mapping: &Mapping, folder: &str) -> (String, Vec<String>) {
    let mut args = options(mapping, true);
    args.push("--".into());
    args.push(mapping.host.clone());
    args.push(format!(
        "cd {} 2>/dev/null || cd; exec \"${{SHELL:-/bin/sh}}\" -l",
        quote(folder)
    ));
    ("ssh".into(), args)
}

/// `command`, which would run in a folder under a mapping, made to run on
/// the server in the matching folder; `None` when it is not under one. Call
/// it once the program, arguments, environment and folder are set; stdin,
/// stdout and stderr are set on what it returns. Arguments and variables
/// that are paths under a mapping become the server's paths.
pub fn route(command: &Command) -> Option<Command> {
    let remote = remote_of(command.get_current_dir()?)?;
    let mappings = mappings();
    let program = Path::new(command.get_program())
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())?;
    let mut words = vec!["exec".to_string()];
    let variables: Vec<String> = command
        .get_envs()
        .filter_map(|(key, value)| {
            let value = value?;
            Some(format!(
                "{}={}",
                key.to_string_lossy(),
                translate(&mappings, value)
            ))
        })
        .collect();
    if !variables.is_empty() {
        words.push("env".into());
        words.extend(variables.iter().map(|word| quote(word)));
    }
    words.push(quote(&program));
    words.extend(
        command
            .get_args()
            .map(|arg| quote(&translate(&mappings, arg))),
    );
    let script = format!("cd {} && {}", quote(&remote.path), words.join(" "));
    tracing::debug!(host = %remote.mapping.host, "on the server: {script}");
    Some(self::command(&remote.mapping, &script))
}

/// `value`, or its server path when it is a path under a mapping.
fn translate(mappings: &[Mapping], value: &OsStr) -> String {
    let text = value.to_string_lossy();
    crate::mapping::remote_in(mappings, &text, cfg!(windows))
        .map(|remote| remote.path)
        .unwrap_or_else(|| text.into_owned())
}

/// `word` for a POSIX shell: as is when it is plainly safe, else in single
/// quotes.
pub fn quote(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if safe {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// What a failed command's `stderr` says about SSH, in words for the user;
/// `None` when the command itself failed (ssh exits 255 for its own errors).
pub fn explain_failure(code: Option<i32>, stderr: &str, host: &str) -> Option<String> {
    if code != Some(255) {
        return None;
    }
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("@@@"))
        .unwrap_or("no details");
    Some(
        if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
            format!(
                "{host}'s key is not the one trusted before. This can mean someone is intercepting the connection: check with whoever runs the server before trusting it again."
            )
        } else if stderr.contains("Host key verification failed")
            || stderr.contains("No ED25519 host key is known")
            || stderr.contains("host key is known")
        {
            format!("{host} is not trusted yet. Check it in Settings → Network drives.")
        } else if stderr.contains("Permission denied") {
            format!("{host} did not accept your SSH key. Set one up in Settings → Network drives.")
        } else {
            format!("Cannot reach {host} over SSH: {first}")
        },
    )
}

/// Closes the connections shared between commands (at quit). They would
/// otherwise stay open, unused, for up to ten minutes.
pub fn close_shared() {
    for mapping in mappings() {
        let Some(path) = control_path(&mapping) else {
            continue;
        };
        let mut command = ph_env::command("ssh");
        command
            .args(["-O", "exit", "-o"])
            .arg(format!("ControlPath=\"{path}\""));
        if let Some(port) = mapping.port {
            command.args(["-p", &port.to_string()]);
        }
        let _ = command
            .arg("--")
            .arg(&mapping.host)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_quoted_for_the_shell() {
        assert_eq!(quote("status"), "status");
        assert_eq!(quote("--porcelain=v2"), "--porcelain=v2");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$HOME"), "'$HOME'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("x;rm -rf /"), "'x;rm -rf /'");
    }

    #[test]
    fn commands_never_prompt_and_shells_may() {
        let mapping = Mapping {
            host: "dev@nas".into(),
            port: Some(2222),
            ..Default::default()
        };
        let batch = options(&mapping, false);
        assert_eq!(batch[0], "-T");
        assert!(batch.contains(&"BatchMode=yes".to_string()));
        assert!(batch.contains(&"StrictHostKeyChecking=yes".to_string()));
        assert!(batch.ends_with(&["-p".to_string(), "2222".to_string()]));
        let (program, args) = shell(&mapping, "/srv/code/my app");
        assert_eq!(program, "ssh");
        assert_eq!(args[0], "-t");
        assert!(!args.contains(&"BatchMode=yes".to_string()));
        let script = args.last().unwrap();
        assert!(
            script.starts_with("cd '/srv/code/my app' 2>/dev/null || cd;"),
            "{script}"
        );
        assert_eq!(&args[args.len() - 3..args.len() - 1], ["--", "dev@nas"]);
    }

    #[test]
    fn ssh_failures_read_as_what_to_do() {
        let changed = "@@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n";
        assert!(
            explain_failure(Some(255), changed, "nas")
                .unwrap()
                .contains("intercepting")
        );
        let unknown = "No ED25519 host key is known for [nas]:22 and you have requested strict checking.\nHost key verification failed.\n";
        assert!(
            explain_failure(Some(255), unknown, "nas")
                .unwrap()
                .contains("not trusted")
        );
        let denied = "dev@nas: Permission denied (publickey,password).\n";
        assert!(
            explain_failure(Some(255), denied, "nas")
                .unwrap()
                .contains("SSH key")
        );
        let down = "ssh: connect to host nas port 22: Connection refused\n";
        assert_eq!(
            explain_failure(Some(255), down, "nas").unwrap(),
            "Cannot reach nas over SSH: ssh: connect to host nas port 22: Connection refused"
        );
        assert!(explain_failure(Some(1), "fatal: not a git repository", "nas").is_none());
    }
}
