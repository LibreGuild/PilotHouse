//! TCP ports that processes listen on, for the ports panel: this
//! computer's (`lsof`, `netstat`), or a server's (`ss`, over SSH) for a
//! workspace on a mapped network drive.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub port: u16,
    pub address: String,
    pub pid: u32,
    pub process: String,
    /// The process's working directory, when it could be read.
    pub cwd: Option<PathBuf>,
}

impl Port {
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Started from inside `root` (its working directory is there).
    pub fn belongs_to(&self, root: &Path) -> bool {
        self.cwd.as_ref().is_some_and(|cwd| cwd.starts_with(root))
    }
}

/// Listening ports of this user's processes, sorted by port. Empty when the
/// platform tool (`lsof`, or `netstat` on Windows) is not available.
pub fn listening() -> Vec<Port> {
    if cfg!(windows) {
        return windows_listening();
    }
    let Ok(output) = ph_env::command("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
        .output()
    else {
        return Vec::new();
    };
    let mut ports = parse(&String::from_utf8_lossy(&output.stdout));
    let pids: Vec<String> = ports.iter().map(|port| port.pid.to_string()).collect();
    if !pids.is_empty()
        && let Ok(output) = ph_env::command("lsof")
            .args(["-a", "-d", "cwd", "-F", "pn", "-p", &pids.join(",")])
            .output()
    {
        let cwds = parse_cwds(&String::from_utf8_lossy(&output.stdout));
        for port in &mut ports {
            port.cwd = cwds
                .iter()
                .find(|(pid, _)| *pid == port.pid)
                .map(|(_, cwd)| cwd.clone());
        }
    }
    ports
}

/// `netstat -ano` for listening TCP ports, named with `tasklist`. Windows
/// does not report a process's working directory, so no port is marked as
/// this project's.
fn windows_listening() -> Vec<Port> {
    let Ok(netstat) = ph_env::command("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
    else {
        return Vec::new();
    };
    let mut ports = parse_netstat(&String::from_utf8_lossy(&netstat.stdout));
    if let Ok(output) = ph_env::command("netstat")
        .args(["-ano", "-p", "TCPv6"])
        .output()
    {
        for port in parse_netstat(&String::from_utf8_lossy(&output.stdout)) {
            if !ports
                .iter()
                .any(|known| known.port == port.port && known.pid == port.pid)
            {
                ports.push(port);
            }
        }
    }
    if let Ok(output) = ph_env::command("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
    {
        let names = parse_tasklist(&String::from_utf8_lossy(&output.stdout));
        for port in &mut ports {
            if let Some((_, name)) = names.iter().find(|(pid, _)| *pid == port.pid) {
                port.process = name.clone();
            }
        }
    }
    ports.sort_by_key(|port| port.port);
    ports
}

/// `  TCP    0.0.0.0:5173    0.0.0.0:0    LISTENING    4242`
fn parse_netstat(text: &str) -> Vec<Port> {
    let mut ports: Vec<Port> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [protocol, local, _, state, pid] = fields[..] else {
            continue;
        };
        if !protocol.eq_ignore_ascii_case("TCP") || state != "LISTENING" {
            continue;
        }
        let Some((address, port)) = local.rsplit_once(':') else {
            continue;
        };
        let (Ok(port), Ok(pid)) = (port.parse(), pid.parse()) else {
            continue;
        };
        if ports
            .iter()
            .any(|known| known.port == port && known.pid == pid)
        {
            continue;
        }
        ports.push(Port {
            port,
            address: address.trim_matches(['[', ']']).to_string(),
            pid,
            process: format!("pid {pid}"),
            cwd: None,
        });
    }
    ports
}

/// `"node.exe","4242","Console","1","52,000 K"`
fn parse_tasklist(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split("\",\"");
            let name = fields.next()?.trim_start_matches('"').to_string();
            let pid = fields.next()?.parse().ok()?;
            Some((pid, name))
        })
        .collect()
}

/// `lsof -F pcn`: `p<pid>`, `c<command>`, then `n<address:port>` lines.
fn parse(text: &str) -> Vec<Port> {
    let mut ports: Vec<Port> = Vec::new();
    let (mut pid, mut process) = (0, String::new());
    for line in text.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => pid = value.parse().unwrap_or(0),
            "c" => process = value.to_string(),
            "n" => {
                let Some((address, port)) = value.rsplit_once(':') else {
                    continue;
                };
                let Ok(port) = port.parse() else { continue };
                // The same port on IPv4 and IPv6 is one entry.
                if ports
                    .iter()
                    .any(|known| known.port == port && known.pid == pid)
                {
                    continue;
                }
                ports.push(Port {
                    port,
                    address: address.to_string(),
                    pid,
                    process: process.clone(),
                    cwd: None,
                });
            }
            _ => {}
        }
    }
    ports.sort_by_key(|port| port.port);
    ports
}

fn parse_cwds(text: &str) -> Vec<(u32, PathBuf)> {
    let mut cwds = Vec::new();
    let mut pid = 0;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().unwrap_or(0);
        } else if let Some(path) = line.strip_prefix('n') {
            cwds.push((pid, PathBuf::from(path)));
        }
    }
    cwds
}

/// Listening ports on `mapping`'s server (`ss`, Linux), with the working
/// folders of the user's own processes as this computer's paths. Empty when
/// the server cannot be reached or has no `ss`.
pub fn listening_on(mapping: &ph_remote::Mapping) -> Vec<Port> {
    // One `ss`, for the ports and their processes' folders alike.
    let script = "listing=$(ss -ltnpH 2>/dev/null); printf '%s\\n' \"$listing\"; echo '--- cwd'; for pid in $(printf '%s\\n' \"$listing\" | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do printf '%s %s\\n' \"$pid\" \"$(readlink /proc/$pid/cwd 2>/dev/null)\"; done";
    let Ok(output) = ph_remote::ssh::command(mapping, script, ph_remote::Connection::Shared)
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    parse_ss(&String::from_utf8_lossy(&output.stdout), mapping)
}

/// `ss -ltnpH` lines, then `--- cwd` and `<pid> <folder>` lines.
fn parse_ss(text: &str, mapping: &ph_remote::Mapping) -> Vec<Port> {
    let (listing, cwds) = text.split_once("--- cwd").unwrap_or((text, ""));
    let cwds: Vec<(u32, String)> = cwds
        .lines()
        .filter_map(|line| {
            let (pid, cwd) = line.trim().split_once(' ')?;
            Some((pid.parse().ok()?, cwd.to_string()))
        })
        .collect();
    let mut ports: Vec<Port> = Vec::new();
    for line in listing.lines() {
        // LISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:(("node",pid=812,fd=21))
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(local) = fields.get(3) else { continue };
        let Some((address, port)) = local.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port.parse::<u16>() else {
            continue;
        };
        let users = fields.get(5).copied().unwrap_or_default();
        let process = users
            .split('"')
            .nth(1)
            .unwrap_or("(another user)")
            .to_string();
        let pid = users
            .split("pid=")
            .nth(1)
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|pid| pid.parse().ok())
            .unwrap_or(0);
        if ports
            .iter()
            .any(|known| known.port == port && known.pid == pid)
        {
            continue;
        }
        let cwd = cwds
            .iter()
            .find(|(known, _)| *known == pid && pid != 0)
            .and_then(|(_, cwd)| ph_remote::to_local(mapping, cwd));
        ports.push(Port {
            port,
            address: address.trim_matches(['[', ']']).to_string(),
            pid,
            process,
            cwd,
        });
    }
    ports.sort_by_key(|port| port.port);
    ports
}

/// Stops process `pid` on `mapping`'s server.
pub fn stop_on(mapping: &ph_remote::Mapping, pid: u32) -> anyhow::Result<()> {
    // `ss` shows no process for other users' ports; `kill 0` would signal
    // the command's own group.
    anyhow::ensure!(pid != 0, "that port's process belongs to another user");
    let output = ph_remote::ssh::command(
        mapping,
        &format!("kill {pid}"),
        ph_remote::Connection::Shared,
    )
    .stdin(std::process::Stdio::null())
    .output()?;
    anyhow::ensure!(
        output.status.success(),
        "could not stop process {pid} on {}: {}",
        mapping.host,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Asks a process to stop: SIGTERM, or on Windows `taskkill` (forced, with
/// its children, since console servers ignore a polite close).
pub fn stop(pid: u32) -> anyhow::Result<()> {
    let status = if cfg!(windows) {
        ph_env::command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()?
    } else {
        ph_env::command("kill").arg(pid.to_string()).status()?
    };
    anyhow::ensure!(status.success(), "could not stop process {pid}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ss_output_from_a_server() {
        let text = "LISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:((\"node\",pid=812,fd=21))\nLISTEN 0 128 [::]:22 [::]:*\nLISTEN 0 511 [::1]:5173 [::]:* users:((\"node\",pid=812,fd=22))\n--- cwd\n812 /srv/code/app\n";
        let ports = parse_ss(text, &ph_remote::Mapping::default());
        assert_eq!(ports.len(), 2);
        assert_eq!((ports[0].port, ports[0].pid), (22, 0));
        assert_eq!(ports[0].process, "(another user)");
        assert_eq!(
            (ports[1].port, ports[1].process.as_str(), ports[1].pid),
            (5173, "node", 812)
        );
    }

    #[test]
    fn parses_lsof_output() {
        let ports = parse("p501\ncnode\nn*:5173\nn[::1]:5173\np77\ncpython3\nn127.0.0.1:8000\n");
        assert_eq!(ports.len(), 2);
        assert_eq!(
            (ports[0].port, ports[0].process.as_str(), ports[0].pid),
            (5173, "node", 501)
        );
        assert_eq!(ports[1].address, "127.0.0.1");
        let windows = parse_netstat(
            "  Proto  Local Address  Foreign Address  State  PID\n  TCP    0.0.0.0:5173   0.0.0.0:0   LISTENING   4242\n  TCP    [::]:5173   [::]:0   LISTENING   4242\n  TCP    127.0.0.1:50000   127.0.0.1:443   ESTABLISHED   7\n",
        );
        assert_eq!(windows.len(), 1);
        assert_eq!((windows[0].port, windows[0].pid), (5173, 4242));
        assert_eq!(
            parse_tasklist("\"node.exe\",\"4242\",\"Console\",\"1\",\"52,000 K\"\n"),
            vec![(4242, "node.exe".to_string())]
        );
        assert_eq!(
            parse_cwds("p501\nfcwd\nn/work/app\n"),
            vec![(501, PathBuf::from("/work/app"))]
        );
    }
}
