//! Tunnels to servers' ports: a server on a mapped network drive's machine
//! listens there, so to open it here a port on this computer is forwarded
//! to it over SSH (`ssh -L`).
//!
//! Each tunnel is an `ssh` whose remote command reads its stdin until it
//! closes. PilotHouse holds that stdin, so when PilotHouse exits, however
//! it exits, the remote command ends and so does the tunnel: none is left
//! behind.

use std::net::TcpListener;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use ph_remote::Mapping;

struct Tunnel {
    host: String,
    port: u16,
    local: u16,
    child: Child,
    _stdin: ChildStdin,
}

static TUNNELS: Mutex<Vec<Tunnel>> = Mutex::new(Vec::new());

/// The port on this computer that reaches `port` on `mapping`'s server,
/// opening a tunnel when there is none yet. Blocks until the tunnel listens.
pub fn open(mapping: &Mapping, port: u16) -> Result<u16> {
    let mut tunnels = TUNNELS.lock().unwrap_or_else(|err| err.into_inner());
    tunnels.retain_mut(|tunnel| matches!(tunnel.child.try_wait(), Ok(None)));
    if let Some(tunnel) = tunnels
        .iter()
        .find(|tunnel| tunnel.host == mapping.host && tunnel.port == port)
    {
        return Ok(tunnel.local);
    }
    // The same port here when it is free (links keep working), else any.
    let local = TcpListener::bind(("127.0.0.1", port))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .and_then(|listener| listener.local_addr())
        .context("finding a free port")?
        .port();
    let mut command = ph_env::command("ssh");
    command
        .args(ph_remote::ssh::options(mapping, false))
        .args(["-o", "ExitOnForwardFailure=yes", "-L"])
        .arg(format!("127.0.0.1:{local}:127.0.0.1:{port}"))
        .arg("--")
        .arg(&mapping.host)
        .arg("cat >/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("running ssh")?;
    let stdin = child.stdin.take().context("ssh stdin")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", local)).is_ok() {
            break;
        }
        if let Ok(Some(_)) = child.try_wait() {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                use std::io::Read as _;
                let _ = pipe.read_to_string(&mut stderr);
            }
            bail!(
                "{}",
                ph_remote::explain_failure(Some(255), &stderr, &mapping.host).unwrap_or_else(
                    || format!("the tunnel to {} closed: {}", mapping.host, stderr.trim())
                )
            );
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            bail!("the tunnel to {}:{port} did not open in time", mapping.host);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    tunnels.push(Tunnel {
        host: mapping.host.clone(),
        port,
        local,
        child,
        _stdin: stdin,
    });
    Ok(local)
}

/// Closes every tunnel (at quit).
pub fn close_all() {
    let mut tunnels = TUNNELS.lock().unwrap_or_else(|err| err.into_inner());
    for mut tunnel in tunnels.drain(..) {
        let _ = tunnel.child.kill();
        let _ = tunnel.child.wait();
    }
}
