//! The environment agents and terminals run in.
//!
//! On macOS and Linux, an app started from the Dock or a launcher inherits a
//! minimal environment, so tools installed through Homebrew, npm or rustup
//! are not on its `PATH`. Coding agents and terminals need the environment
//! the user's own terminal has, so PilotHouse asks the login shell for it
//! once, in the background, at startup. If the shell cannot be queried (it
//! hangs, prints garbage or is missing) the app's own environment is used.
//!
//! On Windows, apps inherit the user's full environment from the session, so
//! it is used as is. Variable names there are case-insensitive (`Path` and
//! `PATH` are the same), and programs are found with `PATHEXT` (`npx.cmd`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ENV: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Starts capturing the environment on a background thread. Optional: the
/// first caller of [`shell_env`] would otherwise do it synchronously.
pub fn warm_up() {
    std::thread::Builder::new()
        .name("shell-env".into())
        .spawn(|| {
            shell_env();
        })
        .ok();
}

/// The environment for child processes (cached after the first call).
pub fn shell_env() -> &'static HashMap<String, String> {
    ENV.get_or_init(|| {
        let captured = capture();
        if captured.is_none() {
            tracing::warn!("could not read the login shell environment; using the app's own");
        }
        captured.unwrap_or_else(|| std::env::vars().collect())
    })
}

/// One variable of [`shell_env`]; the name is case-insensitive on Windows.
pub fn var(name: &str) -> Option<&'static str> {
    let env = shell_env();
    if let Some(value) = env.get(name) {
        return Some(value);
    }
    if cfg!(windows) {
        return env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str());
    }
    None
}

/// Finds `program` on the environment's `PATH`. On Windows `PATHEXT`
/// extensions are tried too, so `which("npx")` finds `npx.cmd`.
pub fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') || program.contains(std::path::MAIN_SEPARATOR) {
        return candidates(PathBuf::from(program)).find(|path| is_executable(path));
    }
    let path = var("PATH")?;
    std::env::split_paths(path)
        .flat_map(|dir| candidates(dir.join(program)))
        .find(|candidate| is_executable(candidate))
}

/// `path` itself, then (on Windows, without an extension) `path.exe`, `.cmd`…
fn candidates(path: PathBuf) -> impl Iterator<Item = PathBuf> {
    let extensions: Vec<String> = if cfg!(windows) && path.extension().is_none() {
        var("PATHEXT")
            .unwrap_or(".COM;.EXE;.BAT;.CMD")
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| ext.to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };
    let with_extensions: Vec<PathBuf> = extensions
        .iter()
        .map(|ext| {
            let mut name = path.clone().into_os_string();
            name.push(ext);
            PathBuf::from(name)
        })
        .collect();
    std::iter::once(path).chain(with_extensions)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(unix)]
fn capture() -> Option<HashMap<String, String>> {
    unix::capture()
}

/// Windows sessions give apps the user's full environment.
#[cfg(not(unix))]
fn capture() -> Option<HashMap<String, String>> {
    Some(std::env::vars().collect())
}

#[cfg(unix)]
mod unix {
    use std::collections::HashMap;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// Marker printed before `env -0`, so anything an rc file prints is skipped.
    pub(super) const MARKER: &str = "__PILOTHOUSE_ENV_BEGIN__";
    /// A shell that takes longer than this to start is not waited for.
    const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

    pub(super) fn capture() -> Option<HashMap<String, String>> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        // `-i` so rc files that extend PATH (e.g. ~/.zshrc) are read too.
        let mut child = Command::new(&shell)
            .args([
                "-l",
                "-i",
                "-c",
                &format!("printf '{MARKER}'; /usr/bin/env -0"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut output);
            let _ = sender.send(output);
        });
        let output = match receiver.recv_timeout(SHELL_TIMEOUT) {
            Ok(output) => output,
            Err(_) => {
                // Reaped, or it stays a zombie. The reader thread ends when
                // whatever the shell started lets go of its output.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        };
        let _ = child.wait();
        parse(&String::from_utf8_lossy(&output))
    }

    /// Parses `MARKER` followed by NUL-separated `KEY=value` pairs.
    pub(super) fn parse(output: &str) -> Option<HashMap<String, String>> {
        let (_, vars) = output.split_once(MARKER)?;
        let env: HashMap<String, String> = vars
            .split('\0')
            .filter_map(|pair| pair.split_once('='))
            .filter(|(key, _)| !key.is_empty() && !key.contains('\n'))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        env.contains_key("PATH").then_some(env)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn skips_rc_noise_before_the_marker() {
            let output = format!("welcome!\n{MARKER}PATH=/usr/bin:/bin\0HOME=/Users/me\0");
            let env = parse(&output).unwrap();
            assert_eq!(env["PATH"], "/usr/bin:/bin");
            assert_eq!(env["HOME"], "/Users/me");
        }

        #[test]
        fn rejects_output_without_path() {
            assert!(parse(&format!("{MARKER}HOME=/x\0")).is_none());
            assert!(parse("no marker").is_none());
        }
    }
}

/// A command for a helper program (git, adb, an agent…). On Windows the
/// child gets no console window: a windowed app's console children would
/// otherwise flash a black window each time one starts.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The user's default shell for terminals: `$SHELL` on Unix; on Windows
/// PowerShell 7 if installed, else Windows PowerShell, else `cmd`.
pub fn default_shell() -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        for shell in ["pwsh", "powershell"] {
            if let Some(path) = which(shell) {
                return (path, vec!["-NoLogo".into()]);
            }
        }
        let cmd = var("COMSPEC").unwrap_or("cmd.exe");
        return (PathBuf::from(cmd), Vec::new());
    }
    let shell = var("SHELL")
        .map(str::to_string)
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/zsh".into());
    // A login shell, so profile files run as in the user's own terminal.
    (PathBuf::from(shell), vec!["-l".into()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_on_path() {
        let found = if cfg!(windows) {
            which("cmd")
        } else {
            which("sh")
        };
        assert!(found.is_some());
        assert!(var("PATH").is_some());
    }
}
