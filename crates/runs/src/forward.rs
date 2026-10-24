//! Tunnels to servers' ports: a server on a mapped network drive's machine
//! listens there, so to open it here a port on this computer is forwarded
//! to it over SSH (`ssh -L`).
//!
//! Each tunnel is an `ssh` of its own (never a shared connection, which
//! would hold the forward after the `ssh` that asked for it is gone), whose
//! remote command reads its stdin until it closes. PilotHouse holds that
//! stdin, so when PilotHouse exits, however it exits, the remote command
//! ends and so does the tunnel: none is left behind.

use std::io::Read as _;
use std::net::TcpListener;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use ph_remote::Mapping;

/// One server's port, as reached from here.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    host: String,
    ssh_port: Option<u16>,
    port: u16,
    address: String,
}

struct Tunnel {
    target: Target,
    local: u16,
    child: Child,
    _stdin: ChildStdin,
}

static TUNNELS: Mutex<Vec<Tunnel>> = Mutex::new(Vec::new());

/// Where to connect on the server for a port listening on `address`: that
/// address when it is one (`::1`, `10.0.0.5`), else the server itself.
fn connect_to(address: &str) -> String {
    match address.trim_matches(['[', ']']) {
        "" | "*" | "0.0.0.0" | "::" | "127.0.0.1" => "127.0.0.1".into(),
        ip if ip.contains(':') => format!("[{ip}]"),
        ip => ip.into(),
    }
}

/// The port on this computer (on `127.0.0.1`) that reaches `port`, which
/// listens on `address`, on `mapping`'s server; a tunnel is opened when there
/// is none yet. Blocks until it listens.
pub fn open(mapping: &Mapping, port: u16, address: &str) -> Result<u16> {
    let target = Target {
        host: mapping.host.clone(),
        ssh_port: mapping.port,
        port,
        address: connect_to(address),
    };
    {
        let mut tunnels = TUNNELS.lock().unwrap_or_else(|err| err.into_inner());
        tunnels.retain_mut(|tunnel| matches!(tunnel.child.try_wait(), Ok(None)));
        if let Some(tunnel) = tunnels.iter().find(|tunnel| tunnel.target == target) {
            return Ok(tunnel.local);
        }
    }
    // The same port here when it is free (links keep working), else any.
    let local = TcpListener::bind(("127.0.0.1", port))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .and_then(|listener| listener.local_addr())
        .context("finding a free port")?
        .port();
    let mut command = ph_remote::ssh::ssh();
    command
        .args(ph_remote::ssh::options(
            mapping,
            false,
            ph_remote::Connection::Own,
        ))
        .args(["-o", "ExitOnForwardFailure=yes", "-L"])
        .arg(format!("127.0.0.1:{local}:{}:{port}", target.address))
        .arg("--")
        .arg(&mapping.host)
        .arg("cat >/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("running ssh")?;
    let stdin = child.stdin.take().context("ssh stdin")?;
    // Read what ssh says all along, or a busy tunnel's messages ("connect
    // failed") would fill the pipe and stop it. The last of it explains a
    // failure.
    let said = Arc::new(Mutex::new(String::new()));
    if let Some(mut pipe) = child.stderr.take() {
        let said = said.clone();
        std::thread::Builder::new()
            .name("ssh-tunnel-stderr".into())
            .spawn(move || {
                let mut buffer = [0u8; 1024];
                while let Ok(read) = pipe.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let mut said = said.lock().unwrap_or_else(|err| err.into_inner());
                    said.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    if said.len() > 4096 {
                        let cut = said.len() - 2048;
                        let cut = (cut..said.len())
                            .find(|ix| said.is_char_boundary(*ix))
                            .unwrap_or(said.len());
                        said.drain(..cut);
                    }
                }
            })
            .ok();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", local)).is_ok() {
            break;
        }
        if let Ok(Some(_)) = child.try_wait() {
            std::thread::sleep(Duration::from_millis(50));
            let said = said.lock().unwrap_or_else(|err| err.into_inner()).clone();
            bail!(
                "{}",
                ph_remote::explain_failure(Some(255), &said, &mapping.host).unwrap_or_else(|| {
                    format!("the tunnel to {} closed: {}", mapping.host, said.trim())
                })
            );
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            bail!("the tunnel to {}:{port} did not open in time", mapping.host);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    TUNNELS
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .push(Tunnel {
            target,
            local,
            child,
            _stdin: stdin,
        });
    Ok(local)
}

/// Closes every tunnel (at quit).
pub fn close_all() {
    let tunnels = std::mem::take(&mut *TUNNELS.lock().unwrap_or_else(|err| err.into_inner()));
    for mut tunnel in tunnels {
        let _ = tunnel.child.kill();
        let _ = tunnel.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnels_reach_the_address_the_port_listens_on() {
        assert_eq!(connect_to("0.0.0.0"), "127.0.0.1");
        assert_eq!(connect_to("*"), "127.0.0.1");
        assert_eq!(connect_to("[::]"), "127.0.0.1");
        assert_eq!(connect_to("::1"), "[::1]");
        assert_eq!(connect_to("[::1]"), "[::1]");
        assert_eq!(connect_to("10.0.0.5"), "10.0.0.5");
    }
}
