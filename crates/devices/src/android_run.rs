//! Run an Android app on a device in one click: build the chosen variant
//! with Gradle, install it on the chosen device and start it, the way
//! Android Studio's Run button does.
//!
//! The build asks Gradle only for the device's ABI
//! (`android.injected.build.abi`), so native code is compiled once rather
//! than for every ABI. The APK and the application id come from the
//! plugin's `output-metadata.json`, so suffixes per variant need no parsing.
//!
//! The card keeps what was chosen (module and variant) per project. A run
//! hands the app to Logcat, which then shows its lines and says when it
//! crashes; a build that fails can be sent to the chat; an install the
//! device refuses because another build of the app is on it offers to
//! remove that one first. The app's menu does what one otherwise types
//! `adb shell` for: stop it, clear its data, uninstall it, grant its
//! permissions, open a link in it.

use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::PopupMenuItem,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    WeakEntity, Window, div,
};
use ph_media::ToChat;
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::adb::{self, Adb};
use crate::android_project::{self, AndroidModule};
use crate::manager::DeviceManager;

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    Idle,
    Building,
    Installing,
    Launching,
    Running {
        package: String,
        took: Duration,
    },
    /// Build APK finished: where the APK is.
    Built {
        apk: PathBuf,
        took: Duration,
    },
    /// Something asked of the app was done ("Cleared its data").
    Done(String),
    Failed {
        message: String,
        failure: Failure,
    },
}

/// What failed, for what the card offers about it.
#[derive(Debug, Clone, PartialEq)]
enum Failure {
    /// Gradle: its log says why, and the chat can be asked.
    Build,
    /// The device refused the APK because the app on it is another build
    /// (other signature, newer version): removing that one helps.
    Conflict {
        package: String,
    },
    /// The app crashed: what its exception printed, for the chat.
    Crash {
        report: String,
    },
    Other,
}

pub enum AndroidRunEvent {
    /// A run began: the device's log is wanted from now, so that what the
    /// app writes as it starts is there.
    Started,
    /// The app `package` started on a device: show its screen.
    Launched { package: String },
    /// Show the build log, in the bottom panel's terminal: run `command`
    /// (it follows the log file as the build writes it).
    ShowLog { command: String },
    /// Something for the chat: a build's errors.
    ToChat(ToChat),
}

/// What the card keeps per project: the module and each module's variant.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Chosen {
    #[serde(default)]
    module: String,
    #[serde(default)]
    variants: std::collections::HashMap<String, String>,
}

/// The settings key of a project's choices.
fn chosen_key(root: &std::path::Path) -> String {
    format!("android.run:{}", root.display())
}

/// Lines of a failed build sent to the chat: its end, where Gradle says
/// what went wrong.
const BUILD_LOG_LINES: usize = 150;

/// The Android card of the Run tab, for one Gradle project.
pub struct AndroidRun {
    root: PathBuf,
    modules: Vec<AndroidModule>,
    module: usize,
    /// The chosen variant of each module, by name.
    variant: Vec<String>,
    /// Where the choices are kept between runs of the app.
    store: Option<ph_store::Store>,
    /// The app installed last, for its menu.
    package: Option<String>,
    manager: Entity<DeviceManager>,
    stage: Stage,
    /// The build's output, as it comes, for the terminal that shows it.
    log: Option<std::io::BufWriter<std::fs::File>>,
    log_path: PathBuf,
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
            kill_tree(&mut child);
        }
        self.log = None;
        let _ = std::fs::remove_file(&self.log_path);
    }
}

impl AndroidRun {
    /// The card for `folder`, when it is a Gradle project with an app module.
    pub fn detect(folder: &std::path::Path) -> Option<(PathBuf, Vec<AndroidModule>)> {
        let root = android_project::gradle_root(folder)?;
        let modules = android_project::find_modules(&root);
        (!modules.is_empty()).then_some((root, modules))
    }

    pub fn new(
        root: PathBuf,
        modules: Vec<AndroidModule>,
        store: Option<ph_store::Store>,
        cx: &mut Context<Self>,
    ) -> Self {
        let manager = DeviceManager::global(cx);
        manager.update(cx, |manager, cx| manager.ensure_tracking(cx));
        cx.observe(&manager, |_, _, cx| cx.notify()).detach();
        let chosen: Chosen = store
            .as_ref()
            .map(|store| store.setting(&chosen_key(&root)))
            .unwrap_or_default();
        let modules = with_chosen_variants(modules, &chosen);
        let variant = modules
            .iter()
            .map(|module| {
                chosen
                    .variants
                    .get(&module.path)
                    .cloned()
                    .or_else(|| default_variant(module))
                    .unwrap_or_default()
            })
            .collect();
        let module = modules
            .iter()
            .position(|module| module.path == chosen.module)
            .unwrap_or(0);
        Self {
            root,
            modules,
            module,
            variant,
            store,
            package: None,
            manager,
            stage: Stage::Idle,
            log: None,
            log_path: std::env::temp_dir().join(format!(
                "pilothouse-build-{}-{}.log",
                std::process::id(),
                cx.entity_id().as_u64()
            )),
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
        let modules = with_chosen_variants(modules, &self.chosen());
        if modules == self.modules {
            return;
        }
        let chosen = self.chosen();
        self.variant = modules
            .iter()
            .map(|module| {
                chosen
                    .variants
                    .get(&module.path)
                    .filter(|name| module.variants.iter().any(|variant| &variant.name == *name))
                    .cloned()
                    .or_else(|| default_variant(module))
                    .unwrap_or_default()
            })
            .collect();
        self.module = modules
            .iter()
            .position(|module| module.path == chosen.module)
            .unwrap_or(0);
        self.modules = modules;
        cx.notify();
    }

    /// What is chosen now.
    fn chosen(&self) -> Chosen {
        Chosen {
            module: self
                .modules
                .get(self.module)
                .map(|module| module.path.clone())
                .unwrap_or_default(),
            variants: self
                .modules
                .iter()
                .zip(&self.variant)
                .map(|(module, variant)| (module.path.clone(), variant.clone()))
                .collect(),
        }
    }

    /// Keeps what is chosen for the next time the project is open.
    fn keep_choices(&self) {
        if let Some(store) = &self.store {
            let _ = store.set_setting(&chosen_key(&self.root), &self.chosen());
        }
    }

    fn choose_module(&mut self, module: usize, cx: &mut Context<Self>) {
        self.module = module;
        self.keep_choices();
        cx.notify();
    }

    fn choose_variant(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(module) = self.modules.get_mut(self.module) else {
            return;
        };
        // One typed in, which the build files do not show.
        if !module.variants.iter().any(|variant| variant.name == name) {
            module
                .variants
                .push(android_project::Variant::custom(&name));
        }
        self.variant[self.module] = name;
        self.keep_choices();
        cx.notify();
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            Stage::Building | Stage::Installing | Stage::Launching
        )
    }

    fn push_log(&mut self, line: impl Into<SharedString>) {
        use std::io::Write as _;
        if let Some(log) = &mut self.log {
            let line: SharedString = line.into();
            if writeln!(log, "{line}").and_then(|_| log.flush()).is_err() {
                self.log = None;
            }
        }
    }

    /// Asks for the build log in the bottom panel's terminal.
    fn show_log(&mut self, cx: &mut Context<Self>) {
        if !self.log_path.exists() {
            return;
        }
        cx.emit(AndroidRunEvent::ShowLog {
            command: follow_command(&self.log_path),
        });
    }

    pub fn run(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let manager = self.manager.read(cx);
        let Some(adb) = manager.adb().cloned() else {
            let message = if manager.looking_for_sdk() {
                "Still looking for the Android SDK; try again in a moment."
            } else {
                "The Android SDK was not found."
            };
            self.stage = failed(message, Failure::Other);
            cx.notify();
            return;
        };
        let Some(device) = manager
            .selected()
            .filter(|device| device.is_ready())
            .cloned()
        else {
            self.stage = failed(
                "Pick a device, or start an emulator, first.",
                Failure::Other,
            );
            cx.notify();
            return;
        };
        let Some(module) = self.modules.get(self.module).cloned() else {
            return;
        };
        // Said now, not after a build the device then refuses.
        if let (Some(needed), Some(has)) = (
            module.min_sdk,
            device.about.as_ref().and_then(|about| about.api),
        ) && has < needed
        {
            self.stage = failed(
                format!(
                    "This app needs Android API {needed} or later; {} has API {has}.",
                    device.display_name()
                ),
                Failure::Other,
            );
            cx.notify();
            return;
        }
        cx.emit(AndroidRunEvent::Started);
        let Some(variant) = module
            .variants
            .iter()
            .find(|variant| variant.name == self.variant[self.module])
            .cloned()
        else {
            return;
        };
        self.serial = Some(device.serial.clone());
        let started = Instant::now();
        let task = format!("{}:assemble{}", module.path, variant.task_suffix());
        let module_dir = module.dir.clone();
        let apk_variant = variant.clone();
        let (build_adb, serial) = (adb.clone(), device.serial.clone());
        let build = self.start_gradle(
            format!(
                "Building {} ({}) for {}…",
                module.path,
                variant.name,
                device.display_name()
            ),
            task,
            Some(device.serial.clone()),
            // Only the device's ABIs: a quicker build.
            move || {
                build_adb
                    .shell(&serial, "getprop ro.product.cpu.abilist")
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            },
            cx,
        );
        self._run = Some(cx.spawn(async move |this, cx| {
            if let Err(message) = build.await {
                let _ = this.update(cx, |run, cx| run.fail(message, Failure::Build, cx));
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
                Err((message, failure)) => {
                    let _ = this.update(cx, |run, cx| run.fail(message, failure, cx));
                    return;
                }
            };
            let _ = this.update(cx, |run, cx| {
                run.package = Some(package.clone());
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
                        cx.emit(AndroidRunEvent::Launched {
                            package: package.clone(),
                        });
                        run.stage = Stage::Running {
                            package,
                            took: started.elapsed(),
                        };
                    }
                    Err(err) => run.fail(format!("{err:#}"), Failure::Other, cx),
                }
                cx.notify();
            });
        }));
    }

    /// Starts Gradle on `task` (for the device `serial`, built for the
    /// ABIs `abis` gives, when there is one): the log fills as it builds
    /// and the status counts the seconds. The task ends with the build.
    fn start_gradle(
        &mut self,
        heading: String,
        task: String,
        serial: Option<String>,
        abis: impl FnOnce() -> String + Send + 'static,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        // A new log for each build.
        self.log = std::fs::File::create(&self.log_path)
            .ok()
            .map(std::io::BufWriter::new);
        self.push_log(heading);
        self.stage = Stage::Building;
        self.started = Some(Instant::now());
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
        let (root, gradle) = (self.root.clone(), self.gradle.clone());
        let build = cx.background_spawn(async move {
            let abis = abis();
            build_with_gradle(
                &root,
                &task,
                serial.as_deref().unwrap_or_default(),
                &abis,
                &gradle,
                lines,
            )
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
        cx.spawn(async move |_, _| {
            let built = build.await;
            reader.await;
            built
        })
    }

    /// Build APK, as Android Studio has it: builds the picked module and
    /// variant for every ABI, no device needed, and says where the APK is.
    pub fn build_apk(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
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
        let task = format!("{}:assemble{}", module.path, variant.task_suffix());
        let started = Instant::now();
        let build = self.start_gradle(
            format!("Building the APK of {} ({})…", module.path, variant.name),
            task,
            None,
            String::new,
            cx,
        );
        let module_dir = module.dir.clone();
        self._run = Some(cx.spawn(async move |this, cx| {
            let built = build.await;
            let found = cx
                .background_spawn(async move {
                    variant
                        .built_apk(&module_dir)
                        .map(|(_, apk)| apk)
                        .ok_or_else(|| {
                            format!(
                                "No APK of {} was found under {}.",
                                variant.name,
                                module_dir.display()
                            )
                        })
                })
                .await;
            let _ = this.update(cx, |run, cx| {
                run._tick = None;
                match built.and(found) {
                    Ok(apk) => {
                        run.push_log(format!("Built {}.", apk.display()));
                        run.stage = Stage::Built {
                            apk,
                            took: started.elapsed(),
                        };
                    }
                    Err(message) => run.fail(message, Failure::Build, cx),
                }
                cx.notify();
            });
        }));
    }

    fn fail(&mut self, message: String, failure: Failure, cx: &mut Context<Self>) {
        self._tick = None;
        self.push_log(message.clone());
        // A build's log says why it failed.
        if failure == Failure::Build {
            self.show_log(cx);
        }
        self.stage = Stage::Failed { message, failure };
        cx.notify();
    }

    /// Sends the end of the failed build's log to the chat.
    fn send_build_log(&mut self, cx: &mut Context<Self>) {
        let path = self.log_path.clone();
        cx.spawn(async move |this, cx| {
            let text = cx
                .background_spawn(async move {
                    let log = std::fs::read(&path).unwrap_or_default();
                    let log = String::from_utf8_lossy(&log);
                    let lines: Vec<&str> = log.lines().collect();
                    lines[lines.len().saturating_sub(BUILD_LOG_LINES)..].join("\n")
                })
                .await;
            let _ = this.update(cx, |_, cx| {
                cx.emit(AndroidRunEvent::ToChat(ToChat::Text {
                    label: "build log".into(),
                    text: format!("The Android build failed:\n```\n{text}\n```"),
                }))
            });
        })
        .detach();
    }

    /// Removes the app the device would not replace, then runs again.
    fn uninstall_and_run(&mut self, package: String, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.manager.read(cx).adb().cloned(), self.serial.clone())
        else {
            return;
        };
        self.stage = Stage::Installing;
        self.push_log(format!("Uninstalling {package}…"));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let removed = cx
                .background_spawn(async move { adb.uninstall(&serial, &package) })
                .await;
            let _ = this.update(cx, |run, cx| match removed {
                Ok(()) => {
                    run.stage = Stage::Idle;
                    run.run(cx);
                }
                Err(err) => run.fail(format!("{err:#}"), Failure::Other, cx),
            });
        })
        .detach();
    }

    /// Does `action` to the project's app on the chosen device, off the UI
    /// thread, and says `done` (or what went wrong).
    fn on_app(
        &mut self,
        done: &'static str,
        action: impl FnOnce(&Adb, &str, &str) -> anyhow::Result<String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let manager = self.manager.read(cx);
        let (Some(adb), Some(serial)) = (
            manager.adb().cloned(),
            manager
                .selected()
                .filter(|device| device.is_ready())
                .map(|device| device.serial.clone()),
        ) else {
            self.stage = failed("Pick a device first.", Failure::Other);
            cx.notify();
            return;
        };
        // The app run last; else the one the chosen variant built.
        let known = self.package.clone();
        let built = self.modules.get(self.module).cloned().zip(
            self.modules
                .get(self.module)
                .and_then(|module| {
                    module
                        .variants
                        .iter()
                        .find(|variant| variant.name == self.variant[self.module])
                })
                .cloned(),
        );
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let package = known
                        .or_else(|| {
                            let (module, variant) = built?;
                            variant.built_apk(&module.dir).map(|(id, _)| id)
                        })
                        .ok_or_else(|| anyhow::anyhow!("Run the app once first."))?;
                    let said = action(&adb, &serial, &package)?;
                    Ok::<_, anyhow::Error>(if said.is_empty() {
                        format!("{done} {package}.")
                    } else {
                        said
                    })
                })
                .await;
            let _ = this.update(cx, |run, cx| {
                run.stage = match result {
                    Ok(said) => Stage::Done(said),
                    Err(err) => failed(format!("{err:#}"), Failure::Other),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// The device's log says an app crashed: when it is the one this card
    /// started, the card says so in place of "is running".
    pub fn crashed(&mut self, crash: &crate::logcat::Crash, cx: &mut Context<Self>) {
        let Stage::Running { package, .. } = &self.stage else {
            return;
        };
        let ours = crash
            .process
            .as_deref()
            .is_some_and(|process| crate::logcat::is_process_of(process, package, false));
        if ours {
            self.stage = failed(
                format!("{package} crashed: {}", crash.summary()),
                Failure::Crash {
                    report: crash.report(),
                },
            );
            cx.notify();
        }
    }

    /// Stops the build, or the running app.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(mut child) = self.gradle.lock().ok().and_then(|mut slot| slot.take()) {
            kill_tree(&mut child);
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
            Stage::Built { apk, took } => (
                format!(
                    "{} built · {:.0}s",
                    apk.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    took.as_secs_f32()
                ),
                cx.theme().success,
                false,
            ),
            Stage::Done(said) => (said.clone(), muted, false),
            Stage::Failed { message, .. } => (message.clone(), cx.theme().danger, false),
        };
        h_flex()
            .id("android-status")
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .text_xs()
            .text_color(color)
            .when(spinning, |this| this.child(ph_ui::Loader::new().xsmall()))
            // One line in the card; all of it on hover.
            .tooltip({
                let all = ph_ui::tip(text.clone());
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(all.clone()).build(window, cx)
                }
            })
            .child(div().min_w_0().truncate().child(text))
    }

    /// The app's menu: what one otherwise types `adb shell` for.
    fn render_app_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        Button::new("android-app-menu")
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .tooltip("The app on the device: stop, clear data, uninstall…")
            .accessibility_label("App actions")
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                let item = |label: &'static str, act: fn(&mut Self, &mut Context<Self>)| {
                    let this = this.clone();
                    PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        _ = this.update(cx, act);
                    })
                };
                let link = this.clone();
                menu.item(item("Force stop", |run, cx| {
                    run.on_app(
                        "Stopped",
                        |adb, serial, package| {
                            adb.force_stop(serial, package).map(|()| String::new())
                        },
                        cx,
                    )
                }))
                .item(item("Clear data", |run, cx| {
                    run.on_app(
                        "Cleared the data of",
                        |adb, serial, package| {
                            adb.clear_data(serial, package).map(|()| String::new())
                        },
                        cx,
                    )
                }))
                .item(item("Grant its permissions", |run, cx| {
                    run.on_app(
                        "",
                        |adb, serial, package| {
                            let granted = adb.grant_permissions(serial, package)?;
                            Ok(format!("Granted {granted} permissions to {package}."))
                        },
                        cx,
                    )
                }))
                .item(
                    PopupMenuItem::new("Open a link…")
                        .on_click(move |_, window, cx| ask_link(link.clone(), window, cx)),
                )
                .item(item("App info on the device", |run, cx| {
                    run.on_app(
                        "Showing the settings of",
                        |adb, serial, package| {
                            adb.app_info(serial, package).map(|()| String::new())
                        },
                        cx,
                    )
                }))
                .separator()
                .item(item("Uninstall", |run, cx| {
                    run.on_app(
                        "Uninstalled",
                        |adb, serial, package| {
                            adb.uninstall(serial, package).map(|()| String::new())
                        },
                        cx,
                    )
                }))
            })
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
                                    _ = this.update(cx, |run, cx| run.choose_module(ix, cx));
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
        let variant_picker =
            {
                let this = this.clone();
                let chosen_label = chosen.clone();
                Button::new("android-variant")
                    .outline()
                    .xsmall()
                    .icon(Icons::Layers)
                    .label(chosen_label)
                    .dropdown_caret(true)
                    .tooltip("Build variant")
                    .menu(gpui_kit::Anchor::TopLeft, move |menu, window, _| {
                        let other = this.clone();
                        variants
                            .iter()
                            .fold(
                                menu.scrollable(true).max_h(ph_ui::scaled_px(360., window)),
                                |menu, name| {
                                    let this = this.clone();
                                    let picked = name.clone();
                                    menu.item(
                                        PopupMenuItem::new(name.clone())
                                            .checked(*name == chosen)
                                            .on_click(move |_, _, cx| {
                                                let picked = picked.clone();
                                                _ = this.update(cx, |run, cx| {
                                                    run.choose_variant(picked, cx)
                                                });
                                            }),
                                    )
                                },
                            )
                            .separator()
                            // The build files are read, not run: flavors a
                            // plugin of the project's own sets up are not seen.
                            .item(PopupMenuItem::new("Other…").on_click(move |_, window, cx| {
                                ask_variant(other.clone(), window, cx)
                            }))
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
        let built_apk = match &self.stage {
            Stage::Built { apk, .. } => Some(apk.clone()),
            _ => None,
        };
        let failure = match &self.stage {
            Stage::Failed { failure, .. } => Some(failure.clone()),
            _ => None,
        };
        let can_run = !busy
            && self
                .manager
                .read(cx)
                .selected()
                .is_some_and(|device| device.is_ready());
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
                            .disabled(!self.log_path.exists())
                            .tooltip("Show the build log in the terminal")
                            .on_click(cx.listener(|run, _, _, cx| run.show_log(cx))),
                    )
                    .child(self.render_app_menu(cx)),
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
                    .child(
                        Button::new("android-build-apk")
                            .ghost()
                            .small()
                            .icon(Icons::Package)
                            .label("Build APK")
                            .disabled(busy)
                            .tooltip(ph_ui::tip(
                                "Build the APK of this module and variant; no device needed",
                            ))
                            .on_click(cx.listener(|run, _, _, cx| run.build_apk(cx))),
                    )
                    .when_some(built_apk, |this, apk| {
                        let copy = apk.clone();
                        this.child(
                            Button::new("android-reveal-apk")
                                .ghost()
                                .small()
                                .icon(Icons::FolderOpen)
                                .tooltip("Show the APK in its folder")
                                .accessibility_label("Show the APK")
                                .on_click(move |_, _, cx| ph_ui::platform::reveal(&apk, cx)),
                        )
                        .child(
                            ph_ui::CopyButton::new(
                                "android-copy-apk",
                                copy.to_string_lossy().into_owned(),
                            )
                            .small()
                            .tooltip("Copy the APK's path"),
                        )
                    })
                    .when_some(failure, |this, failure| match failure {
                        Failure::Build => this.child(
                            Button::new("android-build-to-chat")
                                .ghost()
                                .small()
                                .icon(Icons::SendHorizontal)
                                .label("Send to chat")
                                .tooltip("Send the end of the build log to the chat")
                                .on_click(cx.listener(|run, _, _, cx| run.send_build_log(cx))),
                        ),
                        Failure::Conflict { package } => this.child(
                            Button::new("android-uninstall-run")
                                .outline()
                                .small()
                                .label("Uninstall and run again")
                                .tooltip(ph_ui::tip(
                                    "Another build of the app is on the device. Removing it removes its data too.",
                                ))
                                .on_click(cx.listener(move |run, _, _, cx| {
                                    run.uninstall_and_run(package.clone(), cx)
                                })),
                        ),
                        Failure::Crash { report } => this.child(
                            Button::new("android-crash-to-chat")
                                .ghost()
                                .small()
                                .icon(Icons::SendHorizontal)
                                .label("Send to chat")
                                .tooltip("Send the exception and its stack to the chat")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(AndroidRunEvent::ToChat(ToChat::Text {
                                        label: "crash".into(),
                                        text: report.clone(),
                                    }))
                                })),
                        ),
                        Failure::Other => this,
                    })
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
    }
}

fn failed(message: impl Into<String>, failure: Failure) -> Stage {
    Stage::Failed {
        message: message.into(),
        failure,
    }
}

/// `modules` with, for each, the variant chosen before when the build
/// files do not show it (one typed in by the user).
fn with_chosen_variants(mut modules: Vec<AndroidModule>, chosen: &Chosen) -> Vec<AndroidModule> {
    for module in &mut modules {
        if let Some(name) = chosen.variants.get(&module.path)
            && !name.is_empty()
            && !module.variants.iter().any(|variant| &variant.name == name)
        {
            module.variants.push(android_project::Variant::custom(name));
        }
    }
    modules
}

/// Whether what the device said on refusing an APK means that the app on
/// it is another build, which only removing it gets past.
fn is_conflict(said: &str) -> bool {
    [
        "INSTALL_FAILED_UPDATE_INCOMPATIBLE",
        "INSTALL_FAILED_VERSION_DOWNGRADE",
        "signatures do not match",
    ]
    .iter()
    .any(|sign| said.contains(sign))
}

/// A dialog with one field: `on_ok` gets what was typed, when anything was.
fn ask(
    title: &'static str,
    placeholder: &'static str,
    ok: &'static str,
    on_ok: impl Fn(String, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    let field = input.clone();
    // The dialog is built again as it is drawn; each time with this.
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_dialog(cx, move |dialog, window, _| {
        let field = field.clone();
        let typed = field.clone();
        dialog
            .title(title)
            .w(ph_ui::scaled_px(420., window))
            .child(Input::new(&field))
            .footer(ph_ui::dialog_buttons(ok, true))
            .on_ok({
                let on_ok = on_ok.clone();
                move |_, _, cx| {
                    let text = typed.read(cx).value().trim().to_string();
                    if text.is_empty() {
                        return false;
                    }
                    on_ok(text, cx);
                    true
                }
            })
    });
    input.update(cx, |input, cx| input.focus(window, cx));
}

/// Asks for a variant the build files do not show.
fn ask_variant(run: WeakEntity<AndroidRun>, window: &mut Window, cx: &mut App) {
    ask(
        "Build variant",
        "As Gradle names it, e.g. freeStagingDebug",
        "Use",
        move |name, cx| {
            // A task name is made of it: letters and digits only.
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                _ = run.update(cx, |run, cx| run.choose_variant(name, cx));
            }
        },
        window,
        cx,
    );
}

/// Asks for a link to open in the app.
fn ask_link(run: WeakEntity<AndroidRun>, window: &mut Window, cx: &mut App) {
    ask(
        "Open a link in the app",
        "myapp://item/42 or https://example.com/item/42",
        "Open",
        move |url, cx| {
            _ = run.update(cx, |run, cx| {
                run.on_app(
                    "",
                    move |adb, serial, package| {
                        adb.open_link(serial, &url, Some(package))?;
                        Ok(format!("Opened the link in {package}."))
                    },
                    cx,
                )
            });
        },
        window,
        cx,
    );
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
    // Its own process group, so Stop ends the Java client Gradle starts too.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
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
            for line in ph_env::lossy_lines(pipe) {
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

/// Installs the APK of `variant`'s last build; returns its application id,
/// or what went wrong and of what kind.
fn install(
    adb: &Adb,
    serial: &str,
    variant: &android_project::Variant,
    module: &std::path::Path,
) -> Result<String, (String, Failure)> {
    let (package, apk) = variant.built_apk(module).ok_or_else(|| {
        (
            format!(
                "No APK of {} was found under {}.",
                variant.name,
                module.display()
            ),
            Failure::Other,
        )
    })?;
    if !adb::is_package_name(&package) {
        return Err((
            format!("`{package}` is not a package name."),
            Failure::Other,
        ));
    }
    adb.install(serial, &apk).map_err(|err| {
        let said = format!("{err:#}");
        if is_conflict(&said) {
            (
                format!("Another build of {package} is on the device."),
                Failure::Conflict {
                    package: package.clone(),
                },
            )
        } else {
            (said, Failure::Other)
        }
    })?;
    Ok(package)
}

/// A command that prints the log file at `path` and follows it as it grows.
fn follow_command(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        format!(
            "powershell -NoProfile -Command \"Get-Content -Wait -Path '{}'\"",
            path.replace('\'', "''")
        )
    } else {
        format!("tail -n +1 -F '{}'", path.replace('\'', "'\\''"))
    }
}

/// Ends `child` and what it started: the wrapper script (`gradlew`,
/// `gradlew.bat`) starts the Java client that does the build.
fn kill_tree(child: &mut Child) {
    let id = child.id().to_string();
    #[cfg(unix)]
    let _ = ph_env::command("kill")
        .args(["-KILL", &format!("-{id}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    let _ = ph_env::command("taskkill")
        .args(["/T", "/F", "/PID", &id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}
