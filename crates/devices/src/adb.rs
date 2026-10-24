//! Blocking calls to `adb`. Run them off the UI thread.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use ph_media::Frame;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub serial: String,
    /// `device`, `offline`, `unauthorized`…
    pub state: String,
    /// The model for phones, the AVD name for emulators.
    pub name: String,
    pub emulator: bool,
}

impl DeviceInfo {
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }

    /// The name for people: AVD ids use underscores for spaces.
    pub fn display_name(&self) -> String {
        self.name.replace('_', " ")
    }
}

/// A file or folder on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct Adb {
    path: PathBuf,
}

impl Adb {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn command(&self, serial: Option<&str>) -> Command {
        let mut command = ph_env::command(&self.path);
        if let Some(serial) = serial {
            command.args(["-s", serial]);
        }
        command.stdin(Stdio::null());
        command
    }

    /// Runs adb and returns stdout; fails with stderr on a non-zero exit.
    pub fn run(&self, serial: Option<&str>, args: &[&str]) -> Result<Vec<u8>> {
        let output = self
            .command(serial)
            .args(args)
            .output()
            .with_context(|| format!("running adb {}", args.join(" ")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let message = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            bail!("adb {}: {}", args.first().unwrap_or(&""), message.trim());
        }
        Ok(output.stdout)
    }

    pub fn text(&self, serial: Option<&str>, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.run(serial, args)?).into_owned())
    }

    pub fn shell(&self, serial: &str, command: &str) -> Result<String> {
        self.text(Some(serial), &["shell", command])
    }

    pub fn devices(&self) -> Result<Vec<DeviceInfo>> {
        let mut devices = parse_devices(&self.text(None, &["devices", "-l"])?);
        for device in devices
            .iter_mut()
            .filter(|device| device.emulator && device.is_ready())
        {
            if let Ok(name) = self.text(Some(&device.serial), &["emu", "avd", "name"]) {
                // The answer is the name, then "OK".
                if let Some(first) = name
                    .lines()
                    .next()
                    .filter(|line| !line.trim().is_empty() && *line != "OK")
                {
                    device.name = first.trim().to_string();
                }
            }
        }
        Ok(devices)
    }

    /// The device screen. Emulators send raw pixels (fast over localhost);
    /// USB devices send PNG (smaller over the cable).
    pub fn screencap(&self, serial: &str, raw: bool) -> Result<Frame> {
        if raw {
            let bytes = self.run(Some(serial), &["exec-out", "screencap"])?;
            if let Some(frame) = parse_raw_screencap(&bytes) {
                return Ok(frame);
            }
        }
        Frame::decode(&self.run(Some(serial), &["exec-out", "screencap", "-p"])?)
    }

    /// The UI hierarchy as uiautomator XML.
    pub fn ui_dump(&self, serial: &str) -> Result<String> {
        let path = "/data/local/tmp/pilothouse-ui.xml";
        self.shell(
            serial,
            &format!("uiautomator dump {path} >/dev/null && cat {path}"),
        )
    }

    pub fn tap(&self, serial: &str, x: i32, y: i32) -> Result<()> {
        self.shell(serial, &format!("input tap {x} {y}")).map(drop)
    }

    pub fn swipe(&self, serial: &str, from: (i32, i32), to: (i32, i32), ms: u32) -> Result<()> {
        self.shell(
            serial,
            &format!("input swipe {} {} {} {} {ms}", from.0, from.1, to.0, to.1),
        )
        .map(drop)
    }

    pub fn type_text(&self, serial: &str, text: &str) -> Result<()> {
        self.shell(serial, &input_text_command(text)).map(drop)
    }

    pub fn key(&self, serial: &str, keycode: u32) -> Result<()> {
        self.shell(serial, &format!("input keyevent {keycode}"))
            .map(drop)
    }

    pub fn install(&self, serial: &str, apk: &Path) -> Result<()> {
        let apk = apk.to_string_lossy();
        let output = self.text(Some(serial), &["install", "-r", &apk])?;
        if output.contains("Failure") {
            bail!("{}", output.trim());
        }
        Ok(())
    }

    pub fn launch(&self, serial: &str, package: &str) -> Result<()> {
        if !package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        {
            bail!("`{package}` is not a package name");
        }
        let output = self.shell(
            serial,
            &format!("monkey -p {package} -c android.intent.category.LAUNCHER 1"),
        )?;
        if output.contains("No activities found") {
            bail!("{package} has no launchable activity");
        }
        Ok(())
    }

    /// Apps the user installed (not system apps), sorted.
    pub fn packages(&self, serial: &str) -> Result<Vec<String>> {
        let mut packages: Vec<String> = self
            .shell(serial, "pm list packages -3")?
            .lines()
            .filter_map(|line| line.trim().strip_prefix("package:").map(str::to_string))
            .collect();
        packages.sort();
        Ok(packages)
    }

    /// Recent log lines, oldest first (`logcat -d`).
    pub fn logcat_dump(&self, serial: &str, lines: usize) -> Result<String> {
        self.text(
            Some(serial),
            &["logcat", "-d", "-v", "threadtime", "-t", &lines.to_string()],
        )
    }

    pub fn list_dir(&self, serial: &str, dir: &str) -> Result<Vec<RemoteFile>> {
        let dir = shell_quote(&format!("{}/", dir.trim_end_matches('/')));
        Ok(parse_ls(&self.shell(serial, &format!("ls -la {dir}"))?))
    }

    pub fn pull(&self, serial: &str, remote: &str, local: &Path) -> Result<()> {
        self.run(Some(serial), &["pull", remote, &local.to_string_lossy()])
            .map(drop)
    }

    pub fn push(&self, serial: &str, local: &Path, remote_dir: &str) -> Result<()> {
        self.run(
            Some(serial),
            &["push", &local.to_string_lossy(), remote_dir],
        )
        .map(drop)
    }

    pub fn boot_completed(&self, serial: &str) -> bool {
        self.shell(serial, "getprop sys.boot_completed")
            .is_ok_and(|value| value.trim() == "1")
    }

    pub fn kill_emulator(&self, serial: &str) -> Result<()> {
        self.run(Some(serial), &["emu", "kill"]).map(drop)
    }
}

fn parse_devices(text: &str) -> Vec<DeviceInfo> {
    text.lines()
        .skip_while(|line| !line.starts_with("List of devices"))
        .skip(1)
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let model = parts
                .find_map(|part| part.strip_prefix("model:"))
                .map(|model| model.replace('_', " "));
            let emulator = serial.starts_with("emulator-");
            Some(DeviceInfo {
                name: model.unwrap_or_else(|| serial.clone()),
                serial,
                state,
                emulator,
            })
        })
        .collect()
}

/// `screencap` without `-p`: width, height and format as little-endian u32,
/// sometimes a color space u32, then RGBA pixels.
fn parse_raw_screencap(bytes: &[u8]) -> Option<Frame> {
    let word = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let (width, height, format) = (word(0)?, word(4)?, word(8)?);
    let pixels = (width as usize) * (height as usize) * 4;
    let header = bytes.len().checked_sub(pixels)?;
    // 1 = RGBA_8888, 2 = RGBX_8888; anything else falls back to PNG.
    if !(header == 12 || header == 16) || !(format == 1 || format == 2) {
        return None;
    }
    let pixels = &bytes[header..];
    if format == 1 {
        return Some(Frame::from_rgba_slice(width, height, pixels));
    }
    // RGBX: the fourth byte is padding, so make it opaque.
    let mut rgba = pixels.to_vec();
    rgba.chunks_exact_mut(4).for_each(|pixel| pixel[3] = 255);
    Some(Frame::from_rgba(width, height, rgba))
}

/// `input text` takes no spaces (`%s` stands for one) and runs in a shell.
pub fn input_text_command(text: &str) -> String {
    format!("input text {}", shell_quote(&text.replace(' ', "%s")))
}

pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Parses `ls -la` from toybox: permissions, links, owner, group, size,
/// date, time, name.
fn parse_ls(text: &str) -> Vec<RemoteFile> {
    let mut files: Vec<RemoteFile> = text
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 8 || fields[0].len() != 10 {
                return None;
            }
            let name = fields[7..].join(" ");
            let name = name.split(" -> ").next().unwrap_or(&name).to_string();
            if name == "." || name == ".." {
                return None;
            }
            Some(RemoteFile {
                is_dir: fields[0].starts_with('d') || fields[0].starts_with('l'),
                size: fields[4].parse().unwrap_or(0),
                name,
            })
        })
        .collect();
    files.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_list() {
        let devices = parse_devices(
            "List of devices attached\nemulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 transport_id:1\nR58M12ABC   unauthorized usb:1-1 transport_id:2\n\n",
        );
        assert_eq!(devices.len(), 2);
        assert!(devices[0].emulator && devices[0].is_ready());
        assert_eq!(devices[0].name, "sdk gphone64 arm64");
        assert_eq!(devices[1].state, "unauthorized");
    }

    #[test]
    fn parses_raw_screencap_with_and_without_color_space() {
        for header in [12usize, 16] {
            let mut bytes = Vec::new();
            bytes.extend(2u32.to_le_bytes());
            bytes.extend(1u32.to_le_bytes());
            bytes.extend(1u32.to_le_bytes());
            bytes.resize(header, 0);
            bytes.extend([1, 2, 3, 4, 5, 6, 7, 8]);
            let frame = parse_raw_screencap(&bytes).unwrap();
            assert_eq!((frame.width, frame.height), (2, 1));
            assert_eq!(frame.bgra(), &[3, 2, 1, 4, 7, 6, 5, 8]);
        }
    }

    #[test]
    fn quotes_text_for_input() {
        assert_eq!(input_text_command("it's ok"), r"input text 'it'\''s%sok'");
    }

    #[test]
    fn parses_ls() {
        let files = parse_ls(
            "total 8\ndrwxrwx--x 2 root sdcard_rw 4096 2026-10-24 10:00 .\ndrwxrwx--x 2 root sdcard_rw 4096 2026-10-24 10:00 Download\n-rw-rw---- 1 root sdcard_rw 1234 2026-10-24 10:00 my file.txt\n",
        );
        assert_eq!(
            files,
            vec![
                RemoteFile {
                    name: "Download".into(),
                    is_dir: true,
                    size: 4096
                },
                RemoteFile {
                    name: "my file.txt".into(),
                    is_dir: false,
                    size: 1234
                },
            ]
        );
    }
}
