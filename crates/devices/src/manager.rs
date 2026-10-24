//! Which devices exist: connected phones and emulators, and AVDs to start.
//!
//! One manager for the app (devices are not per project). It follows
//! `adb track-devices`, which reports changes as they happen, so the list is
//! current without polling.

use std::io::Read as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};

use crate::adb::{Adb, DeviceInfo};
use crate::sdk::Sdk;

/// How long an emulator may take to boot.
const BOOT_TIMEOUT: Duration = Duration::from_secs(240);

pub struct DevicesChanged;

pub struct DeviceManager {
    sdk: Sdk,
    adb: Option<Adb>,
    devices: Vec<DeviceInfo>,
    avds: Vec<String>,
    /// The device shown in the pane; agents default to it too.
    selected: Option<String>,
    /// AVDs being started.
    booting: Vec<String>,
    tracking: bool,
    _track: Option<Task<()>>,
}

impl EventEmitter<DevicesChanged> for DeviceManager {}

struct GlobalManager(Entity<DeviceManager>);

impl Global for GlobalManager {}

impl DeviceManager {
    /// The app's manager, created on first use (SDK lookup is cheap; adb
    /// is not started until something asks for devices).
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(manager) = cx.try_global::<GlobalManager>() {
            return manager.0.clone();
        }
        let manager = cx.new(|_| {
            let sdk = Sdk::locate();
            Self {
                adb: sdk.adb.clone().map(Adb::new),
                sdk,
                devices: Vec::new(),
                avds: Vec::new(),
                selected: None,
                booting: Vec::new(),
                tracking: false,
                _track: None,
            }
        });
        cx.set_global(GlobalManager(manager.clone()));
        manager
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

    /// Erases an emulator's data; it must be stopped. Returns bytes freed.
    pub fn wipe_avd(&mut self, avd: &str) -> Result<u64> {
        if self.is_running(avd) {
            bail!("Stop {} before wiping it.", avd.replace('_', " "));
        }
        let home = crate::avd::avd_home().context("could not find the AVD folder")?;
        crate::avd::wipe(&home, avd)
    }

    /// Removes an emulator and its disks; it must be stopped.
    pub fn delete_avd(&mut self, avd: &str, cx: &mut Context<Self>) -> Result<()> {
        if self.is_running(avd) {
            bail!("Stop {} before deleting it.", avd.replace('_', " "));
        }
        let home = crate::avd::avd_home().context("could not find the AVD folder")?;
        crate::avd::delete(&home, avd)?;
        self.avds.retain(|known| known != avd);
        self.refresh_avds(cx);
        Ok(())
    }

    pub fn booting(&self) -> &[String] {
        &self.booting
    }

    pub fn selected(&self) -> Option<&DeviceInfo> {
        let serial = self.selected.as_deref()?;
        self.devices.iter().find(|device| device.serial == serial)
    }

    pub fn select(&mut self, serial: &str, cx: &mut Context<Self>) {
        self.selected = Some(serial.to_string());
        cx.emit(DevicesChanged);
        cx.notify();
    }

    /// Starts following devices (idempotent). Call when the pane opens or an
    /// agent asks for devices.
    pub fn ensure_tracking(&mut self, cx: &mut Context<Self>) {
        if self.tracking {
            return;
        }
        let Some(adb) = self.adb.clone() else { return };
        self.tracking = true;
        let emulator = self.sdk.emulator.clone();
        let (changes, receiver) = smol::channel::unbounded::<()>();
        // `track-devices` prints a length-prefixed list on every change.
        let track_adb = adb.clone();
        std::thread::Builder::new()
            .name("adb-track-devices".into())
            .spawn(move || {
                let Ok(mut child) = track_adb
                    .command(None)
                    .arg("track-devices")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                else {
                    return;
                };
                let mut stdout = child.stdout.take().expect("piped stdout");
                let _ = changes.send_blocking(());
                loop {
                    let mut length = [0u8; 4];
                    if stdout.read_exact(&mut length).is_err() {
                        break;
                    }
                    let length =
                        usize::from_str_radix(std::str::from_utf8(&length).unwrap_or("0"), 16)
                            .unwrap_or(0);
                    let mut payload = vec![0; length];
                    if stdout.read_exact(&mut payload).is_err()
                        || changes.send_blocking(()).is_err()
                    {
                        break;
                    }
                }
                let _ = child.kill();
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
                let devices = cx
                    .background_spawn(async move { devices_adb.devices() })
                    .await;
                let updated = this.update(cx, |manager, cx| {
                    manager.devices = devices.unwrap_or_default();
                    let ready = |serial: &str| {
                        manager
                            .devices
                            .iter()
                            .any(|d| d.serial == serial && d.is_ready())
                    };
                    if !manager.selected.as_deref().is_some_and(ready) {
                        manager.selected = manager
                            .devices
                            .iter()
                            .find(|device| device.is_ready())
                            .map(|device| device.serial.clone());
                    }
                    cx.emit(DevicesChanged);
                    cx.notify();
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
        self.booting.push(avd.to_string());
        cx.notify();
        let name = avd.to_string();
        cx.spawn(async move |this, cx| {
            let boot_name = name.clone();
            let result = cx
                .background_spawn(async move {
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
                        .stderr(Stdio::null())
                        .spawn()
                        .context("starting the emulator")?;
                    // Reaped when it exits, so no zombie is left behind.
                    let mut child = child;
                    std::thread::Builder::new()
                        .name("emulator-wait".into())
                        .spawn(move || {
                            let _ = child.wait();
                        })
                        .ok();
                    wait_for_boot(&adb, &boot_name)
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

/// Waits for an emulator running `avd` to finish booting; returns its serial.
fn wait_for_boot(adb: &Adb, avd: &str) -> Result<String> {
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
        std::thread::sleep(Duration::from_secs(1));
    }
    bail!(
        "{avd} did not finish booting within {} seconds",
        BOOT_TIMEOUT.as_secs()
    )
}
