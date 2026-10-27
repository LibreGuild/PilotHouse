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
        return candidates(PathBuf::from(program))
            .into_iter()
            .find(|path| is_executable(path));
    }
    let path = var("PATH")?;
    std::env::split_paths(path)
        .flat_map(|dir| candidates(dir.join(program)))
        .find(|candidate| is_executable(candidate))
}

/// `path` itself or, on Windows unless it already ends in a runnable
/// extension, `path.exe`, `.cmd`… only: npm puts an extensionless shell
/// script (`npx`) next to `npx.cmd`, and Windows cannot start it ("%1 is
/// not a valid Win32 application"). A dot in a name (`python3.12`) is no
/// such extension.
fn candidates(path: PathBuf) -> Vec<PathBuf> {
    if !cfg!(windows) {
        return vec![path];
    }
    let pathext = var("PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD");
    let runnable = path.extension().is_some_and(|ext| {
        let ext = format!(".{}", ext.to_string_lossy());
        pathext
            .split(';')
            .any(|known| known.eq_ignore_ascii_case(&ext))
    });
    if runnable {
        return vec![path];
    }
    with_extensions(&path, pathext)
}

fn with_extensions(path: &Path, pathext: &str) -> Vec<PathBuf> {
    pathext
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(|ext| {
            let mut name = path.as_os_str().to_owned();
            name.push(ext.to_ascii_lowercase());
            PathBuf::from(name)
        })
        .collect()
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

/// A shell a terminal can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// What the user sees: `zsh`, `PowerShell`, `Git Bash`…
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The one [`default_shell`] starts.
    pub default: bool,
}

/// The shells installed here, the default first: on Unix those listed in
/// `/etc/shells` that exist (each once, by name); on Windows PowerShell 7,
/// Windows PowerShell, Command Prompt, Git Bash and WSL, as found.
pub fn shells() -> Vec<Shell> {
    let (default_program, default_args) = default_shell();
    let mut list: Vec<Shell> = Vec::new();
    let mut add = |name: String, program: PathBuf, args: Vec<String>| {
        if list
            .iter()
            .any(|known| known.name == name || known.program == program)
        {
            return;
        }
        let default = program == default_program;
        list.push(Shell {
            name,
            program,
            args,
            default,
        });
    };
    if cfg!(windows) {
        let found = |name: &str| which(name);
        if let Some(path) = found("pwsh") {
            add("PowerShell 7".into(), path, vec!["-NoLogo".into()]);
        }
        if let Some(path) = found("powershell") {
            add("Windows PowerShell".into(), path, vec!["-NoLogo".into()]);
        }
        let cmd = PathBuf::from(var("COMSPEC").unwrap_or("cmd.exe"));
        add("Command Prompt".into(), cmd, Vec::new());
        let git_bash = [
            var("ProgramFiles"),
            var("ProgramW6432"),
            var("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        .flat_map(|root| {
            [
                PathBuf::from(root).join("Git").join("bin").join("bash.exe"),
                PathBuf::from(root)
                    .join("Programs")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
            ]
        })
        .find(|path| path.is_file());
        if let Some(path) = git_bash {
            add("Git Bash".into(), path, vec!["--login".into(), "-i".into()]);
        }
        if let Some(path) = found("wsl") {
            add("WSL".into(), path, Vec::new());
        }
    } else {
        // The default first, then the rest as /etc/shells lists them.
        let name_of = |path: &std::path::Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        add(
            name_of(&default_program),
            default_program.clone(),
            default_args.clone(),
        );
        let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default();
        for line in listed.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let path = PathBuf::from(line);
            if path.is_file() {
                add(name_of(&path), path, vec!["-l".into()]);
            }
        }
        // Common shells a package manager installs without listing them.
        for name in ["fish", "nu", "pwsh"] {
            if let Some(path) = which(name) {
                let args = if name == "fish" {
                    vec!["-l".into()]
                } else {
                    Vec::new()
                };
                add(name.to_string(), path, args);
            }
        }
    }
    // The default leads.
    list.sort_by_key(|shell| !shell.default);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathext_names_replace_the_bare_one() {
        let names = with_extensions(Path::new("bin/npx"), ".EXE;.CMD;");
        assert_eq!(
            names,
            [PathBuf::from("bin/npx.exe"), PathBuf::from("bin/npx.cmd")]
        );
    }

    #[test]
    fn the_default_shell_is_listed_first() {
        let shells = shells();
        assert!(!shells.is_empty());
        assert!(shells[0].default);
        assert_eq!(shells.iter().filter(|shell| shell.default).count(), 1);
        let mut names: Vec<_> = shells.iter().map(|shell| &shell.name).collect();
        names.dedup();
        assert_eq!(names.len(), shells.len(), "each shell once");
    }

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
