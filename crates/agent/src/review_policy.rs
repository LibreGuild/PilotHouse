//! What a review chat may do without asking: reading. A review runs in its
//! own worktree, often unattended, and must never change anything, so only
//! file reads, searches and a short list of read-only commands are allowed
//! automatically; everything else still asks the user.

use agent_client_protocol_schema::v1::ToolKind;
use serde_json::Value;

/// Commands a review may run, by program (and git subcommand).
const PROGRAMS: &[&str] = &[
    "git", "rg", "grep", "ls", "cat", "head", "tail", "wc", "find", "tree", "pwd",
];
const GIT: &[&str] = &[
    "diff",
    "log",
    "show",
    "blame",
    "status",
    "grep",
    "ls-files",
    "rev-parse",
    "merge-base",
    "cat-file",
    "shortlog",
];

/// Whether a tool call only reads.
pub fn allows(kind: Option<ToolKind>, raw_input: Option<&Value>, title: &str) -> bool {
    match kind {
        Some(ToolKind::Read | ToolKind::Search | ToolKind::Think) => true,
        Some(ToolKind::Execute) => {
            command(raw_input, title).is_some_and(|command| read_only_command(&command))
        }
        _ => false,
    }
}

/// The command line from the tool's input, or from a title like
/// ``Run command `git diff` ``.
fn command(raw_input: Option<&Value>, title: &str) -> Option<String> {
    if let Some(input) = raw_input {
        for key in ["command", "commandline", "cmd", "CommandLine"] {
            match input.get(key) {
                Some(Value::String(command)) => return Some(command.clone()),
                Some(Value::Array(parts)) => {
                    let parts: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
                    return Some(parts.join(" "));
                }
                _ => {}
            }
        }
    }
    let start = title.find('`')?;
    let end = title[start + 1..].find('`')?;
    Some(title[start + 1..start + 1 + end].to_string())
}

/// A single read-only command: no shell operators, no redirection, no
/// substitution, and only an allowed program.
pub fn read_only_command(command: &str) -> bool {
    if command.contains([';', '|', '&', '>', '<', '`', '\n', '$']) {
        return false;
    }
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    if !PROGRAMS.contains(&program) {
        return false;
    }
    let rest: Vec<&str> = words.collect();
    match program {
        "git" => {
            // Skip `-C dir` and `--no-pager`-style options before the subcommand.
            let mut ix = 0;
            while ix < rest.len() && rest[ix].starts_with('-') {
                ix += if rest[ix] == "-C" || rest[ix] == "-c" {
                    2
                } else {
                    1
                };
            }
            let ok = rest.get(ix).is_some_and(|sub| GIT.contains(sub));
            // `git diff --output=file` writes a file.
            ok && !rest
                .iter()
                .any(|arg| arg.starts_with("--output") || arg == &"-o")
        }
        "find" => !rest.iter().any(|arg| {
            matches!(
                *arg,
                "-exec" | "-execdir" | "-delete" | "-ok" | "-fprint" | "-fprintf" | "-fls"
            )
        }),
        _ => true,
    }
}

/// Whether a permission request is for one of PilotHouse's own app tools
/// (browser, device, UI test checks), by the tool's title or arguments.
pub fn is_app_tool(title: &str, raw_input: Option<&serde_json::Value>) -> bool {
    // Agents show the tool as `mcp__pilothouse__browser_open`,
    // `pilothouse.browser_open` or `browser open · pilothouse`.
    let named = |text: &str| {
        let text = text.to_lowercase().replace([' ', '-'], "_");
        text.contains("pilothouse")
            && ["browser_", "device_", "test_expect", "ports_list"]
                .iter()
                .any(|tool| text.contains(tool))
    };
    named(title) || raw_input.is_some_and(|input| named(&input.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allows_reading_only() {
        assert!(read_only_command("git diff origin/main...HEAD"));
        assert!(read_only_command("git --no-pager log -5 --oneline"));
        assert!(read_only_command("rg -n retry src"));
        assert!(read_only_command("/usr/bin/grep -R TODO ."));
        assert!(!read_only_command("git push origin main"));
        assert!(!read_only_command("git diff --output=/tmp/x"));
        assert!(!read_only_command("rm -rf src"));
        assert!(!read_only_command("cat a.txt > b.txt"));
        assert!(!read_only_command("git log; rm -rf ."));
        assert!(!read_only_command("ls $(rm -rf /)"));
        assert!(!read_only_command("find . -delete"));
        assert!(!read_only_command(""));
    }

    #[test]
    fn reads_commands_from_input_or_title() {
        let input = json!({ "command": "git show HEAD" });
        assert!(allows(Some(ToolKind::Execute), Some(&input), ""));
        assert!(allows(
            Some(ToolKind::Execute),
            None,
            "Run command `git diff origin/main...HEAD`"
        ));
        assert!(!allows(
            Some(ToolKind::Execute),
            None,
            "Run command `npm install`"
        ));
        assert!(allows(Some(ToolKind::Read), None, "Read file a.rs"));
        assert!(!allows(Some(ToolKind::Edit), None, "Write file a.rs"));
        assert!(!allows(None, None, "Something"));
    }

    #[test]
    fn app_tools_are_recognised_by_name() {
        assert!(is_app_tool("mcp__pilothouse__browser_click", None));
        assert!(is_app_tool("browser open · pilothouse", None));
        assert!(is_app_tool("test expect · pilothouse", None));
        assert!(is_app_tool(
            "Tool",
            Some(&serde_json::json!({ "server": "pilothouse", "tool": "device_tap" }))
        ));
        assert!(!is_app_tool("mcp__other__browser_click", None));
        assert!(!is_app_tool("Write file", None));
    }
}
