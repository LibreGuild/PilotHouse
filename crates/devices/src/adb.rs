//! Blocking calls to `adb`. Run them off the UI thread.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use ph_media::Frame;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfo {
    pub serial: String,
    /// `device`, `offline`, `unauthorized`…
    pub state: String,
    /// The model for phones, the AVD name for emulators.
    pub name: String,
    pub emulator: bool,
    /// What the device says of itself, once it has been asked.
    pub about: Option<About>,
}

/// What a device says of itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct About {
    /// The device's own serial number (`ro.serialno`): the same for one
    /// phone on a cable and over the network, where adb's serial is its
    /// address.
    pub hardware: String,
    /// Android's version as people say it: `13`.
    pub android: String,
    /// The API level: 33.
    pub api: Option<u32>,
}

impl DeviceInfo {
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }

    /// The name for people: AVD ids use underscores for spaces.
    pub fn display_name(&self) -> String {
        self.name.replace('_', " ")
    }

    /// What tells this device from every other, however it is connected:
    /// its own serial number when known. An emulator is told by adb's
    /// serial (its port), as before.
    pub fn identity(&self) -> &str {
        match &self.about {
            Some(about) if !self.emulator && !about.hardware.is_empty() => &about.hardware,
            _ => &self.serial,
        }
    }

    /// How the device is connected.
    pub fn link(&self) -> Link {
        if self.emulator {
            Link::Emulator
        } else if is_wireless(&self.serial) {
            Link::Network
        } else {
            Link::Usb
        }
    }
}

/// How a device is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Usb,
    Network,
    Emulator,
}

/// A device adb found on the network (`adb mdns services`): a phone with
/// Wireless debugging on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nearby {
    /// The name the device announces: `adb-R5CT1234ABC-x7Yz`.
    pub name: String,
    /// Where it listens: `192.168.1.5:37123`.
    pub address: String,
    /// It waits to be paired (its pairing screen is open), not connected.
    pub pairing: bool,
}

impl Nearby {
    /// The device's own serial number, which the name it announces holds.
    pub fn hardware(&self) -> &str {
        let name = self.name.strip_prefix("adb-").unwrap_or(&self.name);
        name.split('-').next().unwrap_or(name)
    }
}

/// Whether `serial` is a device connected over Wi-Fi: `192.168.1.5:5555`
/// after `adb connect`, `adb-<id>._adb-tls-connect._tcp` after pairing.
/// Emulators (`emulator-5554`) are local.
pub fn is_wireless(serial: &str) -> bool {
    serial.contains("._adb-tls-connect.")
        || (serial.contains(':') && !serial.starts_with("emulator-"))
}

/// Whether `name` looks like an Android package (safe in a shell command).
pub fn is_package_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
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

    /// Runs adb and returns stdout; fails with stderr on a non-zero exit, or
    /// when adb takes too long (a device that went away mid-command): an
    /// agent waits on these.
    pub fn run(&self, serial: Option<&str>, args: &[&str]) -> Result<Vec<u8>> {
        let output = self.output(serial, args)?;
        if !output.status.success() {
            bail!("{}", complaint(args, &output));
        }
        Ok(output.stdout)
    }

    /// Runs adb to its end, whatever its exit status; fails when it cannot
    /// start or takes too long.
    fn output(&self, serial: Option<&str>, args: &[&str]) -> Result<Output> {
        let limit = match args.first() {
            Some(&("install" | "push" | "pull")) => Duration::from_secs(10 * 60),
            // A device that is not there is not waited on for a minute.
            Some(&("connect" | "pair")) => Duration::from_secs(15),
            _ => Duration::from_secs(60),
        };
        let mut command = self.command(serial);
        command.args(args);
        output_within(command, limit).with_context(|| format!("running adb {}", args.join(" ")))
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

    /// What the device says of itself (one call).
    pub fn about(&self, serial: &str) -> Option<About> {
        let text = self
            .shell(
                serial,
                "getprop ro.serialno; getprop ro.build.version.release; getprop ro.build.version.sdk",
            )
            .ok()?;
        let mut lines = text.lines().map(str::trim);
        Some(About {
            hardware: lines.next()?.to_string(),
            android: lines.next().unwrap_or_default().to_string(),
            api: lines.next().and_then(|api| api.parse().ok()),
        })
    }

    /// Phones with Wireless debugging on that adb sees on the network.
    pub fn nearby(&self) -> Vec<Nearby> {
        self.text(None, &["mdns", "services"])
            .map(|text| parse_nearby(&text))
            .unwrap_or_default()
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

    /// The screen's size in device pixels, as it is used now (an override
    /// wins over the panel's own).
    pub fn screen_size(&self, serial: &str) -> Option<(i32, i32)> {
        parse_screen_size(&self.shell(serial, "wm size").ok()?)
    }

    /// Where the status bar is (left, top, right, bottom in device pixels),
    /// while it shows and the device says (Android 11 and later).
    pub fn status_bar(&self, serial: &str) -> Option<(i32, i32, i32, i32)> {
        let out = self
            .shell(serial, "dumpsys window | grep -m 1 'type=statusBars'")
            .ok()?;
        parse_status_bar(&out)
    }

    /// The package of the app in front, if the device says.
    pub fn front_app(&self, serial: &str) -> Option<String> {
        let out = self
            .shell(
                serial,
                "dumpsys activity activities | grep -E 'ResumedActivity|mFocusedApp' | head -n 3",
            )
            .ok()?;
        parse_front_app(&out)
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
        // `-t`: builds made the way Android Studio makes them are marked
        // test-only, and the package manager refuses those without it.
        let output = self.text(Some(serial), &["install", "-r", "-t", &apk])?;
        if output.contains("Failure") {
            bail!("{}", output.trim());
        }
        Ok(())
    }

    /// `adb connect host:port`: a phone with wireless debugging on.
    pub fn connect(&self, address: &str) -> Result<String> {
        let output = self.text(None, &["connect", address])?;
        // adb exits 0 even when it could not connect.
        if output.contains("cannot") || output.contains("failed") || output.contains("unable") {
            bail!("{}", output.trim());
        }
        Ok(output.trim().to_string())
    }

    /// `adb pair host:port code`: the first time a phone is used over Wi-Fi.
    pub fn pair(&self, address: &str, code: &str) -> Result<String> {
        let output = self.text(None, &["pair", address, code])?;
        if !output.contains("Successfully paired") {
            bail!("{}", output.trim());
        }
        Ok(output.trim().to_string())
    }

    /// Moves a USB phone to Wi-Fi, as Android Studio's ADB Wi-Fi does: adb on
    /// the phone listens on TCP port `port`, then this computer connects to
    /// the phone's address on its Wi-Fi network. Returns the address.
    pub fn switch_to_wifi(&self, serial: &str, port: u16) -> Result<String> {
        let ip = self
            .wifi_ip(serial)
            .context("The phone is not on Wi-Fi (no address on its wireless network)")?;
        self.text(Some(serial), &["tcpip", &port.to_string()])?;
        // adbd restarts on the phone before it listens.
        let address = format!("{ip}:{port}");
        let mut last = None;
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(500));
            match self.connect(&address) {
                Ok(_) => return Ok(address),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("could not connect to {address}")))
    }

    /// The phone's IPv4 address on its Wi-Fi network.
    pub fn wifi_ip(&self, serial: &str) -> Option<String> {
        let inet = self.shell(serial, "ip -f inet addr show wlan0").ok();
        inet.as_deref()
            .and_then(parse_inet)
            .or_else(|| parse_route_src(&self.shell(serial, "ip route").ok()?))
    }

    pub fn disconnect(&self, address: &str) -> Result<()> {
        self.text(None, &["disconnect", address]).map(drop)
    }

    /// The running processes of `package` (none when it is not running).
    pub fn pids_of(&self, serial: &str, package: &str) -> std::collections::HashSet<u32> {
        if !is_package_name(package) {
            return Default::default();
        }
        self.shell(serial, &format!("pidof {package}"))
            .map(|out| {
                out.split_whitespace()
                    .filter_map(|pid| pid.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The processes running on the device, by pid: `com.example.app`,
    /// `com.example.app:sync`, `system_server`…
    pub fn processes(&self, serial: &str) -> Vec<(u32, String)> {
        // Android 8 and later list every process only with `-A`; before,
        // `ps` alone did, and knew no `-A`.
        self.shell(serial, "ps -A -o PID,NAME 2>/dev/null || ps")
            .map(|out| parse_processes(&out))
            .unwrap_or_default()
    }

    pub fn launch(&self, serial: &str, package: &str) -> Result<()> {
        if !is_package_name(package) {
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

    /// Starts `package` from its first screen: stopped first, and what its
    /// task still showed (a screen of another app it had opened, which a
    /// stop of this one leaves there) taken away. Waits until it is up.
    pub fn launch_fresh(&self, serial: &str, package: &str) -> Result<()> {
        if !is_package_name(package) {
            bail!("`{package}` is not a package name");
        }
        let _ = self.shell(serial, &format!("am force-stop {package}"));
        // `com.app/.MainActivity`, the last line of what the device answers.
        let component = self
            .shell(
                serial,
                &format!(
                    "cmd package resolve-activity --brief -c android.intent.category.LAUNCHER {package}"
                ),
            )
            .ok()
            .and_then(|out| {
                out.lines()
                    .map(str::trim)
                    .rfind(|line| line.starts_with(package) && line.contains('/'))
                    .map(str::to_string)
            });
        let Some(component) = component else {
            // An old device that does not say: started as it always was.
            return self.launch(serial, package);
        };
        let output = self.shell(
            serial,
            &format!(
                "am start -W -a android.intent.action.MAIN -c android.intent.category.LAUNCHER --activity-clear-task -n {component}"
            ),
        )?;
        if output.contains("Error") {
            bail!("{package} could not be started: {}", output.trim());
        }
        Ok(())
    }

    /// Removes the app `package` and its data from the device.
    pub fn uninstall(&self, serial: &str, package: &str) -> Result<()> {
        self.on_app(serial, package, &["uninstall", package], "Success")
    }

    /// Erases the app's data, as *Clear storage* in its settings does.
    pub fn clear_data(&self, serial: &str, package: &str) -> Result<()> {
        self.on_app(
            serial,
            package,
            &["shell", "pm", "clear", package],
            "Success",
        )
    }

    pub fn force_stop(&self, serial: &str, package: &str) -> Result<()> {
        self.on_app(serial, package, &["shell", "am", "force-stop", package], "")
    }

    /// Shows the app's page in the device's settings (its permissions,
    /// storage, notifications).
    pub fn app_info(&self, serial: &str, package: &str) -> Result<()> {
        let uri = format!("package:{package}");
        self.on_app(
            serial,
            package,
            &[
                "shell",
                "am",
                "start",
                "-a",
                "android.settings.APPLICATION_DETAILS_SETTINGS",
                "-d",
                &uri,
            ],
            "",
        )
    }

    /// Grants the app every permission it asks for that can be granted
    /// this way (those the user is otherwise asked about); how many.
    pub fn grant_permissions(&self, serial: &str, package: &str) -> Result<usize> {
        if !is_package_name(package) {
            bail!("`{package}` is not a package name");
        }
        // What the app asks for, then each tried: one that is not a
        // runtime permission is refused, which is no failure.
        let granted = self.shell(
            serial,
            &format!(
                "for p in $(dumpsys package {package} | sed -n '/requested permissions:/,/install permissions:/p' | grep -o '[A-Za-z0-9_.]*permission[A-Za-z0-9_.]*'); do pm grant {package} $p 2>/dev/null && echo granted; done"
            ),
        )?;
        Ok(granted
            .lines()
            .filter(|line| line.trim() == "granted")
            .count())
    }

    /// Opens `url` on the device, in the app `package` when one is given:
    /// a deep link into it.
    pub fn open_link(&self, serial: &str, url: &str, package: Option<&str>) -> Result<()> {
        let mut command = format!(
            "am start -a android.intent.action.VIEW -d {}",
            shell_quote(url)
        );
        if let Some(package) = package {
            if !is_package_name(package) {
                bail!("`{package}` is not a package name");
            }
            command.push_str(&format!(" {package}"));
        }
        let said = self.shell(serial, &command)?;
        // `am` exits 0 whatever happened.
        if let Some(error) = said.lines().find(|line| line.starts_with("Error")) {
            bail!("Nothing on the device opens that link ({})", error.trim());
        }
        Ok(())
    }

    /// Runs an adb command about the app `package`; `expect` is what it
    /// prints when it worked (adb exits 0 either way).
    fn on_app(&self, serial: &str, package: &str, args: &[&str], expect: &str) -> Result<()> {
        if !is_package_name(package) {
            bail!("`{package}` is not a package name");
        }
        let said = self.text(Some(serial), args)?;
        if !said.contains(expect) {
            bail!("{}", said.trim());
        }
        Ok(())
    }

    /// Apps the user installed (not system apps), sorted.
    pub fn packages(&self, serial: &str) -> Result<Vec<String>> {
        let listed = |command: &str| -> Result<Vec<String>> {
            Ok(self
                .shell(serial, command)?
                .lines()
                .filter_map(|line| line.trim().strip_prefix("package:").map(str::to_string))
                .collect())
        };
        // Of the user in front. With no user named, a phone that has one
        // the shell may not look into (Samsung's Secure Folder) refuses
        // the whole list.
        let mut packages = listed(
            "pm list packages -3 --user $(am get-current-user 2>/dev/null || echo 0) 2>/dev/null",
        )
        .unwrap_or_default();
        if packages.is_empty() {
            packages = listed("pm list packages -3")?;
        }
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
        let args = ["shell", &format!("ls -la {dir}")];
        let output = self.output(Some(serial), &args)?;
        let failed = (!output.status.success()).then(|| complaint(&args, &output));
        listed(&String::from_utf8_lossy(&output.stdout), failed)
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

/// What adb said when it failed: its stderr, else its stdout.
fn complaint(args: &[&str], output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    format!("adb {}: {}", args.first().unwrap_or(&""), message.trim())
}

/// The entries `ls` printed. It exits with an error when it could not read
/// a single entry (`/metadata: Permission denied` in `/` on a phone that is
/// not rooted) though it listed all the others: those are the folder, and
/// `failed` (what it complained of) is the answer only when it listed none.
fn listed(output: &str, failed: Option<String>) -> Result<Vec<RemoteFile>> {
    let files = parse_ls(output);
    match failed {
        Some(complaint) if files.is_empty() => bail!("{complaint}"),
        _ => Ok(files),
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
                about: None,
            })
        })
        .collect()
}

/// `adb mdns services`: a heading, then a service a line, its name, kind
/// and address: `adb-R5CT1234ABC-x7Yz  _adb-tls-connect._tcp  192.168.1.5:37123`.
fn parse_nearby(text: &str) -> Vec<Nearby> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (name, kind, address) = (fields.next()?, fields.next()?, fields.next()?);
            let pairing = kind.starts_with("_adb-tls-pairing");
            (pairing || kind.starts_with("_adb-tls-connect")).then(|| Nearby {
                name: name.to_string(),
                address: address.to_string(),
                pairing,
            })
        })
        .collect()
}

/// `ps` output: a header naming the columns, then a process a line. The
/// name is the last column, the pid the one under `PID`.
fn parse_processes(output: &str) -> Vec<(u32, String)> {
    let mut lines = output.lines();
    let Some(pid_column) = lines
        .next()
        .and_then(|header| header.split_whitespace().position(|name| name == "PID"))
    else {
        return Vec::new();
    };
    lines
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let pid = fields.get(pid_column)?.parse().ok()?;
            Some((pid, fields.last()?.to_string()))
        })
        .collect()
}

/// The address in `ip addr` output: `inet 192.168.1.23/24 brd …`.
fn parse_inet(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .skip_while(|word| *word != "inet")
        .nth(1)
        .and_then(|cidr| {
            let ip = cidr.split('/').next()?;
            ip.parse::<std::net::Ipv4Addr>()
                .ok()
                .map(|ip| ip.to_string())
        })
}

/// The Wi-Fi source address in `ip route` output: `… dev wlan0 … src 192.168.1.23`.
fn parse_route_src(output: &str) -> Option<String> {
    output
        .lines()
        .filter(|line| line.contains("wlan"))
        .find_map(|line| {
            let mut words = line.split_whitespace().skip_while(|word| *word != "src");
            words.next()?;
            words.next()?.parse::<std::net::Ipv4Addr>().ok()
        })
        .map(|ip| ip.to_string())
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
    rgba.as_chunks_mut::<4>()
        .0
        .iter_mut()
        .for_each(|pixel| pixel[3] = 255);
    Some(Frame::from_rgba(width, height, rgba))
}

/// [`Command::output`], killed after `limit`.
fn output_within(mut command: Command, limit: Duration) -> std::io::Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = read(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = read(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer within {} seconds", limit.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// `input text` takes no spaces (`%s` stands for one) and runs in a shell.
/// `Physical size: 1080x2400` (and `Override size: 720x1600` after it, when
/// set) → the size in use.
fn parse_screen_size(out: &str) -> Option<(i32, i32)> {
    out.lines()
        .filter_map(|line| line.rsplit_once(':'))
        .filter_map(|(_, size)| {
            let (width, height) = size.trim().split_once('x')?;
            Some((width.parse().ok()?, height.parse().ok()?))
        })
        .next_back()
}

/// `InsetsSource id=1cf90000 type=statusBars frame=[0,0][1080,63]
/// visible=true …` → (0, 0, 1080, 63); `None` while it is hidden.
fn parse_status_bar(out: &str) -> Option<(i32, i32, i32, i32)> {
    if !out.contains("visible=true") {
        return None;
    }
    let frame = out.split("frame=").nth(1)?.split_whitespace().next()?;
    let numbers: Vec<i32> = frame
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    match numbers[..] {
        [left, top, right, bottom] if right > left && bottom > top => {
            Some((left, top, right, bottom))
        }
        _ => None,
    }
}

/// `… ResumedActivity: ActivityRecord{1a2b u0 com.app/.MainActivity t12}` →
/// `com.app`.
fn parse_front_app(out: &str) -> Option<String> {
    out.split_whitespace()
        .filter_map(|word| word.split_once('/'))
        .map(|(package, _)| package.trim_start_matches('{'))
        .find(|package| is_package_name(package))
        .map(str::to_string)
}

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
    fn the_screen_size_in_use_and_the_app_in_front_are_read() {
        assert_eq!(
            parse_screen_size("Physical size: 1080x2400\n"),
            Some((1080, 2400))
        );
        assert_eq!(
            parse_screen_size("Physical size: 1080x2400\nOverride size: 720x1600\n"),
            Some((720, 1600))
        );
        assert_eq!(parse_screen_size("error"), None);
        assert_eq!(
            parse_front_app(
                "  mResumedActivity: ActivityRecord{1a2b u0 com.example.app/.MainActivity t12}"
            )
            .as_deref(),
            Some("com.example.app")
        );
        assert_eq!(parse_front_app("nothing here"), None);
        assert_eq!(
            parse_status_bar(
                "  InsetsSource id=1cf90000 type=statusBars frame=[0,0][1080,63] visible=true flags="
            ),
            Some((0, 0, 1080, 63))
        );
        assert_eq!(
            parse_status_bar("InsetsSource type=statusBars frame=[0,0][1080,63] visible=false"),
            None
        );
    }

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
    fn reads_the_phones_adb_sees_on_the_network() {
        let text = "List of discovered mdns services\nadb-R5CT1234ABC-x7Yz\t_adb-tls-connect._tcp\t192.168.1.5:37123\nadb-R5CT1234ABC-x7Yz\t_adb-tls-pairing._tcp\t192.168.1.5:41231\nother\t_adb._tcp\t192.168.1.9:5555\n";
        let found = parse_nearby(text);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].address.as_str(), found[0].pairing),
            ("192.168.1.5:37123", false)
        );
        assert!(found[1].pairing);
        assert_eq!(found[0].hardware(), "R5CT1234ABC");
        assert!(parse_nearby("List of discovered mdns services\n").is_empty());
    }

    #[test]
    fn a_device_is_told_by_its_own_serial_number() {
        let mut phone = DeviceInfo {
            serial: "192.168.1.5:5555".into(),
            state: "device".into(),
            name: "Pixel 7".into(),
            ..Default::default()
        };
        assert_eq!(phone.identity(), "192.168.1.5:5555", "not asked yet");
        assert_eq!(phone.link(), Link::Network);
        phone.about = Some(About {
            hardware: "R5CT1234ABC".into(),
            ..Default::default()
        });
        assert_eq!(phone.identity(), "R5CT1234ABC");
        let emulator = DeviceInfo {
            serial: "emulator-5554".into(),
            emulator: true,
            about: phone.about.clone(),
            ..Default::default()
        };
        assert_eq!(emulator.identity(), "emulator-5554");
        assert_eq!(emulator.link(), Link::Emulator);
    }

    #[test]
    fn reads_the_processes_running() {
        let new = "  PID NAME\n    1 init\n 4321 com.example.app\n 4400 com.example.app:sync\n";
        assert_eq!(
            parse_processes(new),
            [
                (1, "init".to_string()),
                (4321, "com.example.app".to_string()),
                (4400, "com.example.app:sync".to_string())
            ]
        );
        let old = "USER     PID   PPID  VSIZE  RSS   WCHAN    PC  NAME\nroot      1     0     8904   788   ffffffff 00000000 S /init\nu0_a57    4321  218   1 2 ffffffff 00000000 S com.example.app\n";
        assert_eq!(
            parse_processes(old)[1],
            (4321, "com.example.app".to_string())
        );
        assert!(parse_processes("").is_empty());
    }

    #[test]
    fn tells_wireless_devices() {
        assert!(is_wireless("192.168.1.5:5555"));
        assert!(is_wireless("adb-R5CT1234._adb-tls-connect._tcp"));
        assert!(!is_wireless("emulator-5554"));
        assert!(!is_wireless("R5CT1234ABC"));
    }

    #[test]
    fn reads_the_phones_wifi_address() {
        let addr = "30: wlan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500\n    inet 192.168.1.23/24 brd 192.168.1.255 scope global wlan0\n";
        assert_eq!(parse_inet(addr).as_deref(), Some("192.168.1.23"));
        assert_eq!(parse_inet("Device \"wlan0\" does not exist."), None);
        let route = "192.168.1.0/24 dev wlan0 proto kernel scope link src 192.168.1.23\n";
        assert_eq!(parse_route_src(route).as_deref(), Some("192.168.1.23"));
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
    fn a_folder_with_unreadable_entries_is_still_listed() {
        // `ls -la /` on a phone that is not rooted: exit status 1.
        let root = "ls: /metadata: Permission denied\ntotal 24\ndrwxr-xr-x 2 root root 4096 2026-10-24 10:00 acct\nls: /init: Permission denied\ndrwxrwx--x 2 root root 4096 2026-10-24 10:00 data\n";
        let names: Vec<String> =
            listed(root, Some("adb shell: ls: /init: Permission denied".into()))
                .unwrap()
                .into_iter()
                .map(|file| file.name)
                .collect();
        assert_eq!(names, ["acct", "data"]);
        // Nothing listed: the complaint is the answer.
        let denied = listed(
            "ls: /data/: Permission denied\n",
            Some("adb shell: ls: /data/: Permission denied".into()),
        );
        assert_eq!(
            denied.unwrap_err().to_string(),
            "adb shell: ls: /data/: Permission denied"
        );
        // An empty folder is not a failure.
        assert_eq!(listed("total 0\n", None).unwrap(), Vec::new());
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
