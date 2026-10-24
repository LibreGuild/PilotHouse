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
  pilothouse mcp [--project PATH] [--agent NAME]
                                     run the MCP server agents use for board tools
  pilothouse agy-acp                 run Google Antigravity (agy) as an ACP agent
";

/// What `main` does after reading the arguments.
pub enum Launch {
    /// A subcommand ran (or the paths went to the running app): exit.
    Exit(i32),
    /// Start the app, then open these paths.
    App(Vec<PathBuf>),
}

/// Runs a subcommand if `args` names one, or hands paths to the running
/// app; otherwise the app starts.
pub fn run(args: &[String]) -> Launch {
    let first = args.first().map(String::as_str);
    if first.is_some_and(|first| !first.starts_with('-') && Path::new(first).exists()) {
        return open(args);
    }
    match subcommand(args) {
        Some(code) => Launch::Exit(code),
        None => Launch::App(Vec::new()),
    }
}

/// `pilothouse <path>...`: the running app opens them, so a second window
/// (and a second copy of every agent) never starts; with none running, this
/// launch becomes the app.
fn open(args: &[String]) -> Launch {
    let paths: Vec<PathBuf> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(|arg| paths::canonical(Path::new(arg)).unwrap_or_else(|_| PathBuf::from(arg)))
        .collect();
    let forwarded = ControlClient::connect(&paths::control_socket_path()).and_then(|mut client| {
        client.call(
            "app.open",
            serde_json::json!({ "paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>() }),
        )
    });
    match forwarded {
        Ok(_) => Launch::Exit(0),
        Err(_) => Launch::App(paths),
    }
}

fn subcommand(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str) {
        None => None,
        Some("ctl") => Some(ctl(&args[1..])),
        Some("mcp") => Some(mcp(&args[1..])),
        Some("agy-acp") => Some(agy_acp()),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Some(0)
        }
        Some("--version" | "-V") => {
            println!("pilothouse {}", env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        // macOS passes `-psn_…` when launched from Finder; ignore unknown flags.
        Some(flag) if flag.starts_with('-') => None,
        Some(other) => {
            eprintln!("unknown command `{other}`\n\n{USAGE}");
            Some(2)
        }
    }
}

/// `pilothouse mcp`: serves MCP on stdio for a coding agent.
///
/// The project defaults to the current directory, which is where agents run.
fn mcp(args: &[String]) -> i32 {
    let mut project_path = std::env::current_dir().unwrap_or_default();
    let mut agent_name = "agent".to_string();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--project", Some(path)) => project_path = path.into(),
            ("--agent", Some(name)) => agent_name = name.clone(),
            _ => {
                eprintln!("usage: pilothouse mcp [--project PATH] [--agent NAME]");
                return 2;
            }
        }
    }
    let options = ph_mcp::BridgeOptions {
        socket_path: paths::control_socket_path(),
        project_path,
        agent_name,
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
