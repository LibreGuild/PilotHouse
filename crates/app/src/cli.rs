//! Command-line subcommands that run without opening a window.

use std::path::{Path, PathBuf};

use ph_control::ControlClient;
use ph_store::paths;
use serde_json::Value;

const USAGE: &str = "\
usage:
  pilothouse                         start the app
  pilothouse <path>...               open folders as projects and files in the editor
                                     (in the running app, if there is one)
  pilothouse ctl <method> [json]     call the running app's control socket
  pilothouse mcp [--project PATH] [--agent NAME] [--workspace ID]
                                     run the MCP server agents use for board tools
  pilothouse agy-acp                 run Google Antigravity (agy) as an ACP agent
  pilothouse agent-hook --socket PATH --pane ID
                                     pass a terminal agent's hook event (stdin) to the app
";

/// What `main` does after reading the arguments.
pub enum Launch {
    /// A subcommand ran (or the paths went to the running app): exit.
    Exit(i32),
    /// Start the app, then open these paths.
    App(Vec<PathBuf>),
}

/// Runs a subcommand if `args` names one, or hands paths to the running
/// app; otherwise the app starts. Subcommands come first: an agent starts
/// `pilothouse mcp` in its project, which may well have a folder `mcp`.
pub fn run(args: &[String]) -> Launch {
    let args: Vec<String> = args.iter().map(|arg| unquoted(arg)).collect();
    let args = args.as_slice();
    match args.first().map(String::as_str) {
        Some("ctl") => Launch::Exit(ctl(&args[1..])),
        Some("mcp") => Launch::Exit(mcp(&args[1..])),
        Some("agy-acp") => Launch::Exit(agy_acp()),
        Some("agent-hook") => Launch::Exit(agent_hook(&args[1..])),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Launch::Exit(0)
        }
        Some("--version" | "-V") => {
            println!("pilothouse {}", env!("CARGO_PKG_VERSION"));
            Launch::Exit(0)
        }
        Some(first) if !first.starts_with('-') && Path::new(first).exists() => open(args),
        // macOS passes `-psn_…` when launched from Finder; ignore unknown flags.
        None => hand_over(Vec::new()),
        Some(flag) if flag.starts_with('-') => hand_over(Vec::new()),
        Some(other) => {
            eprintln!("unknown command `{other}`\n\n{USAGE}");
            Launch::Exit(2)
        }
    }
}

/// Explorer passes a drive root as `"C:\"`, whose backslash escapes the
/// closing quote, so the argument arrives as `C:"`.
fn unquoted(arg: &str) -> String {
    match arg.strip_suffix('"') {
        Some(root) if cfg!(windows) => format!("{root}\\"),
        _ => arg.to_string(),
    }
}

/// `pilothouse <path>...`: the running app opens them (see [`hand_over`]).
fn open(args: &[String]) -> Launch {
    let paths: Vec<PathBuf> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(|arg| paths::canonical(Path::new(arg)).unwrap_or_else(|_| PathBuf::from(arg)))
        .collect();
    hand_over(paths)
}

/// With PilotHouse already running (on this data folder), it opens `paths`
/// and comes to the front, and this launch exits: a second copy would share
/// the database and run every schedule twice. With none running, this
/// launch becomes the app.
fn hand_over(paths: Vec<PathBuf>) -> Launch {
    let Ok(mut client) = ControlClient::connect(&paths::control_socket_path()) else {
        return Launch::App(paths);
    };
    let listed: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    match client.call("app.open", serde_json::json!({ "paths": listed })) {
        Ok(_) => Launch::Exit(0),
        Err(err) => {
            eprintln!("PilotHouse is running but could not open that: {err:#}");
            Launch::Exit(1)
        }
    }
}

/// `pilothouse mcp`: serves MCP on stdio for a coding agent.
///
/// The project defaults to the current directory, which is where agents run.
fn mcp(args: &[String]) -> i32 {
    let mut project_path = std::env::current_dir().unwrap_or_default();
    let mut agent_name = "agent".to_string();
    let mut project_id = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--project", Some(path)) => project_path = path.into(),
            ("--agent", Some(name)) => agent_name = name.clone(),
            ("--workspace", Some(id)) if id.parse::<i64>().is_ok() => {
                project_id = id.parse().ok();
            }
            _ => {
                eprintln!("usage: pilothouse mcp [--project PATH] [--agent NAME] [--workspace ID]");
                return 2;
            }
        }
    }
    let options = ph_mcp::BridgeOptions {
        socket_path: paths::control_socket_path(),
        project_path,
        agent_name,
        project_id,
    };
    match ph_mcp::run_stdio(options) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("pilothouse mcp: {err:#}");
            1
        }
    }
}

/// `pilothouse agy-acp`: serves ACP on stdio by driving `agy`'s headless
/// mode. PilotHouse starts it for Antigravity chats.
fn agy_acp() -> i32 {
    match ph_agy::run_stdio() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("pilothouse agy-acp: {err:#}");
            1
        }
    }
}

/// A hook of an agent running in a PilotHouse terminal (Claude Code's
/// `--settings` hooks): the event JSON on stdin goes to the app as
/// `agent.event` for terminal pane `--pane`. It prints nothing and always
/// succeeds, so the agent is never held up or blocked by it.
fn agent_hook(args: &[String]) -> i32 {
    use std::io::Read as _;
    let value_of = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let (Some(socket), Some(pane)) = (value_of("--socket"), value_of("--pane")) else {
        return 0;
    };
    // All of it (a large tool event too): the agent must not hit a closed
    // pipe. An event that does not parse is not sent.
    let mut input = Vec::new();
    let _ = std::io::stdin().take(64 << 20).read_to_end(&mut input);
    let Ok(event) = serde_json::from_slice::<Value>(&input) else {
        return 0;
    };
    let params = serde_json::json!({ "pane": pane, "event": event });
    // Sent, not waited for: a busy app never holds the agent's tool up.
    let _ = ControlClient::connect(Path::new(&socket))
        .and_then(|mut client| client.send("agent.event", params));
    0
}

/// `pilothouse ctl <method> [json-params]`: prints the JSON result.
fn ctl(args: &[String]) -> i32 {
    let Some(method) = args.first() else {
        eprintln!("usage: pilothouse ctl <method> [json-params]");
        return 2;
    };
    let params = match args.get(1) {
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(params) => params,
            Err(err) => {
                eprintln!("params must be JSON: {err}");
                return 2;
            }
        },
        None => Value::Object(Default::default()),
    };
    let result = ControlClient::connect(&paths::control_socket_path())
        .and_then(|mut client| client.call(method, params));
    match result {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            );
            0
        }
        Err(err) => {
            eprintln!("{err:#}");
            1
        }
    }
}
