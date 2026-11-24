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
use std::sync::atomic::{AtomicBool, Ordering};

/// The login shell's environment, once it answered.
static ENV: OnceLock<HashMap<String, String>> = OnceLock::new();
/// Set when the first capture has ended, answered or not.
static ASKED: OnceLock<()> = OnceLock::new();
/// The app's own environment, while the login shell has not answered.
static FALLBACK: OnceLock<HashMap<String, String>> = OnceLock::new();
/// A capture asked again (see [`forget_lookups`]) is running.
static RETRYING: AtomicBool = AtomicBool::new(false);

/// What Claude Code sets for the programs it starts, to say "you run inside
/// my session": seen when the app itself is started from a terminal in a
/// Claude Code session, or by an agent. Passed on, they would tell every
/// agent the app starts that it is that session's child: a `claude` in a
/// terminal then says "Transcript saving is off — inherited
/// CLAUDE_CODE_CHILD_SESSION marker" and keeps no record of what is said.
const PARENT_SESSION: [&str; 11] = [
    "AI_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
];

/// Whether variable `name` says which agent session a program runs inside
/// (see [`forget_parent_session`]). Not what a user sets for an agent
/// (`CLAUDE_CONFIG_DIR`, `ANTHROPIC_API_KEY`, `CLAUDE_CODE_USE_BEDROCK`…).
pub fn is_parent_session_variable(name: &str) -> bool {
    PARENT_SESSION.contains(&name)
}

/// Forgets that the app was started from inside an agent's session: the
/// agents it starts are the user's own sessions, nobody's children. What
/// the user's shell profile sets is read after this, and stays.
///
/// Call first thing in the app, before anything reads the environment on
/// another thread.
pub fn forget_parent_session() {
    for name in PARENT_SESSION {
        if std::env::var_os(name).is_some() {
            // SAFETY: called at the start of the app, before any thread
            // that reads the environment is started.
            unsafe { std::env::remove_var(name) };
        }
    }
}

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
    ASKED.get_or_init(|| match capture() {
        Some(env) => {
            let _ = ENV.set(env);
        }
        None => {
            tracing::warn!("could not read the login shell environment; using the app's own")
        }
    });
    ENV.get().unwrap_or_else(fallback)
}

/// The app's own environment, for when the login shell did not answer. Kept
/// apart from [`ENV`], so a later answer can still take its place.
fn fallback() -> &'static HashMap<String, String> {
    FALLBACK.get_or_init(|| std::env::vars().collect())
}

/// [`shell_env`] once it has been read, without waiting for it (the login
/// shell can take seconds to start).
pub fn shell_env_if_ready() -> Option<&'static HashMap<String, String>> {
    ENV.get().or_else(|| ASKED.get().map(|()| fallback()))
}

/// Asks the login shell again, in the background, when it did not answer
/// the first time (a slow first start after login, a profile fixed since):
/// without this the app's own short `PATH` stayed until the app was quit.
fn ask_again() {
    if ASKED.get().is_none() || ENV.get().is_some() || RETRYING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("shell-env".into())
        .spawn(|| {
            if let Some(env) = capture() {
                let _ = ENV.set(env);
                clear_lookups();
            }
            RETRYING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        RETRYING.store(false, Ordering::SeqCst);
    }
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

/// How long a search of `PATH` is remembered. Views ask "is this agent
/// installed?" while they draw, and one search looks at a file in every
/// `PATH` folder (times every `PATHEXT` ending on Windows, where each look
/// is slow): asked on every frame, that held the window up. A program
/// installed meanwhile is found once this has passed, or at once after
/// [`forget_lookups`].
const LOOKUP_KEPT: std::time::Duration = std::time::Duration::from_secs(5);

type Lookups = HashMap<String, (std::time::Instant, Option<PathBuf>)>;

static LOOKUPS: std::sync::Mutex<Option<Lookups>> = std::sync::Mutex::new(None);

/// Forgets what [`which`] remembers: the next search looks again. For when
/// the user says something was just installed (*Check again*). If the login
/// shell's environment could not be read, it is asked for again in the
/// background, and used once it comes.
pub fn forget_lookups() {
    clear_lookups();
    ask_again();
}

fn clear_lookups() {
    if let Ok(mut lookups) = LOOKUPS.lock() {
        *lookups = None;
    }
}

/// Finds `program` on the environment's `PATH`. On Windows `PATHEXT`
/// extensions are tried too, so `which("npx")` finds `npx.cmd`. A search of
/// `PATH` is remembered for a few seconds (see [`LOOKUP_KEPT`]).
pub fn which(program: &str) -> Option<PathBuf> {
    // A path is looked at where it says: one or a few files, never kept.
    if program.contains('/') || program.contains(std::path::MAIN_SEPARATOR) {
        return candidates(PathBuf::from(program))
            .into_iter()
            .find(|path| is_executable(path));
    }
    let now = std::time::Instant::now();
    if let Ok(lookups) = LOOKUPS.lock()
        && let Some((at, found)) = lookups.as_ref().and_then(|known| known.get(program))
        && now.duration_since(*at) < LOOKUP_KEPT
    {
        return found.clone();
    }
    let path = var("PATH")?;
    let found = std::env::split_paths(path)
        .flat_map(|dir| candidates(dir.join(program)))
        .find(|candidate| is_executable(candidate));
    if let Ok(mut lookups) = LOOKUPS.lock() {
        lookups
            .get_or_insert_with(HashMap::new)
            .insert(program.to_string(), (now, found.clone()));
    }
    found
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
    let own = APP_ONLY.lock().unwrap_or_else(|err| err.into_inner());
    Some(
        std::env::vars()
            .filter(|(name, _)| !own.iter().any(|kept| kept.eq_ignore_ascii_case(name)))
            .collect(),
    )
}

/// Variables the app set for itself at its start (how its own windows are
/// drawn), by name: the terminals and agents it starts do not get them.
static APP_ONLY: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Says that the app set variable `name` for itself alone: what it starts
/// is given the environment without it. (Another app of the same toolkit
/// started from a terminal here would else be drawn the app's way.)
pub fn keep_to_app(name: &str) {
    APP_ONLY
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .push(name.to_string());
}

#[cfg(unix)]
mod unix {
    use std::collections::HashMap;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// Marker printed before `env -0`, so anything an rc file prints is skipped.
    pub(super) const MARKER: &str = "__PILOTHOUSE_ENV_BEGIN__";
    /// Marker printed after `env -0`: the answer is complete when it comes,
    /// even while something the profile started in the background still
    /// holds the pipe open.
    pub(super) const END: &str = "__PILOTHOUSE_ENV_END__";
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
                &format!("printf '{MARKER}'; /usr/bin/env -0; printf '{END}'"),
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
            let mut sender = Some(sender);
            let mut chunk = [0u8; 8192];
            loop {
                match std::io::Read::read(&mut stdout, &mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        // After the answer the pipe is still read to its end,
                        // so a job the profile left writing to it is not
                        // killed by a broken pipe.
                        if sender.is_none() {
                            continue;
                        }
                        output.extend_from_slice(&chunk[..read]);
                        if complete(&output)
                            && let Some(sender) = sender.take()
                        {
                            let _ = sender.send(std::mem::take(&mut output));
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            if let Some(sender) = sender {
                let _ = sender.send(output);
            }
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
        // The shell ends once it printed the answer; reaped aside, so a slow
        // logout script does not hold up the caller.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        parse(&String::from_utf8_lossy(&output))
    }

    /// Whether `output` holds the whole answer: `MARKER`, then `END`.
    pub(super) fn complete(output: &[u8]) -> bool {
        let find = |haystack: &[u8], needle: &str| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle.as_bytes())
        };
        find(output, MARKER)
            .is_some_and(|begin| find(&output[begin + MARKER.len()..], END).is_some())
    }

    /// Parses `MARKER` followed by NUL-separated `KEY=value` pairs, up to
    /// `END` when it is there (a logout script may print after it).
    pub(super) fn parse(output: &str) -> Option<HashMap<String, String>> {
        let (_, vars) = output.split_once(MARKER)?;
        let vars = vars.split_once(END).map_or(vars, |(vars, _)| vars);
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
        #[test]
        fn lines_go_on_past_text_that_is_not_utf8() {
            let input: &[u8] = b"one\r\ntw\xffo\nthree";
            let lines: Vec<String> = crate::lossy_lines(input).collect();
            assert_eq!(lines, ["one", "tw\u{fffd}o", "three"]);
        }

        use super::*;

        #[test]
        fn skips_rc_noise_before_the_marker() {
            let output = format!("welcome!\n{MARKER}PATH=/usr/bin:/bin\0HOME=/Users/me\0");
            let env = parse(&output).unwrap();
            assert_eq!(env["PATH"], "/usr/bin:/bin");
            assert_eq!(env["HOME"], "/Users/me");
        }

        #[test]
        fn the_answer_ends_at_the_end_marker() {
            let output = format!("hi\n{MARKER}PATH=/bin\0HOME=/h\0{END}bye=now\n");
            assert!(complete(output.as_bytes()));
            let env = parse(&output).unwrap();
            assert_eq!(env["HOME"], "/h");
            assert!(!env.contains_key("bye"));
            // Not complete until the end marker came after the answer.
            assert!(!complete(format!("{MARKER}PATH=/bin\0").as_bytes()));
            assert!(!complete(format!("{END}{MARKER}PATH=/bin\0").as_bytes()));
        }

        #[test]
        fn rejects_output_without_path() {
            assert!(parse(&format!("{MARKER}HOME=/x\0")).is_none());
            assert!(parse("no marker").is_none());
        }
    }
}

/// The lines a program writes to `pipe`, as text even where they are not
/// UTF-8 (a log cut mid-character, output in the system code page):
/// `BufRead::lines` stops at the first such line, and then the program
/// writes into a closed pipe. Ends when the pipe does.
pub fn lossy_lines(pipe: impl std::io::Read) -> impl Iterator<Item = String> {
    use std::io::BufRead as _;
    let mut reader = std::io::BufReader::new(pipe);
    let mut bytes = Vec::new();
    std::iter::from_fn(move || {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                    if bytes.last() == Some(&b'\r') {
                        bytes.pop();
                    }
                }
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
        }
    })
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

/// The shell the user chose for new terminals (Settings → Terminal), by
/// the name [`shells`] lists it under; `None`: the system's.
static CHOSEN_SHELL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Chooses the shell new terminals start, by the name [`shells`] lists it
/// under (`None`, or a shell that is not installed: the system's). On a
/// computer where the system's default may not be run (PowerShell held back
/// by a policy), this is how another one is used.
pub fn choose_shell(name: Option<String>) {
    if let Ok(mut chosen) = CHOSEN_SHELL.lock() {
        *chosen = name.filter(|name| !name.trim().is_empty());
    }
}

/// The name given to [`choose_shell`] (`None`: the system's shell).
pub fn chosen_shell_name() -> Option<String> {
    CHOSEN_SHELL.lock().ok()?.clone()
}

/// The shell chosen with [`choose_shell`], when it is installed.
fn chosen_shell() -> Option<Shell> {
    let name = chosen_shell_name()?;
    installed_shells(&system_shell())
        .into_iter()
        .find(|shell| shell.name == name)
}

/// The shell new terminals start: the one the user chose, else the
/// system's ([`system_shell`]).
pub fn default_shell() -> (PathBuf, Vec<String>) {
    match chosen_shell() {
        Some(shell) => (shell.program, shell.args),
        None => system_shell(),
    }
}

/// The system's default shell: `$SHELL` on Unix; on Windows PowerShell 7 if
/// installed, else Windows PowerShell, else `cmd`.
pub fn system_shell() -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        for shell in ["pwsh", "powershell"] {
            if let Some(path) = which(shell) {
                return (path, vec!["-NoLogo".into()]);
            }
        }
        return command_prompt();
    }
    // Never waits for the login shell: terminals start on the UI thread, and
    // the shell asked for the environment is this same `$SHELL` anyway.
    let shell = shell_env_if_ready()
        .and_then(|env| env.get("SHELL").cloned())
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/zsh".into());
    // A login shell, so profile files run as in the user's own terminal.
    (PathBuf::from(shell), vec!["-l".into()])
}

/// Windows' Command Prompt.
pub fn command_prompt() -> (PathBuf, Vec<String>) {
    (
        PathBuf::from(var("COMSPEC").unwrap_or("cmd.exe")),
        Vec::new(),
    )
}

/// Whether `program` is Command Prompt, whichever way its path is written.
pub fn is_command_prompt(program: &Path) -> bool {
    program
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd"))
}

/// The shell that runs a program as a terminal's whole point (an agent's
/// CLI, in `cwd`), told its command line.
///
/// On Unix the default shell. On Windows Command Prompt, or Git Bash when
/// that is the shell the user chose; never PowerShell, chosen or not: the
/// command-line tools npm installs are `.cmd` files with a PowerShell
/// script beside them, which PowerShell prefers and then refuses to run
/// where scripts are not allowed (Windows' own default, and many
/// companies' policy), and PowerShell passes on an argument with quotes in
/// it differently from version to version. Nobody types in this shell: it
/// only starts the program. Command Prompt cannot start in a folder named
/// by a network path (`\\server\share`): there the default shell runs it.
pub fn program_shell(cwd: &Path) -> (PathBuf, Vec<String>) {
    if !cfg!(windows) {
        return default_shell();
    }
    if let Some(shell) = chosen_shell()
        && starts_programs(&shell.program)
    {
        return (shell.program, shell.args);
    }
    if cwd.to_string_lossy().starts_with(r"\\") {
        return system_shell();
    }
    command_prompt()
}

/// Whether the shell `program`, chosen for terminals, is also the one to
/// start an agent's CLI with on Windows: Command Prompt and Git Bash find
/// a tool's `.cmd` file; PowerShell would take the script beside it (see
/// [`program_shell`]), and WSL's launcher is no shell of this computer.
fn starts_programs(program: &Path) -> bool {
    program.file_stem().is_some_and(|name| {
        ["cmd", "bash"]
            .iter()
            .any(|known| name.eq_ignore_ascii_case(known))
    })
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
    let mut list = installed_shells(&system_shell());
    let (default_program, _) = default_shell();
    for shell in &mut list {
        shell.default = shell.program == default_program;
    }
    // The default leads.
    list.sort_by_key(|shell| !shell.default);
    list
}

/// The shells installed here, the system's own (`system`) first on Unix;
/// none marked as the default.
fn installed_shells((system_program, system_args): &(PathBuf, Vec<String>)) -> Vec<Shell> {
    let mut list: Vec<Shell> = Vec::new();
    let mut add = |name: String, program: PathBuf, args: Vec<String>| {
        if list
            .iter()
            .any(|known| known.name == name || known.program == program)
        {
            return;
        }
        list.push(Shell {
            name,
            program,
            args,
            default: false,
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
        let (cmd, _) = command_prompt();
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
        // The system's first, then the rest as /etc/shells lists them.
        let name_of = |path: &std::path::Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        add(
            name_of(system_program),
            system_program.clone(),
            system_args.clone(),
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
        // The terminal's menus list these as they open: until the login
        // shell has answered, the app's own `PATH` is searched instead of
        // waiting for it.
        let found = |name: &str| match shell_env_if_ready() {
            Some(_) => which(name),
            None => std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(name))
                    .find(|candidate| is_executable(candidate))
            }),
        };
        for name in ["fish", "nu", "pwsh"] {
            if let Some(path) = found(name) {
                let args = if name == "fish" {
                    vec!["-l".into()]
                } else {
                    Vec::new()
                };
                add(name.to_string(), path, args);
            }
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An agent's CLI is never started through PowerShell, whichever shell
    /// terminals start: it would run the script npm puts beside the
    /// `.cmd`, which a computer that allows no scripts refuses.
    #[test]
    fn an_agents_cli_is_not_started_by_powershell() {
        // (Named without their folders: a path of Windows is read as one
        // only there.)
        for shell in ["cmd.exe", "CMD", "bash.exe", "Git/bin/bash.exe"] {
            assert!(starts_programs(Path::new(shell)), "{shell}");
        }
        for shell in ["pwsh.exe", "powershell.exe", "PowerShell", "wsl.exe"] {
            assert!(!starts_programs(Path::new(shell)), "{shell}");
        }
    }

    #[test]
    fn only_a_sessions_own_marks_are_forgotten() {
        for mark in [
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "AI_AGENT",
        ] {
            assert!(is_parent_session_variable(mark), "{mark}");
        }
        // What a user sets for an agent is theirs to pass on.
        for theirs in [
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "CODEX_HOME",
            "PATH",
        ] {
            assert!(!is_parent_session_variable(theirs), "{theirs}");
        }
    }

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
    fn a_chosen_shell_is_the_default_while_it_is_installed() {
        let system = system_shell();
        let other = shells().into_iter().find(|shell| !shell.default);
        if let Some(other) = other {
            choose_shell(Some(other.name.clone()));
            assert_eq!(default_shell().0, other.program);
            let listed = shells();
            assert!(listed[0].default && listed[0].name == other.name);
            assert_eq!(listed.iter().filter(|shell| shell.default).count(), 1);
        }
        // One that is not installed (taken off since): the system's again.
        choose_shell(Some("no such shell".into()));
        assert_eq!(default_shell(), system);
        choose_shell(None);
        assert_eq!(default_shell(), system);
        assert!(is_command_prompt(Path::new(r"C:\Windows\System32\cmd.exe")) || !cfg!(windows));
        assert!(is_command_prompt(Path::new("cmd.exe")));
        assert!(!is_command_prompt(Path::new("/bin/zsh")));
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
