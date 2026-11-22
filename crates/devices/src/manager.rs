//! Which devices exist: connected phones and emulators, and AVDs to start.
//!
//! One manager for the app (devices are not per project). It follows device
//! changes as the adb server reports them, so the list is current without
//! polling.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};

use crate::adb::{About, Adb, DeviceInfo, Link};
use crate::sdk::Sdk;

/// How long an emulator may take to boot.
const BOOT_TIMEOUT: Duration = Duration::from_secs(240);
/// Settings: the devices connected over the network before, and the
/// device chosen last (by what tells it apart, see [`DeviceInfo::identity`]).
const SAVED: &str = "devices.known";
const CHOSEN: &str = "devices.selected";
/// Devices kept at most: the ones used longest ago go first.
const SAVED_MAX: usize = 30;

pub struct DevicesChanged;

/// A device connected over the network before, kept so that it connects
/// again in one click: an office's test devices on fixed addresses are not
/// typed in each morning.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedDevice {
    /// Where it was reached last: `192.168.1.20:5555`.
    pub address: String,
    /// The device's model.
    pub name: String,
    /// The name the user gave it, which tells devices of one model apart.
    #[serde(default)]
    pub label: Option<String>,
    /// The device's own serial number: at another address it is still
    /// this entry.
    #[serde(default)]
    pub hardware: String,
    /// When it was last connected, in seconds since 1970.
    #[serde(default)]
    pub last_used: u64,
}

impl SavedDevice {
    /// The name to show: the user's, else the model.
    pub fn title(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// Where the manager keeps its memory, given by the app at start.
struct Memory(ph_store::Store);

impl Global for Memory {}

pub(crate) fn remember_in(store: ph_store::Store, cx: &mut App) {
    cx.set_global(Memory(store));
}

/// Adds `device`, connected now at `address`, to `saved`, or brings its
/// entry up to date: the same device (by its own serial number, else by
/// its address) keeps its entry and the name the user gave it.
fn save_device(saved: &mut Vec<SavedDevice>, address: &str, device: &DeviceInfo, now: u64) {
    let hardware = device
        .about
        .as_ref()
        .map(|about| about.hardware.clone())
        .unwrap_or_default();
    let known = saved.iter().position(|entry| {
        (!hardware.is_empty() && entry.hardware == hardware) || entry.address == address
    });
    let label = known.and_then(|at| saved.remove(at).label);
    // Another entry at this address is of a device that has left it.
    saved.retain(|entry| entry.address != address);
    saved.insert(
        0,
        SavedDevice {
            address: address.to_string(),
            name: device.display_name(),
            label,
            hardware,
            last_used: now,
        },
    );
    saved.truncate(SAVED_MAX);
}

/// Whether `serial` is an address one can connect to again (`host:port`),
/// not the name of a device adb found by itself on the network.
fn is_address(serial: &str) -> bool {
    !serial.contains("._adb-tls-connect.")
        && serial
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

pub struct DeviceManager {
    sdk: Sdk,
    adb: Option<Adb>,
    /// The SDK is still being looked for (see [`Self::global`]): until it
    /// is found there is no adb, as when there is none.
    locating: bool,
    /// Closed once the SDK has been looked for.
    located: smol::channel::Receiver<()>,
    /// Devices were asked for while the SDK was looked for: they are
    /// followed once adb is found.
    track_when_located: bool,
    devices: Vec<DeviceInfo>,
    avds: Vec<String>,
    /// The device shown in the pane; agents default to it too.
    selected: Option<String>,
    /// The device the user chose last, by what tells it apart: it is the
    /// one chosen again when it comes back, also after a restart.
    chosen: Option<String>,
    /// A serial the user chose that the list does not show yet (it is
    /// connecting): once it does, it is the chosen device.
    choosing: Option<String>,
    /// What each connected device says of itself, by serial.
    about: std::collections::HashMap<String, About>,
    /// Devices connected over the network before, the latest first.
    saved: Vec<SavedDevice>,
    /// AVDs being started.
    booting: Vec<String>,
    tracking: bool,
    _track: Option<Task<()>>,
    _locate: Option<Task<()>>,
}

impl EventEmitter<DevicesChanged> for DeviceManager {}

struct GlobalManager(Entity<DeviceManager>);

impl Global for GlobalManager {}

impl DeviceManager {
    /// The app's manager, created on first use (SDK lookup is cheap; adb
    /// is not started until something asks for devices).
    /// The manager, if something already uses devices (it is made on first
    /// use: finding the SDK touches the disk).
    pub fn existing(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalManager>()
            .map(|manager| manager.0.clone())
    }

    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(manager) = cx.try_global::<GlobalManager>() {
            return manager.0.clone();
        }
        let manager = cx.new(|cx: &mut Context<Self>| {
            // Finding the SDK reads the login shell's environment, which
            // can take seconds to come just after the app starts, and looks
            // at the disk. The first window may be what makes this manager
            // (Devices or Logcat was open last), so it is done on a
            // background thread and the manager starts without an SDK.
            let (done, located) = smol::channel::bounded::<()>(1);
            let locate = cx.spawn(async move |this, cx| {
                let sdk = cx.background_spawn(async { Sdk::locate() }).await;
                let _ = this.update(cx, |manager, cx| {
                    manager.adb = sdk.adb.clone().map(Adb::new);
                    manager.sdk = sdk;
                    manager.locating = false;
                    if std::mem::take(&mut manager.track_when_located) {
                        manager.ensure_tracking(cx);
                    }
                    cx.emit(DevicesChanged);
                    cx.notify();
                });
                // Closes the channel: whoever waits in `located` goes on.
                drop(done);
            });
            Self {
                sdk: Sdk::default(),
                adb: None,
                locating: true,
                located,
                track_when_located: false,
                devices: Vec::new(),
                avds: Vec::new(),
                selected: None,
                chosen: cx
                    .try_global::<Memory>()
                    .map(|memory| memory.0.setting::<String>(CHOSEN))
                    .filter(|chosen| !chosen.is_empty()),
                choosing: None,
                about: Default::default(),
                saved: cx
                    .try_global::<Memory>()
                    .map(|memory| memory.0.setting(SAVED))
                    .unwrap_or_default(),
                booting: Vec::new(),
                tracking: false,
                _track: None,
                _locate: Some(locate),
            }
        });
        cx.set_global(GlobalManager(manager.clone()));
        manager
    }

    /// Whether the SDK is still being looked for: no adb yet does not mean
    /// there is none.
    pub fn looking_for_sdk(&self) -> bool {
        self.locating
    }

    /// Waits until the SDK has been looked for (no time at all once it
    /// has): for callers that answer "not found" otherwise.
    pub(crate) fn located(&self) -> impl Future<Output = ()> + use<> {
        let located = self.located.clone();
        async move {
            let _ = located.recv().await;
        }
    }

    pub fn adb(&self) -> Option<&Adb> {
        self.adb.as_ref()
    }

    pub fn emulator_tool(&self) -> Option<std::path::PathBuf> {
        self.sdk.emulator.clone()
    }

    pub fn devices(&self) -> &[DeviceInfo] {
        &self.devices
    }

    pub fn avds(&self) -> &[String] {
        &self.avds
    }

    /// System images an emulator can be made from.
    pub fn system_images(&self) -> Vec<crate::avd::SystemImage> {
        self.sdk
            .root
            .as_deref()
            .map(crate::avd::system_images)
            .unwrap_or_default()
    }

    /// Running or starting: its files are in use.
    pub fn is_running(&self, avd: &str) -> bool {
        self.booting.iter().any(|booting| booting == avd)
            || self
                .devices
                .iter()
                .any(|device| device.emulator && device.name == avd)
    }

    /// Reads the AVD list again (after one was made or deleted).
    pub fn refresh_avds(&mut self, cx: &mut Context<Self>) {
        let emulator = self.sdk.emulator.clone();
        cx.spawn(async move |this, cx| {
            let avds = cx
                .background_spawn(async move { list_avds(emulator.as_deref()) })
                .await;
            let _ = this.update(cx, |manager, cx| {
                manager.avds = avds;
                cx.emit(DevicesChanged);
                cx.notify();
            });
        })
        .detach();
    }

    /// Makes an emulator (see [`crate::avd::create`]); returns its name.
    pub fn create_avd(
        &mut self,
        display: &str,
        image: &crate::avd::SystemImage,
        profile: crate::avd::Profile,
        cx: &mut Context<Self>,
    ) -> Result<String> {
        let home = crate::avd::avd_home().context("could not find the AVD folder")?;
        let name = crate::avd::create(&home, display, image, profile)?;
        // Shown at once; the emulator's own list catches up.
        self.avds.push(name.clone());
        self.refresh_avds(cx);
        Ok(name)
    }

    /// Erases an emulator's data (gigabytes of disk images: off the UI
    /// thread); it must be stopped. Returns bytes freed.
    pub fn wipe_avd(&mut self, avd: &str, cx: &mut Context<Self>) -> Task<Result<u64>> {
        if self.is_running(avd) {
            let message = format!("Stop {} before wiping it.", avd.replace('_', " "));
            return Task::ready(Err(anyhow::anyhow!(message)));
        }
        let avd = avd.to_string();
        cx.background_spawn(async move {
            let home = crate::avd::avd_home().context("could not find the AVD folder")?;
            crate::avd::wipe(&home, &avd)
        })
    }

    /// Removes an emulator and its disks (off the UI thread); it must be
    /// stopped.
    pub fn delete_avd(&mut self, avd: &str, cx: &mut Context<Self>) -> Task<Result<()>> {
        if self.is_running(avd) {
            let message = format!("Stop {} before deleting it.", avd.replace('_', " "));
            return Task::ready(Err(anyhow::anyhow!(message)));
        }
        let avd = avd.to_string();
        let delete = {
            let avd = avd.clone();
            cx.background_spawn(async move {
                let home = crate::avd::avd_home().context("could not find the AVD folder")?;
                crate::avd::delete(&home, &avd)
            })
        };
        cx.spawn(async move |this, cx| {
            delete.await?;
            _ = this.update(cx, |this, cx| {
                this.avds.retain(|known| *known != avd);
                this.refresh_avds(cx);
            });
            Ok(())
        })
    }

    pub fn booting(&self) -> &[String] {
        &self.booting
    }

    pub fn selected(&self) -> Option<&DeviceInfo> {
        let serial = self.selected.as_deref()?;
        self.devices.iter().find(|device| device.serial == serial)
    }

    /// The user chose the device `serial`.
    pub fn select(&mut self, serial: &str, cx: &mut Context<Self>) {
        self.selected = Some(serial.to_string());
        self.choosing = Some(serial.to_string());
        self.remember_choice(cx);
        cx.emit(DevicesChanged);
        cx.notify();
    }

    /// Keeps the device the user chose, once the list shows it (what tells
    /// it apart is asked of the device).
    fn remember_choice(&mut self, cx: &App) {
        let Some(device) = self
            .choosing
            .as_deref()
            .and_then(|serial| self.devices.iter().find(|device| device.serial == serial))
            .filter(|device| device.emulator || device.about.is_some())
        else {
            return;
        };
        let identity = device.identity().to_string();
        self.choosing = None;
        if self.chosen.as_deref() != Some(identity.as_str()) {
            if let Some(memory) = cx.try_global::<Memory>() {
                let _ = memory.0.set_setting(CHOSEN, &identity);
            }
            self.chosen = Some(identity);
        }
    }

    /// Devices connected over the network before, the latest first.
    pub fn saved(&self) -> &[SavedDevice] {
        &self.saved
    }

    /// Whether the saved device is connected now.
    pub fn is_connected(&self, saved: &SavedDevice) -> bool {
        self.devices.iter().any(|device| {
            device.is_ready()
                && (device.serial == saved.address
                    || (!saved.hardware.is_empty()
                        && device.link() == Link::Network
                        && device.identity() == saved.hardware))
        })
    }

    fn store_saved(&self, cx: &App) {
        if let Some(memory) = cx.try_global::<Memory>() {
            let _ = memory.0.set_setting(SAVED, &self.saved);
        }
    }

    /// Forgets a saved device.
    pub fn forget(&mut self, address: &str, cx: &mut Context<Self>) {
        self.saved.retain(|saved| saved.address != address);
        self.store_saved(cx);
        cx.notify();
    }

    /// Names a saved device (no name: its model again).
    pub fn rename(&mut self, address: &str, label: &str, cx: &mut Context<Self>) {
        if let Some(saved) = self.saved.iter_mut().find(|saved| saved.address == address) {
            let label = label.trim();
            saved.label = (!label.is_empty() && label != saved.name).then(|| label.to_string());
            self.store_saved(cx);
            cx.notify();
        }
    }

    /// Connects to the device at `address` (`adb connect`) and chooses it;
    /// the serial it has, or why it could not be reached.
    pub fn connect(&mut self, address: &str, cx: &mut Context<Self>) -> Task<Result<String>> {
        let Some(adb) = self.adb.clone() else {
            return Task::ready(Err(anyhow!(Sdk::missing_message())));
        };
        self.ensure_tracking(cx);
        let address = address.to_string();
        cx.spawn(async move |this, cx| {
            let target = address.clone();
            let said = cx
                .background_spawn(async move { adb.connect(&target) })
                .await?;
            // "connected to 192.168.1.20:5555": the serial it has now.
            let serial = said
                .strip_prefix("already connected to ")
                .or_else(|| said.strip_prefix("connected to "))
                .map_or(address, |serial| serial.trim().to_string());
            this.update(cx, |manager, cx| manager.select(&serial, cx))?;
            Ok(serial)
        })
    }

    /// The device list changed: `devices` is what adb lists now and
    /// `learned` what newly connected ones said of themselves.
    fn devices_listed(
        &mut self,
        devices: Vec<DeviceInfo>,
        learned: Vec<(String, About)>,
        cx: &mut Context<Self>,
    ) {
        let was_ready: std::collections::HashSet<String> = self
            .devices
            .iter()
            .filter(|device| device.is_ready())
            .map(|device| device.serial.clone())
            .collect();
        self.about.extend(learned);
        // A serial that went away may come back as another device (an
        // address handed to the next phone): it is asked again then.
        self.about.retain(|serial, _| {
            devices
                .iter()
                .any(|device| device.serial == *serial && device.is_ready())
        });
        self.devices = devices;
        for device in &mut self.devices {
            device.about = self.about.get(&device.serial).cloned();
        }
        // Devices that came over the network are kept, to connect again.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let mut saved = self.saved.clone();
        for device in &self.devices {
            let known = saved.iter().any(|entry| entry.address == device.serial);
            if device.is_ready()
                && device.about.is_some()
                && is_address(&device.serial)
                && device.link() == Link::Network
                && (!was_ready.contains(&device.serial) || !known)
            {
                save_device(&mut saved, &device.serial, device, now);
            }
        }
        if saved != self.saved {
            self.saved = saved;
            self.store_saved(cx);
        }
        let ready = |serial: &str| {
            self.devices
                .iter()
                .any(|device| device.serial == serial && device.is_ready())
        };
        if !self.selected.as_deref().is_some_and(ready) {
            // The device chosen last when it is here, else the first.
            let chosen = self.chosen.as_deref();
            self.selected = self
                .devices
                .iter()
                .filter(|device| device.is_ready())
                .min_by_key(|device| Some(device.identity()) != chosen)
                .map(|device| device.serial.clone());
        }
        self.remember_choice(cx);
        cx.emit(DevicesChanged);
        cx.notify();
    }

    /// Starts following devices (idempotent). Call when the pane opens or an
    /// agent asks for devices.
    pub fn ensure_tracking(&mut self, cx: &mut Context<Self>) {
        if self.tracking {
            return;
        }
        let Some(adb) = self.adb.clone() else {
            // Not found yet: followed as soon as it is.
            self.track_when_located = self.locating;
            return;
        };
        self.tracking = true;
        let emulator = self.sdk.emulator.clone();
        let (changes, receiver) = smol::channel::unbounded::<()>();
        let track_adb = adb.clone();
        std::thread::Builder::new()
            .name("adb-track-devices".into())
            .spawn(move || {
                // The server can restart (`adb kill-server`, an SDK update):
                // follow it again while anyone listens, less often while it
                // cannot be reached (a broken adb is not started every few
                // seconds for good).
                let mut wait = Duration::from_secs(5);
                while !changes.is_closed() {
                    wait = match track_devices(&track_adb, &changes) {
                        true => Duration::from_secs(5),
                        false => (wait * 2).min(Duration::from_secs(5 * 60)),
                    };
                    std::thread::sleep(wait);
                }
            })
            .ok();
        self._track = Some(cx.spawn(async move |this, cx| {
            let list_emulator = emulator.clone();
            let avds = cx
                .background_spawn(async move { list_avds(list_emulator.as_deref()) })
                .await;
            let _ = this.update(cx, |manager, cx| {
                manager.avds = avds;
                cx.notify();
            });
            while receiver.recv().await.is_ok() {
                let devices_adb = adb.clone();
                let Ok(asked) = this.read_with(cx, |manager, _| {
                    manager
                        .about
                        .keys()
                        .cloned()
                        .collect::<std::collections::HashSet<String>>()
                }) else {
                    return;
                };
                let (devices, learned) = cx
                    .background_spawn(async move {
                        let devices = devices_adb.devices().unwrap_or_default();
                        // Each device is asked about itself once.
                        let learned: Vec<(String, About)> = devices
                            .iter()
                            .filter(|device| device.is_ready() && !asked.contains(&device.serial))
                            .filter_map(|device| {
                                Some((device.serial.clone(), devices_adb.about(&device.serial)?))
                            })
                            .collect();
                        (devices, learned)
                    })
                    .await;
                let updated = this.update(cx, |manager, cx| {
                    manager.devices_listed(devices, learned, cx)
                });
                if updated.is_err() {
                    return;
                }
            }
            // adb stopped (killed, or the server restarted): forget the list
            // and follow again on the next use.
            let _ = this.update(cx, |manager, cx| {
                manager.tracking = false;
                manager.devices.clear();
                cx.emit(DevicesChanged);
                cx.notify();
            });
        }));
    }

    /// The serial to act on: the one given, else the pane's device, else the
    /// only ready device.
    pub fn resolve(&self, serial: Option<&str>) -> Result<String> {
        if let Some(serial) = serial {
            return Ok(serial.to_string());
        }
        if let Some(device) = self.selected().filter(|device| device.is_ready()) {
            return Ok(device.serial.clone());
        }
        Self::resolve_from(&self.devices)
    }

    /// The only ready device in `devices`, or an error saying what to do.
    pub fn resolve_from(devices: &[DeviceInfo]) -> Result<String> {
        let ready: Vec<&DeviceInfo> = devices.iter().filter(|device| device.is_ready()).collect();
        match ready[..] {
            [device] => Ok(device.serial.clone()),
            [] => bail!(
                "No Android device is connected. Start an emulator with device_start_emulator, or connect a phone with USB debugging on."
            ),
            _ => bail!("Several devices are connected; pass `serial` (see device_list)."),
        }
    }

    /// Starts an AVD headless (its screen shows in the pane) and waits until
    /// Android has booted. Returns the serial.
    pub fn start_emulator(&mut self, avd: &str, cx: &mut Context<Self>) -> Task<Result<String>> {
        let (Some(emulator), Some(adb)) = (self.sdk.emulator.clone(), self.adb.clone()) else {
            return Task::ready(Err(anyhow!(
                "The Android emulator was not found in your SDK."
            )));
        };
        if !self.avds.iter().any(|known| known == avd) && !self.avds.is_empty() {
            return Task::ready(Err(anyhow!(
                "There is no emulator named `{avd}`. Available: {}",
                self.avds.join(", ")
            )));
        }
        if let Some(device) = self
            .devices
            .iter()
            .find(|device| device.emulator && device.name == avd && device.is_ready())
        {
            return Task::ready(Ok(device.serial.clone()));
        }
        self.ensure_tracking(cx);
        // Asked for twice: a second emulator would only refuse to run
        // beside the first, so this one waits for that.
        let starting = self.booting.iter().any(|booting| booting == avd);
        self.booting.push(avd.to_string());
        cx.notify();
        let name = avd.to_string();
        // The list is not read yet (an agent's first call of the session).
        let unchecked = self.avds.is_empty();
        cx.spawn(async move |this, cx| {
            let boot_name = name.clone();
            let result = cx
                .background_spawn(async move {
                    if starting {
                        return wait_for_boot(&adb, &boot_name, None, None);
                    }
                    // So it is read here: a name that does not exist is
                    // refused now, not waited on until the boot times out.
                    if unchecked {
                        let avds = list_avds(Some(&emulator));
                        if !avds.is_empty() && !avds.contains(&boot_name) {
                            bail!(
                                "There is no emulator named `{boot_name}`. Available: {}",
                                avds.join(", ")
                            );
                        }
                    }
                    let complaints = complaints_file();
                    let (kept, stderr) = match complaints {
                        Some((path, file)) => (Some(path), Stdio::from(file)),
                        None => (None, Stdio::null()),
                    };
                    let child = ph_env::command(&emulator)
                        .args([
                            "-avd",
                            &boot_name,
                            "-no-window",
                            "-no-audio",
                            "-no-boot-anim",
                        ])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(stderr)
                        .spawn()
                        .context("starting the emulator");
                    let booted = child.and_then(|mut child| {
                        let booted =
                            wait_for_boot(&adb, &boot_name, Some(&mut child), kept.as_deref());
                        // Reaped when it exits, so no zombie is left behind.
                        std::thread::Builder::new()
                            .name("emulator-wait".into())
                            .spawn(move || {
                                let _ = child.wait();
                            })
                            .ok();
                        booted
                    });
                    // Read by now, if it was needed; a running emulator
                    // keeps writing to it, unseen, until it ends.
                    if let Some(path) = kept {
                        let _ = std::fs::remove_file(path);
                    }
                    booted
                })
                .await;
            let _ = this.update(cx, |manager, cx| {
                manager.booting.retain(|booting| *booting != name);
                if let Ok(serial) = &result {
                    manager.selected = Some(serial.clone());
                }
                cx.emit(DevicesChanged);
                cx.notify();
            });
            result
        })
    }

    /// Forgets a Wi-Fi device (`adb disconnect`).
    pub fn disconnect(&self, serial: &str, cx: &App) {
        let Some(adb) = self.adb.clone() else { return };
        let serial = serial.to_string();
        cx.background_spawn(async move {
            if let Err(err) = adb.disconnect(&serial) {
                tracing::warn!("disconnecting {serial} failed: {err:#}");
            }
        })
        .detach();
    }

    pub fn stop_emulator(&self, serial: &str, cx: &App) {
        let Some(adb) = self.adb.clone() else { return };
        let serial = serial.to_string();
        cx.background_spawn(async move {
            if let Err(err) = adb.kill_emulator(&serial) {
                tracing::warn!("stopping {serial} failed: {err:#}");
            }
        })
        .detach();
    }
}

pub(crate) fn list_avds(emulator: Option<&std::path::Path>) -> Vec<String> {
    let Some(emulator) = emulator else {
        return Vec::new();
    };
    let Ok(output) = ph_env::command(emulator)
        .arg("-list-avds")
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        // The tool prints warnings on the same stream.
        .filter(|line| {
            !line.is_empty()
                && !line.contains(' ')
                && !line.starts_with("INFO")
                && !line.starts_with("WARNING")
        })
        .map(str::to_string)
        .collect()
}

/// A new file for what the emulator complains of while it starts, which
/// only the user can read; `None` when it cannot be made that way (the
/// complaints are then not kept).
fn complaints_file() -> Option<(PathBuf, std::fs::File)> {
    static STARTS: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "emulator-start-{}-{}.log",
        std::process::id(),
        STARTS.fetch_add(1, Ordering::Relaxed)
    ));
    // Never an existing file: in a temp folder everyone shares, one of that
    // name may be a link someone else put there.
    let _ = std::fs::remove_file(&path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(&path).ok()?;
    Some((path, file))
}

/// What to say when the emulator stopped before Android started: the last
/// lines it wrote, which say why (no such AVD, no hardware acceleration, a
/// broken system image).
fn gave_up(avd: &str, complaints: &str) -> String {
    let lines: Vec<String> = complaints
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect())
        .collect();
    let last = &lines[lines.len().saturating_sub(6)..];
    if last.is_empty() {
        return format!(
            "The emulator stopped before {avd} started. Run `emulator -avd {avd}` in a terminal to see why."
        );
    }
    format!(
        "The emulator stopped before {avd} started:\n{}",
        last.join("\n")
    )
}

/// Waits for an emulator running `avd` to finish booting; returns its
/// serial. `emulator` is its process when this call started it, and
/// `complaints` the file its errors go to.
fn wait_for_boot(
    adb: &Adb,
    avd: &str,
    mut emulator: Option<&mut Child>,
    complaints: Option<&Path>,
) -> Result<String> {
    let started = Instant::now();
    while started.elapsed() < BOOT_TIMEOUT {
        if let Ok(devices) = adb.devices()
            && let Some(device) = devices
                .iter()
                .find(|device| device.emulator && device.name == avd && device.is_ready())
            && adb.boot_completed(&device.serial)
        {
            return Ok(device.serial.clone());
        }
        // It gave up: said now, with its reason, not when the wait is over.
        // One that ended without an error is still waited for (the program
        // started may be a launcher that leaves the emulator running).
        if let Some(emulator) = emulator.as_deref_mut()
            && let Ok(Some(status)) = emulator.try_wait()
            && !status.success()
        {
            let said = complaints
                .and_then(|path| std::fs::read(path).ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            bail!("{}", gave_up(avd, &said));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    bail!(
        "{avd} did not finish booting within {} seconds",
        BOOT_TIMEOUT.as_secs()
    )
}

/// Follows device changes on the adb server's own socket, the way
/// `adb track-devices` does, and sends one message per change. A socket
/// rather than that command: a helper process would outlive the app when
/// it quits or is killed, and nothing reaps it until a device changes.
/// Follows the adb server's device list until it stops; false when it could
/// not be reached at all.
fn track_devices(adb: &Adb, changes: &smol::channel::Sender<()>) -> bool {
    // The server may not be running yet; this starts it (as any adb call
    // does) and returns once it listens.
    let _ = adb
        .command(None)
        .arg("start-server")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let port = std::env::var("ANDROID_ADB_SERVER_PORT")
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(5037);
    let Ok(mut server) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        tracing::warn!("cannot reach the adb server on port {port}");
        return false;
    };
    // Requests are a 4-digit hex length and the request; the server answers
    // OKAY, then a length-prefixed device list on every change.
    let request = "host:track-devices";
    let mut status = [0u8; 4];
    if server
        .write_all(format!("{:04x}{request}", request.len()).as_bytes())
        .is_err()
        || server.read_exact(&mut status).is_err()
        || &status != b"OKAY"
    {
        tracing::warn!("the adb server refused to report devices");
        return false;
    }
    let _ = changes.send_blocking(());
    loop {
        let mut length = [0u8; 4];
        if server.read_exact(&mut length).is_err() {
            break;
        }
        let length =
            usize::from_str_radix(std::str::from_utf8(&length).unwrap_or("0"), 16).unwrap_or(0);
        let mut payload = vec![0; length];
        if server.read_exact(&mut payload).is_err() || changes.send_blocking(()).is_err() {
            break;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(serial: &str, hardware: &str) -> DeviceInfo {
        DeviceInfo {
            serial: serial.into(),
            state: "device".into(),
            name: "Pixel_7".into(),
            emulator: false,
            about: Some(About {
                hardware: hardware.into(),
                android: "14".into(),
                api: Some(34),
            }),
        }
    }

    #[test]
    fn a_device_keeps_one_entry_and_its_name_at_a_new_address() {
        let mut saved = Vec::new();
        save_device(
            &mut saved,
            "192.168.1.5:5555",
            &phone("192.168.1.5:5555", "AAA"),
            10,
        );
        save_device(
            &mut saved,
            "192.168.1.9:5555",
            &phone("192.168.1.9:5555", "BBB"),
            20,
        );
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].hardware, "BBB", "the latest first");
        assert_eq!(saved[0].title(), "Pixel 7");
        saved[1].label = Some("Kiosk 3".into());
        // The first device again, at another address: its entry, its name.
        save_device(
            &mut saved,
            "192.168.1.7:5555",
            &phone("192.168.1.7:5555", "AAA"),
            30,
        );
        assert_eq!(saved.len(), 2);
        assert_eq!(
            (
                saved[0].address.as_str(),
                saved[0].title(),
                saved[0].last_used
            ),
            ("192.168.1.7:5555", "Kiosk 3", 30)
        );
        // Another device at an address one had: that one's entry goes.
        save_device(
            &mut saved,
            "192.168.1.9:5555",
            &phone("192.168.1.9:5555", "CCC"),
            40,
        );
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().all(|entry| entry.hardware != "BBB"));
    }

    #[test]
    fn only_addresses_are_kept_to_connect_again() {
        assert!(is_address("192.168.1.20:5555"));
        assert!(is_address("[fe80::1]:5555"));
        assert!(!is_address("adb-R5CT1234-x7Yz._adb-tls-connect._tcp"));
        assert!(!is_address("R5CT1234ABC"));
        assert!(!is_address("emulator-5554x"));
    }

    #[test]
    fn an_emulator_that_gave_up_says_why() {
        let said = "INFO    | Android emulator version 35.1\n\nPANIC: Unknown AVD name [Pixel], use -list-avds to see valid list.\nHOME is defined but there is no file Pixel.ini in $HOME/.android/avd\n";
        assert_eq!(
            gave_up("Pixel", said),
            "The emulator stopped before Pixel started:\nINFO    | Android emulator version 35.1\nPANIC: Unknown AVD name [Pixel], use -list-avds to see valid list.\nHOME is defined but there is no file Pixel.ini in $HOME/.android/avd"
        );
        // Only the end of a long complaint, and a long line cut short.
        let long: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let message = gave_up("Pixel", &long);
        assert!(message.ends_with("line 15\nline 16\nline 17\nline 18\nline 19\nline 20"));
        assert!(!message.contains("line 14"));
        assert!(gave_up("Pixel", &"é".repeat(1000)).chars().count() < 400);
        // Nothing was written: how to find out.
        assert!(gave_up("Pixel", "\n").contains("emulator -avd Pixel"));
    }
}
