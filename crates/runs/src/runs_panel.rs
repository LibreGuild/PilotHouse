//! The Run tab: run configurations and the ports local servers listen on.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div,
};
use ph_store::Store;
use ph_ui::Icons;

use crate::detect::{RunConfig, detect};
use crate::ports::{self, Port};

/// How often ports are re-read while the tab is shown.
const PORTS_REFRESH: Duration = Duration::from_secs(3);

pub enum RunsPanelEvent {
    /// Run `command` in a terminal named `name`, in folder `cwd` (the
    /// primary one when `None`).
    Run {
        name: String,
        command: String,
        cwd: Option<PathBuf>,
    },
    /// Show `url` in the project's browser pane.
    OpenUrl(String),
    /// An Android app started on a device: show the Devices pane.
    ShowDevices,
}

pub struct RunsPanel {
    store: Store,
    /// The workspace's folders, named; the primary first.
    folders: Vec<(String, PathBuf)>,
    project_id: i64,
    detected: Vec<RunConfig>,
    custom: Vec<RunConfig>,
    /// The Android card, when a folder is a Gradle project with an app.
    android: Option<Entity<ph_devices::AndroidRun>>,
    ports: Vec<Port>,
    adding: bool,
    /// Show ports of other programs too, not just this project's.
    show_other_ports: bool,
    name_input: Entity<InputState>,
    command_input: Entity<InputState>,
    status: Option<SharedString>,
    active: bool,
    /// The server the workspace's work runs on (a mapped network drive):
    /// its ports are listed, and opened through a tunnel.
    server: Option<ph_remote::Mapping>,
    _poll: Option<Task<()>>,
}

impl EventEmitter<RunsPanelEvent> for RunsPanel {}

impl RunsPanel {
    /// `folders`: the workspace's folders with their names, the primary
    /// first. Each folder's configurations are found; with several, their
    /// names say the folder.
    pub fn new(
        store: Store,
        project_id: i64,
        folders: Vec<(String, PathBuf)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let custom = store.setting::<Vec<RunConfig>>(&custom_key(project_id));
        let name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Name, e.g. Dev server"));
        let command_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Command, e.g. npm run dev -- --port 3000")
        });
        let mut panel = Self {
            store,
            project_id,
            folders,
            detected: Vec::new(),
            custom,
            android: None,
            ports: Vec::new(),
            adding: false,
            show_other_ports: false,
            name_input,
            command_input,
            status: None,
            active: false,
            server: None,
            _poll: None,
        };
        panel.rescan(cx);
        panel
    }

    /// Ports are polled only while the tab is shown.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        // Mappings can change in Settings; look again each time it shows.
        self.server = self
            .folders
            .first()
            .and_then(|(_, folder)| ph_remote::remote_of(folder))
            .map(|remote| remote.mapping);
        let server = self.server.clone();
        self._poll = active.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    let server = server.clone();
                    let ports = cx
                        .background_spawn(async move { listening(server.as_ref()) })
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            this.ports = ports;
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                    cx.background_executor().timer(PORTS_REFRESH).await;
                }
            })
        });
    }

    fn rescan(&mut self, cx: &mut Context<Self>) {
        let folders = self.folders.clone();
        cx.spawn(async move |this, cx| {
            let (detected, android) = cx
                .background_spawn(async move {
                    let android = folders
                        .iter()
                        .find_map(|(_, folder)| ph_devices::AndroidRun::detect(folder));
                    (detect_all(&folders), android)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.detected = detected;
                this.set_android(android, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn set_android(
        &mut self,
        found: Option<(PathBuf, Vec<ph_devices::android_project::AndroidModule>)>,
        cx: &mut Context<Self>,
    ) {
        let Some((root, modules)) = found else {
            // A scan in the middle of a build (gradlew gone for a moment, a
            // branch switch) does not take the card away from it.
            if !self
                .android
                .as_ref()
                .is_some_and(|android| android.read(cx).is_busy())
            {
                self.android = None;
            }
            return;
        };
        if let Some(android) = &self.android {
            android.update(cx, |android, cx| android.set_modules(modules, cx));
            return;
        }
        let android = cx.new(|cx| ph_devices::AndroidRun::new(root, modules, cx));
        cx.subscribe(
            &android,
            |_, _, event: &ph_devices::AndroidRunEvent, cx| match event {
                ph_devices::AndroidRunEvent::Launched => cx.emit(RunsPanelEvent::ShowDevices),
            },
        )
        .detach();
        self.android = Some(android);
    }

    fn save_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().trim().to_string();
        let command = self.command_input.read(cx).value().trim().to_string();
        if name.is_empty() || command.is_empty() {
            self.status = Some("Give the configuration a name and a command.".into());
            cx.notify();
            return;
        }
        self.custom.retain(|config| config.name != name);
        self.custom.push(RunConfig {
            name,
            command,
            custom: true,
            cwd: None,
        });
        self.persist();
        self.adding = false;
        self.status = None;
        for input in [&self.name_input, &self.command_input] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    fn remove_custom(&mut self, name: &str, cx: &mut Context<Self>) {
        self.custom.retain(|config| config.name != name);
        self.persist();
        cx.notify();
    }

    fn persist(&self) {
        if let Err(err) = self
            .store
            .set_setting(&custom_key(self.project_id), &self.custom)
        {
            tracing::warn!("saving run configurations failed: {err:#}");
        }
    }

    fn stop_port(&mut self, port: &Port, cx: &mut Context<Self>) {
        let (pid, name) = (port.pid, port.process.clone());
        let server = self.server.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let server = server.clone();
                    async move {
                        match &server {
                            Some(server) => ports::stop_on(server, pid),
                            None => ports::stop(pid),
                        }
                    }
                })
                .await;
            // Fresh: the port just stopped must not show from the cache.
            let ports = cx
                .background_spawn(async move {
                    match &server {
                        Some(server) => ports::listening_on(server),
                        None => ports::listening_within(std::time::Duration::ZERO),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status = Some(match result {
                    Ok(()) => format!("Stopped {name} ({pid})").into(),
                    Err(err) => format!("{err:#}").into(),
                });
                this.ports = ports;
                cx.notify();
            });
        })
        .detach();
    }

    fn render_config(&self, ix: usize, config: &RunConfig, cx: &mut Context<Self>) -> AnyElement {
        let (name, command, cwd) = (
            config.name.clone(),
            config.command.clone(),
            config.cwd.clone(),
        );
        let remove = config.custom.then(|| name.clone());
        h_flex()
            .id(("run-config", ix))
            .group("run-config")
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(cx.theme().radius)
            .hover(|style| style.bg(cx.theme().list_hover))
            .child(
                Button::new(("run", ix))
                    .ghost()
                    .small()
                    .icon(Icons::Play)
                    .tooltip("Run in a terminal")
                    .accessibility_label(format!("Run {name}"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(RunsPanelEvent::Run {
                            name: name.clone(),
                            command: command.clone(),
                            cwd: cwd.clone(),
                        })
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(config.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().muted_foreground)
                            .child(config.command.clone()),
                    ),
            )
            .when_some(remove, |this, name| {
                this.child(
                    div()
                        .invisible()
                        .group_hover("run-config", |style| style.visible())
                        .child(
                            Button::new(("remove-run", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Delete)
                                .tooltip("Remove")
                                .accessibility_label("Remove configuration")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.remove_custom(&name, cx)
                                    }),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    /// Opens `port` in the browser pane (`copy`: copies its address): on a
    /// server, through a tunnel to a port here.
    fn reach_port(&mut self, port: &Port, copy: bool, cx: &mut Context<Self>) {
        let Some(server) = self.server.clone() else {
            let url = port.url();
            if copy {
                cx.write_to_clipboard(ClipboardItem::new_string(url));
            } else {
                cx.emit(RunsPanelEvent::OpenUrl(url));
            }
            return;
        };
        let (number, address) = (port.port, port.address.clone());
        self.status = Some(format!("Connecting to port {number} on {}…", server.host).into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let host = server.host.clone();
            let local = cx
                .background_spawn(async move { crate::forward::open(&server, number, &address) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match local {
                    Ok(local) => {
                        // The tunnel listens on 127.0.0.1 only: `localhost`
                        // could be ::1, and another program there.
                        let url = format!("http://127.0.0.1:{local}");
                        this.status = Some(
                            format!("Port {number} on {host} is at 127.0.0.1:{local} here.").into(),
                        );
                        if copy {
                            cx.write_to_clipboard(ClipboardItem::new_string(url));
                        } else {
                            cx.emit(RunsPanelEvent::OpenUrl(url));
                        }
                    }
                    Err(err) => this.status = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_port(&self, ix: usize, port: &Port, cx: &mut Context<Self>) -> AnyElement {
        let (open_port, copy_port) = (port.clone(), port.clone());
        // `ss` gives no process for other users' ports: nothing to stop.
        let can_stop = port.pid != 0;
        let stop = port.clone();
        let ours = self
            .folders
            .iter()
            .any(|(_, folder)| port.belongs_to(folder));
        h_flex()
            .id(("port", ix))
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(cx.theme().radius)
            .hover(|style| style.bg(cx.theme().list_hover))
            .child(
                div()
                    .w_16()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(format!(":{}", port.port)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .truncate()
                            .child(format!("{} · {}", port.process, port.pid)),
                    )
                    .when(ours, |this| {
                        this.child(
                            div()
                                .px_1p5()
                                .rounded_full()
                                .bg(cx.theme().info.opacity(0.14))
                                .text_color(cx.theme().info)
                                .child("this project"),
                        )
                    }),
            )
            .child(
                Button::new(("open-port", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icons::Globe)
                    .tooltip("Open in the browser pane")
                    .accessibility_label(format!("Open port {}", port.port))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.reach_port(&open_port, false, cx)),
                    ),
            )
            .child(
                Button::new(("copy-port", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Copy)
                    .tooltip("Copy the address")
                    .accessibility_label("Copy the address")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.reach_port(&copy_port, true, cx)),
                    ),
            )
            .when(can_stop, |this| {
                this.child(
                    Button::new(("stop-port", ix))
                        .ghost()
                        .xsmall()
                        .icon(Icons::SquareStop)
                        .tooltip(format!("Stop {} (pid {})", port.process, port.pid))
                        .accessibility_label("Stop the process")
                        .on_click(cx.listener(move |this, _, _, cx| this.stop_port(&stop, cx))),
                )
            })
            .into_any_element()
    }
}

/// Listening ports here (as seen in the last two seconds, shared with the
/// status bar), or on `server`.
fn listening(server: Option<&ph_remote::Mapping>) -> Vec<Port> {
    match server {
        Some(server) => ports::listening_on(server),
        None => ports::listening_within(std::time::Duration::from_secs(2)),
    }
}

fn custom_key(project_id: i64) -> String {
    format!("runs.{project_id}")
}

fn section(title: &'static str, count: Option<usize>, cx: &gpui_kit::App) -> gpui_kit::Div {
    h_flex()
        .px_2()
        .pt_3()
        .pb_1()
        .gap_1p5()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(title)
        .children(count.map(|count| div().child(count.to_string())))
        .child(div().flex_1())
}

impl Render for RunsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let configs: Vec<RunConfig> = self.custom.iter().chain(&self.detected).cloned().collect();
        // This project's servers first; everything else behind a toggle.
        let (ours, others): (Vec<Port>, Vec<Port>) = self.ports.iter().cloned().partition(|port| {
            self.folders
                .iter()
                .any(|(_, folder)| port.belongs_to(folder))
        });
        let other_count = others.len();
        let ports: Vec<Port> = ours
            .into_iter()
            .chain(others.into_iter().filter(|_| self.show_other_ports))
            .collect();
        v_flex()
            .id("runs-panel")
            .size_full()
            .overflow_y_scroll()
            .px_2()
            .pb_3()
            .when_some(self.android.clone(), |this, android| {
                this.child(div().pt_2().child(android))
            })
            .child(
                section("Run configurations", None, cx)
                    .child(
                        Button::new("rescan-runs")
                            .ghost()
                            .xsmall()
                            .icon(Icons::RotateCw)
                            .tooltip("Detect again")
                            .accessibility_label("Detect again")
                            .on_click(cx.listener(|this, _, _, cx| this.rescan(cx))),
                    )
                    .child(
                        Button::new("add-run")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .label("Add")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.adding = !this.adding;
                                if this.adding {
                                    this.name_input
                                        .update(cx, |input, cx| input.focus(window, cx));
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(self.adding, |this| {
                this.child(
                    v_flex()
                        .gap_1p5()
                        .p_2()
                        .mb_1()
                        .rounded(cx.theme().radius_lg)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(Input::new(&self.name_input).small())
                        .child(Input::new(&self.command_input).small())
                        .child(
                            h_flex()
                                .gap_1()
                                .justify_end()
                                .child(
                                    Button::new("cancel-run")
                                        .ghost()
                                        .small()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.adding = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("save-run")
                                        .primary()
                                        .small()
                                        .label("Save")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save_custom(window, cx)
                                        })),
                                ),
                        ),
                )
            })
            .when(configs.is_empty(), |this| {
                this.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Nothing detected. Add the commands you run often."),
                )
            })
            .children(
                configs
                    .iter()
                    .enumerate()
                    .map(|(ix, config)| self.render_config(ix, config, cx)),
            )
            .child(
                section(
                    if self.server.is_some() {
                        "Ports on the server"
                    } else {
                        "Ports"
                    },
                    None,
                    cx,
                )
                .when(other_count > 0, |this| {
                    this.child(
                        Button::new("other-ports")
                            .ghost()
                            .xsmall()
                            .label(if self.show_other_ports {
                                "Only this project".to_string()
                            } else {
                                format!("Show {other_count} more")
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_other_ports = !this.show_other_ports;
                                cx.notify();
                            })),
                    )
                }),
            )
            .when(ports.is_empty(), |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(Icons::Network).small())
                        .child("No server from this project is listening."),
                )
            })
            .children(
                ports
                    .iter()
                    .enumerate()
                    .map(|(ix, port)| self.render_port(ix, port, cx)),
            )
            .children(self.status.clone().map(|status| {
                div()
                    .px_2()
                    .pt_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status)
            }))
    }
}

/// Every folder's configurations; with several folders, each named after
/// its folder and run in it.
fn detect_all(folders: &[(String, PathBuf)]) -> Vec<RunConfig> {
    let several = folders.len() > 1;
    folders
        .iter()
        .flat_map(|(name, path)| {
            detect(path).into_iter().map(move |mut config| {
                if several {
                    config.name = format!("{name}: {}", config.name);
                    config.cwd = Some(path.clone());
                }
                config
            })
        })
        .collect()
}
