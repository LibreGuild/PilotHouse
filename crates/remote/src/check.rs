//! What Settings checks before a mapping is used: the server's key (shown
//! as a fingerprint for the user to trust), that the user can sign in, and
//! that the folder here and the one on the server really are one folder.
//! Each check blocks; run them on a background thread.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use sha2::Digest as _;

use crate::mapping::{Mapping, remote_in};
use crate::ssh::{self, known_hosts_file, quote};

/// Where `ssh` really connects for a host: `~/.ssh/config` can rename it,
/// and a host-key alias decides the name its key is filed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub hostname: String,
    pub port: u16,
    pub user: String,
    /// The name in `known_hosts` (`[host]:port` off port 22).
    pub key_name: String,
}

pub fn resolve(mapping: &Mapping) -> Result<Resolved> {
    let mut command = ssh::ssh();
    command.arg("-G");
    if let Some(port) = mapping.port {
        command.args(["-p", &port.to_string()]);
    }
    let output = command
        .arg("--")
        .arg(&mapping.host)
        .stdin(std::process::Stdio::null())
        .output()
        .context("running ssh (is OpenSSH installed?)")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    parse_resolved(&String::from_utf8_lossy(&output.stdout))
}

fn parse_resolved(config: &str) -> Result<Resolved> {
    let value = |key: &str| {
        config.lines().find_map(|line| {
            let (name, value) = line.split_once(' ')?;
            (name == key).then(|| value.trim().to_string())
        })
    };
    let hostname = value("hostname").context("ssh did not say where it connects")?;
    let port = value("port")
        .and_then(|port| port.parse().ok())
        .unwrap_or(22);
    let user = value("user").unwrap_or_default();
    // A host-key alias is the name as it is, whatever the port.
    let key_name = match value("hostkeyalias") {
        Some(alias) => alias,
        None if port == 22 => hostname.clone(),
        None => format!("[{hostname}]:{port}"),
    };
    Ok(Resolved {
        hostname,
        port,
        user,
        key_name,
    })
}

/// One of the server's keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// `ssh-ed25519`, `ecdsa-sha2-nistp256`, `ssh-rsa`…
    pub kind: String,
    /// `SHA256:…`, as `ssh-keygen -l` and the server's admin show it.
    pub fingerprint: String,
    /// The `known_hosts` line for it, as `ssh` wrote it (under the name
    /// `ssh` looks it up by).
    pub line: String,
}

/// The key the server presents, to show before trusting. `ssh` itself
/// fetches it, into a scratch `known_hosts`, so `~/.ssh/config` applies as
/// for every other connection (jump hosts, host-key aliases, ports).
pub fn host_keys(mapping: &Mapping) -> Result<Vec<HostKey>> {
    let dir = known_hosts_file()
        .and_then(|file| file.parent().map(PathBuf::from))
        .context("PilotHouse's SSH folder is not set")?;
    ssh::private_dir(&dir)?;
    let scratch = dir.join(format!("scan-{:016x}", probe_id()));
    let mut command = ssh::ssh();
    command
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
        ])
        .args(["-o", "ControlPath=none", "-o", "ConnectTimeout=10"])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile=\"{}\"",
            scratch.to_string_lossy().replace('\\', "/")
        ));
    if let Some(port) = mapping.port {
        command.args(["-p", &port.to_string()]);
    }
    let output = command
        .arg("--")
        .arg(&mapping.host)
        .arg("true")
        .stdin(std::process::Stdio::null())
        .output()
        .context("running ssh (is OpenSSH installed?)");
    let text = std::fs::read_to_string(&scratch).unwrap_or_default();
    let _ = std::fs::remove_file(&scratch);
    let _ = std::fs::remove_file(scratch.with_extension("old"));
    let keys = parse_known_hosts(&text);
    if keys.is_empty() {
        let stderr = output
            .map(|output| String::from_utf8_lossy(&output.stderr).trim().to_string())
            .unwrap_or_else(|err| format!("{err:#}"));
        bail!("{} sent no host key: {stderr}", mapping.host);
    }
    Ok(keys)
}

fn parse_known_hosts(text: &str) -> Vec<HostKey> {
    let mut keys: Vec<HostKey> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (_, kind, key) = (fields.next()?, fields.next()?, fields.next()?);
            let blob = base64::engine::general_purpose::STANDARD.decode(key).ok()?;
            let digest = sha2::Sha256::digest(&blob);
            Some(HostKey {
                kind: kind.to_string(),
                fingerprint: format!(
                    "SHA256:{}",
                    base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
                ),
                line: line.trim().to_string(),
            })
        })
        .collect();
    keys.dedup_by(|a, b| a.fingerprint == b.fingerprint);
    keys
}

/// Files `keys` as the server's in PilotHouse's `known_hosts`.
pub fn trust(keys: &[HostKey]) -> Result<()> {
    let file = known_hosts_file().context("PilotHouse's SSH folder is not set")?;
    if let Some(dir) = file.parent() {
        ssh::private_dir(dir)?;
    }
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    for key in keys {
        if !text.lines().any(|existing| existing == key.line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&key.line);
            text.push('\n');
        }
    }
    std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))
}

/// Removes the server's keys trusted in PilotHouse (after the server was
/// reinstalled, say). Keys in the user's own `known_hosts` stay; for those
/// the user runs [`forget_command`] themselves.
pub fn forget(mapping: &Mapping) -> Result<()> {
    let resolved = resolve(mapping)?;
    let Some(file) = known_hosts_file().filter(|file| file.exists()) else {
        return Ok(());
    };
    let output = ph_env::command("ssh-keygen")
        .args(["-R", &resolved.key_name, "-f"])
        .arg(&file)
        .stdin(std::process::Stdio::null())
        .output()
        .context("running ssh-keygen")?;
    let _ = std::fs::remove_file(file.with_extension("old"));
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// The command that removes the server's key from the user's own
/// `known_hosts`.
pub fn forget_command(mapping: &Mapping) -> String {
    let name = resolve(mapping)
        .map(|resolved| resolved.key_name)
        .unwrap_or_else(|_| mapping.host.clone());
    format!("ssh-keygen -R \"{name}\"")
}

/// Whether signing in works, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    Ready,
    /// The server's key is not trusted yet.
    UnknownHost,
    /// The server's key differs from the one trusted.
    ChangedHost,
    /// The server refused the user's keys (or wants a password).
    NoKey,
    Unreachable(String),
}

pub fn access(mapping: &Mapping) -> Access {
    // A connection of its own: a shared one someone signed in to with a
    // password would pass the check.
    let output = ssh::command(mapping, "true", ssh::Connection::Own)
        .stdin(std::process::Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) => return Access::Unreachable(format!("running ssh: {err}")),
    };
    if output.status.success() {
        return Access::Ready;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        Access::ChangedHost
    } else if stderr.contains("Host key verification failed")
        || stderr.contains("host key is known")
    {
        Access::UnknownHost
    } else if stderr.contains("Permission denied") {
        Access::NoKey
    } else {
        let first = stderr.lines().map(str::trim).find(|line| !line.is_empty());
        Access::Unreachable(first.unwrap_or("ssh failed").to_string())
    }
}

/// What checking the folder found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// `mapping.local` as the system resolves it (for [`Mapping::resolved`]).
    pub resolved: String,
    /// The server's Git (`git version 2.43.0`), if it has one.
    pub git: Option<String>,
}

/// Proves the folder here and the one on the server are one folder: a small
/// file written here must be there. The file is removed after.
pub fn same_folder(mapping: &Mapping) -> Result<Checked> {
    let local = PathBuf::from(&mapping.local);
    if !local.is_dir() {
        bail!("{} is not a folder on this computer", mapping.local);
    }
    let resolved = dunce::canonicalize(&local)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| mapping.local.clone());
    let name = format!(".pilothouse-check-{:016x}", probe_id());
    let probe = local.join(&name);
    std::fs::write(
        &probe,
        b"PilotHouse checks this folder is the one on the server.\n",
    )
    .with_context(|| format!("cannot write in {} to check it", mapping.local))?;
    let remote = remote_in(
        std::slice::from_ref(mapping),
        &probe.to_string_lossy(),
        cfg!(windows),
    )
    .map(|remote| remote.path)
    .unwrap_or_else(|| format!("{}/{name}", mapping.remote.trim_end_matches('/')));
    let script = format!(
        "test -f {} && echo same; git --version 2>/dev/null || true",
        quote(&remote)
    );
    let output = ssh::command(mapping, &script, ssh::Connection::Own)
        .stdin(std::process::Stdio::null())
        .output();
    let _ = std::fs::remove_file(&probe);
    let output = output.context("running ssh")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            ssh::explain_failure(output.status.code(), &stderr, &mapping.host)
                .unwrap_or_else(|| stderr.trim().to_string())
        ));
    }
    if !stdout.lines().any(|line| line.trim() == "same") {
        bail!(
            "{} on the server is not the same folder as {} here: a file written here did not appear there",
            mapping.remote,
            mapping.local
        );
    }
    Ok(Checked {
        resolved,
        git: stdout
            .lines()
            .find(|line| line.starts_with("git version"))
            .map(|line| line.trim().to_string()),
    })
}

fn probe_id() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();
    nanos ^ (u64::from(std::process::id()) << 32)
}

/// The folder Samba shares as `share` on the server, if `testparm` says.
pub fn share_path(mapping: &Mapping, share: &str) -> Option<String> {
    if share.is_empty() || share.contains(['\'', '\n']) {
        return None;
    }
    let script = format!(
        "testparm -s --section-name={} --parameter-name=path 2>/dev/null",
        quote(share)
    );
    let output = ssh::command(mapping, &script, ssh::Connection::Own)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    // `testparm` exits 1 for a mere warning about the config, the path
    // printed all the same; only ssh's own failure (255) means no answer.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout.lines().next().unwrap_or_default().trim().to_string();
    (output.status.code() != Some(255) && path.starts_with('/')).then_some(path)
}

/// The user's public key (`~/.ssh/id_ed25519.pub`, else ECDSA, else RSA).
pub fn public_key() -> Option<(PathBuf, String)> {
    let dir = ssh_dir()?;
    ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"]
        .iter()
        .map(|name| dir.join(name))
        .find_map(|path| {
            let key = std::fs::read_to_string(&path).ok()?.trim().to_string();
            valid_public_key(&key).then_some((path, key))
        })
}

/// `ssh-add` arguments that add the private key of `public` (its `.pub`
/// file) to the user's SSH agent, for a terminal: it asks for the
/// passphrase once, then commands can use the key.
pub fn agent_add_args(public: &std::path::Path) -> (String, Vec<String>) {
    let private = public.with_extension("").to_string_lossy().into_owned();
    if cfg!(target_os = "macos") {
        // Apple's own ssh-add (Homebrew's lacks the option), keeping the
        // passphrase in the keychain so the key is there after a restart.
        return (
            "/usr/bin/ssh-add".into(),
            vec!["--apple-use-keychain".into(), private],
        );
    }
    ("ssh-add".into(), vec![private])
}

/// `~/.ssh`.
pub fn ssh_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".ssh"))
}

/// One line of a public key file, with nothing a shell would read.
fn valid_public_key(key: &str) -> bool {
    let mut fields = key.split_whitespace();
    let kind_ok = fields
        .next()
        .is_some_and(|kind| kind.starts_with("ssh-") || kind.starts_with("ecdsa-"));
    kind_ok
        && fields.next().is_some()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " +/=@._-:".contains(c))
}

/// `ssh-keygen` arguments that make `~/.ssh/id_ed25519` (it asks for a
/// passphrase in the terminal).
pub fn keygen_args() -> Option<(String, Vec<String>)> {
    let dir = ssh_dir()?;
    ssh::private_dir(&dir).ok()?;
    let file = dir.join("id_ed25519");
    Some((
        "ssh-keygen".into(),
        vec![
            "-t".into(),
            "ed25519".into(),
            "-f".into(),
            file.to_string_lossy().into_owned(),
        ],
    ))
}

/// `ssh` arguments that add `key` to the server's `authorized_keys`, for a
/// terminal: the user signs in with their password once.
pub fn install_key_args(mapping: &Mapping, key: &str) -> Option<(String, Vec<String>)> {
    if !valid_public_key(key) {
        return None;
    }
    let key = quote(key);
    let script = format!(
        "umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && {{ grep -qxF {key} ~/.ssh/authorized_keys || echo {key} >> ~/.ssh/authorized_keys; }} && echo 'PilotHouse: your key is installed. You can close this.'"
    );
    // Not shared: signing in with the password must not leave a shared
    // connection that would pass the next check without the key.
    let mut args = ssh::options(mapping, true, ssh::Connection::Own);
    args.extend(["--".into(), mapping.host.clone(), script]);
    Some(("ssh".into(), args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_says_where_it_connects() {
        let config = "user dev\nhostname 10.0.0.5\nport 2222\nbatchmode no\n";
        let resolved = parse_resolved(config).unwrap();
        assert_eq!(resolved.hostname, "10.0.0.5");
        assert_eq!(resolved.key_name, "[10.0.0.5]:2222");
        let aliased = parse_resolved("hostname nas\nport 2222\nhostkeyalias buildbox\n").unwrap();
        assert_eq!(aliased.key_name, "buildbox", "an alias has no port");
    }

    #[test]
    fn host_keys_have_their_fingerprints() {
        // A key and the fingerprint `ssh-keygen -lf` gives for it.
        let scan = "# nas:22 SSH-2.0-OpenSSH_9.7\nnas ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIF8DM4HgKqFlPkGHaNV6c9sAw4uGyStK/hW5D7Ichg8q\n";
        let keys = parse_known_hosts(scan);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kind, "ssh-ed25519");
        assert_eq!(
            keys[0].fingerprint,
            "SHA256:5jNzoWWaj8z+D6whwsR+0Cl8+ZMm2ZigTNlsJtpYwjY"
        );
    }

    #[test]
    fn only_plain_public_keys_are_installed() {
        assert!(valid_public_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0 dev@laptop"
        ));
        assert!(!valid_public_key("ssh-ed25519 AAAA'; rm -rf ~; echo '"));
        assert!(!valid_public_key("-----BEGIN OPENSSH PRIVATE KEY-----"));
        let mapping = Mapping {
            host: "dev@nas".into(),
            ..Default::default()
        };
        let (_, args) = install_key_args(&mapping, "ssh-ed25519 AAAAC3Nz dev@laptop").unwrap();
        assert!(
            args.last()
                .unwrap()
                .contains("'ssh-ed25519 AAAAC3Nz dev@laptop'")
        );
    }
}
