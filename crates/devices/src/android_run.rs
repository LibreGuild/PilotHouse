//! Run an Android app on a device in one click: build the chosen variant
//! with Gradle, install it on the chosen device and start it, the way
//! Android Studio's Run button does.
//!
//! The build asks Gradle only for the device's ABI
//! (`android.injected.build.abi`), so native code is compiled once rather
//! than for every ABI. The APK and the application id come from the
//! plugin's `output-metadata.json`, so suffixes per variant need no parsing.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    Window, div, px,
};
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::adb::{self, Adb};
use crate::android_project::{self, AndroidModule};
use crate::manager::DeviceManager;

/// Build output lines kept.
const LOG_LINES: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    Idle,
    Building,
    Installing,
    Launching,
    Running { package: String, took: Duration },
    Failed(String),
}

pub enum AndroidRunEvent {
    /// The app started on a device: show its screen.
    Launched,
}

/// The Android card of the Run tab, for one Gradle project.
pub struct AndroidRun {
    root: PathBuf,
    modules: Vec<AndroidModule>,
    module: usize,
    /// The chosen variant of each module, by name.
    variant: Vec<String>,
    manager: Entity<DeviceManager>,
    stage: Stage,
    log: VecDeque<SharedString>,
    show_log: bool,
    started: Option<Instant>,
    /// The Gradle process while building, to stop it.
    gradle: Arc<Mutex<Option<Child>>>,
    serial: Option<String>,
    _run: Option<Task<()>>,
    _tick: Option<Task<()>>,
}

impl EventEmitter<AndroidRunEvent> for AndroidRun {}

impl Drop for AndroidRun {
    /// A build no card shows any more is not left running.
    fn drop(&mut self) {
        if let Some(mut child) = self.gradle.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
        }
    }
}

impl AndroidRun {
    /// The card for `folder`, when it is a Gradle project with an app module.
    pub fn detect(folder: &std::path::Path) -> Option<(PathBuf, Vec<AndroidModule>)> {
        let root = android_project::gradle_root(folder)?;
        let modules = android_project::find_modules(&root);
        (!modules.is_empty()).then_some((root, modules))
    }

    pub fn new(root: PathBuf, modules: Vec<AndroidModule>, cx: &mut Context<Self>) -> Self {
        let manager = DeviceManager::global(cx);
        manager.update(cx, |manager, cx| manager.ensure_tracking(cx));
        cx.observe(&manager, |_, _, cx| cx.notify()).detach();
        let variant = modules
            .iter()
            .map(|module| default_variant(module).unwrap_or_default())
            .collect();
        Self {
            root,
            modules,
            module: 0,
            variant,
            manager,
            stage: Stage::Idle,
            log: VecDeque::new(),
            show_log: false,
            started: None,
            gradle: Arc::default(),
            serial: None,
            _run: None,
            _tick: None,
        }
    }

    /// Updates the modules after the build files changed, keeping the
    /// choices that still exist.
    pub fn set_modules(&mut self, modules: Vec<AndroidModule>, cx: &mut Context<Self>) {
        if modules == self.modules {
            return;
        }
        let chosen = self.variant.get(self.module).cloned();
        let path = self
            .modules
            .get(self.module)
            .map(|module| module.path.clone());
        self.variant = modules
            .iter()
            .map(|module| {
                let keep = chosen
                    .clone()
                    .filter(|_| Some(&module.path) == path.as_ref())
                    .filter(|name| module.variants.iter().any(|variant| &variant.name == name));
                keep.or_else(|| default_variant(module)).unwrap_or_default()
            })
            .collect();
        self.module = path
            .and_then(|path| modules.iter().position(|module| module.path == path))
            .unwrap_or(0);
        self.modules = modules;
        cx.notify();
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            Stage::Building | Stage::Installing | Stage::Launching
        )
    }

    fn push_log(&mut self, line: impl Into<SharedString>) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line.into());
    }

    pub fn run(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let manager = self.manager.read(cx);
        let Some(adb) = manager.adb().cloned() else {
            self.stage = Stage::Failed("The Android SDK was not found.".into());
            cx.notify();
            return;
        };
        let Some(device) = manager
            .selected()
            .filter(|device| device.is_ready())
            .cloned()
        else {
            self.stage = Stage::Failed("Pick a device, or start an emulator, first.".into());
            cx.notify();
            return;
        };
        let Some(module) = self.modules.get(self.module).cloned() else {
            return;
        };
        let Some(variant) = module
            .variants
            .iter()
            .find(|variant| variant.name == self.variant[self.module])
            .cloned()
        else {
            return;
        };
        self.log.clear();
        self.push_log(format!(
            "Building {} ({}) for {}…",
            module.path,
            variant.name,
            device.display_name()
        ));
        self.stage = Stage::Building;
        self.serial = Some(device.serial.clone());
        let started = Instant::now();
        self.started = Some(started);
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        }));
        cx.notify();

        let (lines, receiver) = smol::channel::unbounded::<String>();
        let root = self.root.clone();
        let gradle = self.gradle.clone();
        let serial = device.serial.clone();
        let task = format!("{}:assemble{}", module.path, variant.task_suffix());
        let module_dir = module.dir.clone();
        let apk_variant = variant.clone();
        let build_adb = adb.clone();
        let build = cx.background_spawn(async move {
            let abis = build_adb
                .shell(&serial, "getprop ro.product.cpu.abilist")
                .unwrap_or_default()
                .trim()
                .to_string();
            build_with_gradle(&root, &task, &serial, &abis, &gradle, lines)
        });
        // Gradle's output, as it comes; ends when the build does.
        let reader = cx.spawn(async move |this, cx| {
            while let Ok(first) = receiver.recv().await {
                let mut batch = vec![first];
                while let Ok(line) = receiver.try_recv() {
                    batch.push(line);
                }
                if this
                    .update(cx, |run, cx| {
                        batch.into_iter().for_each(|line| run.push_log(line));
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        });
        self._run = Some(cx.spawn(async move |this, cx| {
            let built = build.await;
            reader.await;
            if let Err(message) = built {
                let _ = this.update(cx, |run, cx| run.fail(message, cx));
                return;
            }
            let _ = this.update(cx, |run, cx| {
                run.stage = Stage::Installing;
                run.push_log("Installing…");
                cx.notify();
            });
            let serial = device.serial.clone();
            let install_adb = adb.clone();
            let installed = cx
                .background_spawn(async move {
                    install(&install_adb, &serial, &apk_variant, &module_dir)
                })
                .await;
            let package = match installed {
                Ok(package) => package,
                Err(message) => {
                    let _ = this.update(cx, |run, cx| run.fail(message, cx));
                    return;
                }
            };
            let _ = this.update(cx, |run, cx| {
                run.stage = Stage::Launching;
                run.push_log(format!("Starting {package}…"));
                cx.notify();
            });
            let (serial, launch_package) = (device.serial.clone(), package.clone());
            let launched = cx
                .background_spawn(async move {
                    let _ = adb.shell(&serial, &format!("am force-stop {launch_package}"));
                    adb.launch(&serial, &launch_package)
                })
                .await;
            let _ = this.update(cx, |run, cx| {
                run._tick = None;
                match launched {
                    Ok(()) => {
                        run.push_log("Started.");
                        run.stage = Stage::Running {
                            package,
                            took: started.elapsed(),
                        };
                        cx.emit(AndroidRunEvent::Launched);
                    }
                    Err(err) => run.fail(format!("{err:#}"), cx),
                }
                cx.notify();
            });
        }));
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self._tick = None;
        self.push_log(message.clone());
        self.stage = Stage::Failed(message);
        // The log usually says why.
        self.show_log = true;
        cx.notify();
    }

    /// Stops the build, or the running app.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(mut child) = self.gradle.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
        }
        if let (Stage::Running { package, .. }, Some(serial), Some(adb)) = (
            &self.stage,
            &self.serial,
            self.manager.read(cx).adb().cloned(),
        ) {
            let (serial, package) = (serial.clone(), package.clone());
            cx.background_spawn(async move {
                let _ = adb.shell(&serial, &format!("am force-stop {package}"));
            })
            .detach();
        }
        self._run = None;
        self._tick = None;
        self.stage = Stage::Idle;
        self.push_log("Stopped.");
        cx.notify();
    }

    fn render_status(&self, cx: &App) -> impl IntoElement {
        let elapsed = self
            .started
            .map(|started| started.elapsed().as_secs())
            .unwrap_or_default();
        let muted = cx.theme().muted_foreground;
        let (text, color, spinning): (String, gpui_kit::Hsla, bool) = match &self.stage {
            Stage::Idle => ("Ready to run".into(), muted, false),
            Stage::Building => (format!("Building… {elapsed}s"), muted, true),
            Stage::Installing => (format!("Installing… {elapsed}s"), muted, true),
            Stage::Launching => ("Starting the app…".into(), muted, true),
            Stage::Running { package, took } => (
                format!("{package} is running · {:.0}s", took.as_secs_f32()),
                cx.theme().success,
                false,
            ),
            Stage::Failed(message) => (message.clone(), cx.theme().danger, false),
        };
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .text_xs()
            .text_color(color)
            .when(spinning, |this| this.child(Spinner::new().xsmall()))
            .child(div().min_w_0().truncate().child(text))
    }

    fn render_pickers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let modules: Vec<String> = self
            .modules
            .iter()
            .map(|module| module.path.clone())
            .collect();
        let current_module = self.module;
        let module_picker = (modules.len() > 1).then(|| {
            let this = this.clone();
            Button::new("android-module")
                .outline()
                .xsmall()
                .label(modules[current_module].clone())
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                    modules.iter().enumerate().fold(menu, |menu, (ix, path)| {
                        let this = this.clone();
                        menu.item(
                            PopupMenuItem::new(path.clone())
                                .checked(ix == current_module)
                                .on_click(move |_, _, cx| {
                                    _ = this.update(cx, |run, cx| {
                                        run.module = ix;
                                        cx.notify();
                                    });
                                }),
                        )
                    })
                })
        });
        let variants: Vec<String> = self.modules[self.module]
            .variants
            .iter()
            .map(|variant| variant.name.clone())
            .collect();
        let chosen = self.variant[self.module].clone();
        let variant_picker = {
            let this = this.clone();
            let chosen_label = chosen.clone();
            Button::new("android-variant")
                .outline()
                .xsmall()
                .icon(Icons::Layers)
                .label(chosen_label)
                .dropdown_caret(true)
                .tooltip("Build variant")
                .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                    variants
                        .iter()
                        .fold(menu.scrollable(true).max_h(px(360.)), |menu, name| {
                            let this = this.clone();
                            let picked = name.clone();
                            menu.item(
                                PopupMenuItem::new(name.clone())
                                    .checked(*name == chosen)
                                    .on_click(move |_, _, cx| {
                                        let picked = picked.clone();
                                        _ = this.update(cx, |run, cx| {
                                            let module = run.module;
                                            run.variant[module] = picked;
                                            cx.notify();
                                        });
                                    }),
                            )
                        })
                })
        };
        let manager = self.manager.read(cx);
        let device_label = manager
            .selected()
            .map(|device| device.display_name())
            .unwrap_or_else(|| "No device".into());
        let manager_entity = self.manager.clone();
        let device_picker = Button::new("android-device")
            .outline()
            .xsmall()
            .icon(Icons::Smartphone)
            .label(device_label)
            .dropdown_caret(true)
            .tooltip("Device to run on")
            .menu(gpui_kit::Anchor::TopLeft, move |menu, window, cx| {
                crate::devices_panel::device_menu(menu, &manager_entity, window, cx)
            });
        h_flex()
            .flex_wrap()
            .gap_1()
            .children(module_picker)
            .child(variant_picker)
            .child(device_picker)
    }
}

impl Render for AndroidRun {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let running = matches!(self.stage, Stage::Running { .. });
        let can_run = !busy
            && self
                .manager
                .read(cx)
                .selected()
                .is_some_and(|device| device.is_ready());
        let log: Vec<SharedString> = if self.show_log {
            self.log.iter().rev().take(300).rev().cloned().collect()
        } else {
            Vec::new()
        };
        v_flex()
            .gap_2()
            .p_2()
            .mb_1()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.4))
            .child(
                h_flex()
                    .gap_1p5()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(
                        Icon::new(Icons::Smartphone)
                            .small()
                            .text_color(cx.theme().green),
                    )
                    .child("Android app")
                    .child(div().flex_1())
                    .child(
                        Button::new("android-log")
                            .ghost()
                            .xsmall()
                            .icon(Icons::ScrollText)
                            .label("Build log")
                            .selected(self.show_log)
                            .on_click(cx.listener(|run, _, _, cx| {
                                run.show_log = !run.show_log;
                                cx.notify();
                            })),
                    ),
            )
            .child(self.render_pickers(cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("android-run")
                            .primary()
                            .small()
                            .icon(if running {
                                Icons::RotateCw
                            } else {
                                Icons::Play
                            })
                            .label(if running { "Run again" } else { "Run" })
                            .disabled(!can_run)
                            .tooltip("Build, install and start the app on the device")
                            .on_click(cx.listener(|run, _, _, cx| run.run(cx))),
                    )
                    .when(busy || running, |this| {
                        this.child(
                            Button::new("android-stop")
                                .ghost()
                                .small()
                                .icon(Icons::SquareStop)
                                .tooltip(if busy {
                                    "Stop the build"
                                } else {
                                    "Stop the app"
                                })
                                .accessibility_label("Stop")
                                .on_click(cx.listener(|run, _, _, cx| run.stop(cx))),
                        )
                    })
                    .child(self.render_status(cx)),
            )
            .when(self.show_log, |this| {
                this.child(
                    v_flex()
                        .id("android-build-log")
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().background)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .when(log.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Nothing built yet."),
                            )
                        })
                        .children(log.into_iter().map(|line| {
                            let color = log_color(&line, cx);
                            div()
                                .when_some(color, |this, color| this.text_color(color))
                                .child(line)
                        })),
                )
            })
    }
}

/// The first debug variant, as Android Studio picks.
fn default_variant(module: &AndroidModule) -> Option<String> {
    module
        .variants
        .iter()
        .find(|variant| variant.build_type == "debug")
        .or(module.variants.first())
        .map(|variant| variant.name.clone())
}

fn log_color(line: &str, cx: &App) -> Option<gpui_kit::Hsla> {
    let theme = cx.theme();
    if line.starts_with("e:")
        || line.contains("error:")
        || line.starts_with("FAILURE")
        || line.contains("BUILD FAILED")
    {
        Some(theme.danger)
    } else if line.starts_with("w:") || line.contains("warning:") {
        Some(theme.warning)
    } else if line.contains("BUILD SUCCESSFUL") {
        Some(theme.success)
    } else if line.starts_with("> Task") {
        Some(theme.muted_foreground)
    } else {
        None
    }
}

/// Runs `task` with the project's wrapper, sending its output line by line.
fn build_with_gradle(
    root: &std::path::Path,
    task: &str,
    serial: &str,
    abis: &str,
    gradle: &Mutex<Option<Child>>,
    lines: smol::channel::Sender<String>,
) -> Result<(), String> {
    let wrapper = root.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    let mut command = ph_env::command(&wrapper);
    command
        .current_dir(root)
        .envs(ph_env::shell_env())
        .env("ANDROID_SERIAL", serial)
        .args([task, "--console=plain"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(jdk) = crate::sdk::studio_jdk() {
        let _ = lines.send_blocking(format!("Using Android Studio's JDK ({}).", jdk.display()));
        command.env("JAVA_HOME", jdk);
    }
    if !abis.is_empty()
        && abis
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_,".contains(c))
    {
        command.arg(format!("-Pandroid.injected.build.abi={abis}"));
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("Gradle did not start: {err}"))?;
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|pipe| {
        let lines = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if lines.send_blocking(line).is_err() {
                    return;
                }
            }
        })
    })
    .collect();
    if let Ok(mut slot) = gradle.lock() {
        *slot = Some(child);
    }
    for reader in readers {
        let _ = reader.join();
    }
    // Stopped by the user: the slot is empty.
    let Some(mut child) = gradle.lock().ok().and_then(|mut slot| slot.take()) else {
        return Err("Stopped.".into());
    };
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err("The build failed; the build log says why.".into()),
        Err(err) => Err(format!("Gradle failed: {err}")),
    }
}

/// Installs the APK of `variant`'s last build; returns its application id.
fn install(
    adb: &Adb,
    serial: &str,
    variant: &android_project::Variant,
    module: &std::path::Path,
) -> Result<String, String> {
    let (package, apk) = variant.built_apk(module).ok_or_else(|| {
        format!(
            "No APK of {} was found under {}.",
            variant.name,
            module.display()
        )
    })?;
    if !adb::is_package_name(&package) {
        return Err(format!("`{package}` is not a package name."));
    }
    adb.install(serial, &apk)
        .map_err(|err| format!("{err:#}"))?;
    Ok(package)
}
