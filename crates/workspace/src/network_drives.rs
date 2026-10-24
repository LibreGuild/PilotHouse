//! Settings → Network drives: folders on this computer that are a server's
//! share, whose Git, terminals and run configurations work on that server
//! over SSH (see [`ph_remote`]). Files are still read and saved through the
//! drive, and agents run here.
//!
//! Adding a drive checks it step by step: the server's key (its fingerprint
//! is shown to trust), signing in (with a terminal here to make and install
//! an SSH key when none works), and that the folder here and on the server
//! are one folder. Only then is it saved and used.

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task, Window, div, px,
};
use ph_remote::Mapping;
use ph_remote::check::{self, Access, HostKey};
use ph_remote::drives::NetworkDrive;
use ph_store::Store;
use ph_terminal::{Terminal, TerminalEvent, TerminalView};
use ph_ui::Icons;

use crate::AppState;

/// The setting holding the drives (`Vec<Mapping>`).
const KEY: &str = "network_drives";

/// Loads the drives and PilotHouse's SSH folder at startup, and closes
/// shared connections and tunnels at quit.
pub fn install(cx: &mut App) {
    ph_remote::configure(
        ph_store::paths::data_dir().join("ssh"),
        ph_store::paths::socket_dir("ssh"),
    );
    ph_remote::set_mappings(AppState::store(cx).setting(KEY));
    cx.on_app_quit(|_| {
        ph_runs::forward::close_all();
        ph_remote::close_shared();
        async {}
    })
    .detach();
}

/// Saves `mappings` and starts using them: open workspaces refresh Git.
fn save(store: &Store, mappings: Vec<Mapping>, cx: &mut App) -> anyhow::Result<()> {
    store.set_setting(KEY, &mappings)?;
    ph_remote::set_mappings(mappings);
    for workspace in crate::windows::all(cx) {
        workspace.update(cx, |workspace, cx| workspace.refresh_git(cx));
    }
    Ok(())
}

/// Opens the drives on their own, `drive` (a workspace's) filled in to add.
pub fn open(drive: Option<NetworkDrive>, window: &mut Window, cx: &mut App) {
    let store = AppState::store(cx).clone();
    let view = cx.new(|cx| {
        let mut view = NetworkDrivesView::new(store, window, cx);
        if let Some(drive) = drive {
            view.fill(&drive, window, cx);
        }
        view
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Network drives")
            .w(px(680.))
            .child(view.clone())
    });
}

/// Where adding (or changing) a drive is.
enum Step {
    Idle,
    Working(SharedString),
    /// The server's key is new: show its fingerprints.
    Trust {
        hostname: String,
        keys: Vec<HostKey>,
    },
    /// The server's key is not the one trusted before.
    Changed {
        mapping: Mapping,
        again: bool,
    },
    /// No key of the user's is accepted. `key`: their public key file and
    /// text, if they have one.
    NeedKey {
        key: Option<(PathBuf, String)>,
    },
    Failed(SharedString),
    Saved(SharedString),
}

pub struct NetworkDrivesView {
    store: Store,
    mappings: Vec<Mapping>,
    local: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    remote: Entity<InputState>,
    /// The share's name, to ask Samba where it is on the server.
    share: String,
    /// The folder of the drive being changed ("Change"), which saving
    /// replaces even if the form now names another.
    editing: Option<String>,
    step: Step,
    /// A terminal here for making, installing or unlocking a key.
    terminal: Option<(Entity<TerminalView>, Subscription)>,
    /// The key was installed on the server in this session: if it is still
    /// refused, it needs its passphrase (the agent).
    installed: bool,
    _task: Option<Task<()>>,
}

impl NetworkDrivesView {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        Self {
            mappings: store.setting(KEY),
            store,
            local: input(
                r"The drive or folder here, e.g. Z:\ or \\server\share",
                window,
                cx,
            ),
            host: input(
                "Server: user@server, or a host from ~/.ssh/config",
                window,
                cx,
            ),
            port: input("Port (22)", window, cx),
            remote: input("The same folder on the server, e.g. /srv/code", window, cx),
            share: String::new(),
            editing: None,
            step: Step::Idle,
            terminal: None,
            installed: false,
            _task: None,
        }
    }

    /// Fills the form from a detected network drive.
    fn fill(&mut self, drive: &NetworkDrive, window: &mut Window, cx: &mut Context<Self>) {
        let root = PathBuf::from(&drive.root);
        let known = self.mappings.iter().find(|mapping| {
            mapping.local == drive.root
                || ph_remote::mapping::remote_of_in(std::slice::from_ref(mapping), &root).is_some()
        });
        self.editing = known.map(|mapping| mapping.local.clone());
        let (local, host, port, remote) = match known {
            Some(mapping) => (
                mapping.local.clone(),
                mapping.host.clone(),
                mapping
                    .port
                    .map(|port| port.to_string())
                    .unwrap_or_default(),
                mapping.remote.clone(),
            ),
            None => (
                drive.root.clone(),
                drive.server.clone(),
                String::new(),
                String::new(),
            ),
        };
        self.share = drive.share.clone();
        for (input, value) in [
            (&self.local, local),
            (&self.host, host),
            (&self.port, port),
            (&self.remote, remote),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }

    fn edit(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mapping) = self.mappings.get(ix).cloned() else {
            return;
        };
        for (input, value) in [
            (&self.local, mapping.local.clone()),
            (&self.host, mapping.host.clone()),
            (
                &self.port,
                mapping
                    .port
                    .map(|port| port.to_string())
                    .unwrap_or_default(),
            ),
            (&self.remote, mapping.remote.clone()),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.editing = Some(mapping.local);
        self.step = Step::Idle;
        cx.notify();
    }

    fn remove(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.mappings.len() {
            self.mappings.remove(ix);
        }
        if let Err(err) = save(&self.store, self.mappings.clone(), cx) {
            self.step = Step::Failed(format!("{err:#}").into());
        }
        cx.notify();
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let drive = cx
                .background_spawn({
                    let path = path.clone();
                    async move { ph_remote::drives::network_drive_of(&path) }
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| {
                let text = path.to_string_lossy().into_owned();
                this.local
                    .update(cx, |input, cx| input.set_value(text, window, cx));
                if let Some(drive) = drive {
                    this.share = drive.share;
                    if this.host.read(cx).value().trim().is_empty() {
                        this.host
                            .update(cx, |input, cx| input.set_value(drive.server, window, cx));
                    }
                }
            });
        })
        .detach();
    }

    /// The form as a mapping.
    fn mapping(&self, cx: &App) -> Result<Mapping, &'static str> {
        let port = self.port.read(cx).value().trim().to_string();
        let port = if port.is_empty() {
            None
        } else {
            Some(
                port.parse::<u16>()
                    .ok()
                    .filter(|port| *port > 0)
                    .ok_or("The port must be a number between 1 and 65535.")?,
            )
        };
        Ok(Mapping {
            local: self.local.read(cx).value().trim().to_string(),
            resolved: String::new(),
            host: self.host.read(cx).value().trim().to_string(),
            port,
            remote: self
                .remote
                .read(cx)
                .value()
                .trim()
                .trim_end_matches('/')
                .to_string(),
        })
    }

    fn set_step(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        cx.notify();
    }

    /// Checks the form's drive from the start, and saves it when all is well.
    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut mapping = match self.mapping(cx) {
            Ok(mapping) => mapping,
            Err(problem) => return self.set_step(Step::Failed(problem.into()), cx),
        };
        if let Some(problem) = mapping.connect_problem() {
            return self.set_step(Step::Failed(problem.into()), cx);
        }
        self.set_step(
            Step::Working(format!("Connecting to {}…", mapping.host).into()),
            cx,
        );
        let share = self.share.clone();
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let probe = mapping.clone();
            let access = cx
                .background_spawn(async move { check::access(&probe) })
                .await;
            let next = match access {
                Access::Ready => None,
                Access::UnknownHost => {
                    let probe = mapping.clone();
                    let keys = cx
                        .background_spawn(async move {
                            let hostname = check::resolve(&probe)
                                .map(|resolved| resolved.hostname)
                                .unwrap_or_else(|_| probe.host.clone());
                            anyhow::Ok((hostname, check::host_keys(&probe)?))
                        })
                        .await;
                    Some(match keys {
                        Ok((hostname, keys)) => Step::Trust { hostname, keys },
                        Err(err) => Step::Failed(format!("{err:#}").into()),
                    })
                }
                Access::ChangedHost => Some(Step::Changed {
                    mapping: mapping.clone(),
                    again: false,
                }),
                Access::NoKey => {
                    let key = cx.background_spawn(async { check::public_key() }).await;
                    Some(Step::NeedKey { key })
                }
                Access::Unreachable(why) => Some(Step::Failed(
                    format!("Cannot reach {} over SSH: {why}", mapping.host).into(),
                )),
            };
            if let Some(step) = next {
                _ = this.update(cx, |this, cx| this.set_step(step, cx));
                return;
            }
            // Signed in. Where is the folder on the server? Samba knows by
            // the share's name: as detected, else the drive's, else the
            // folder's own (a share is often named like its folder).
            let mut share = share;
            if mapping.remote.is_empty() && share.is_empty() {
                let local = PathBuf::from(&mapping.local);
                share = cx
                    .background_spawn(async move {
                        ph_remote::drives::network_drive_of(&local)
                            .map(|drive| drive.share)
                            .or_else(|| {
                                local
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                            })
                            .unwrap_or_default()
                    })
                    .await;
            }
            if mapping.remote.is_empty() && !share.is_empty() {
                _ = this.update(cx, |this, cx| {
                    this.set_step(
                        Step::Working("Asking the server where the share is…".into()),
                        cx,
                    )
                });
                let probe = mapping.clone();
                if let Some(path) = cx
                    .background_spawn(async move { check::share_path(&probe, &share) })
                    .await
                {
                    mapping.remote = path.clone();
                    _ = this.update_in(cx, |this, window, cx| {
                        this.remote
                            .update(cx, |input, cx| input.set_value(path, window, cx));
                    });
                }
            }
            if let Some(problem) = mapping.problem() {
                _ = this.update(cx, |this, cx| {
                    this.set_step(Step::Failed(problem.into()), cx)
                });
                return;
            }
            _ = this.update(cx, |this, cx| {
                this.set_step(Step::Working("Checking it is the same folder…".into()), cx)
            });
            let probe = mapping.clone();
            let checked = cx
                .background_spawn(async move { check::same_folder(&probe) })
                .await;
            _ = this.update(cx, |this, cx| match checked {
                Ok(checked) => {
                    mapping.resolved = checked.resolved;
                    let git = checked.git.unwrap_or_else(|| {
                        "no Git on the server: Git features will not work there".into()
                    });
                    let replaced = this.editing.take().unwrap_or_else(|| mapping.local.clone());
                    this.mappings
                        .retain(|known| known.local != mapping.local && known.local != replaced);
                    this.mappings.push(mapping.clone());
                    let step = match save(&this.store, this.mappings.clone(), cx) {
                        Ok(()) => Step::Saved(
                            format!(
                                "Saved. Git, terminals and runs in {} now work on {} ({git}).",
                                mapping.local,
                                mapping.destination()
                            )
                            .into(),
                        ),
                        Err(err) => Step::Failed(format!("{err:#}").into()),
                    };
                    this.set_step(step, cx);
                }
                Err(err) => this.set_step(Step::Failed(format!("{err:#}").into()), cx),
            });
        }));
    }

    fn trust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Step::Trust { keys, .. } = &self.step else {
            return;
        };
        let keys = keys.clone();
        self.set_step(Step::Working("Trusting the server…".into()), cx);
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { check::trust(&keys) })
                .await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => this.check(window, cx),
                Err(err) => this.set_step(Step::Failed(format!("{err:#}").into()), cx),
            });
        }));
    }

    /// Forgets the server's old key (it was reinstalled), then checks again.
    /// A key still differing after that is in the user's own `known_hosts`.
    fn forget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Step::Changed { mapping, .. } = &self.step else {
            return;
        };
        let mapping = mapping.clone();
        self.set_step(Step::Working("Forgetting the old key…".into()), cx);
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let probe = mapping.clone();
            let result = cx
                .background_spawn(async move {
                    check::forget(&probe)?;
                    anyhow::Ok(check::access(&probe))
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Access::ChangedHost) => this.set_step(
                    Step::Changed {
                        mapping,
                        again: true,
                    },
                    cx,
                ),
                Ok(_) => this.check(window, cx),
                Err(err) => this.set_step(Step::Failed(format!("{err:#}").into()), cx),
            });
        }));
    }

    /// Runs `program` in a terminal here; when it ends, checks again.
    /// `installs`: it installs the key on the server.
    fn run_in_terminal(
        &mut self,
        program: (String, Vec<String>),
        installs: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let shell = match Terminal::start_program(&program.0, program.1, home) {
            Ok(shell) => shell,
            Err(err) => {
                return self.set_step(
                    Step::Failed(format!("Could not start {}: {err:#}", program.0).into()),
                    cx,
                );
            }
        };
        let terminal = cx.new(|cx| Terminal::new(shell, cx));
        let view = cx.new(|cx| TerminalView::new(terminal.clone(), cx));
        let subscription = cx.subscribe_in(&terminal, window, move |this, _, event, window, cx| {
            if let TerminalEvent::Exited = event {
                // Installed only if that worked (a wrong password exits 255).
                let succeeded = this
                    .terminal
                    .as_ref()
                    .and_then(|(view, _)| view.read(cx).terminal().read(cx).exit_code())
                    == Some(0);
                this.terminal = None;
                this.installed |= installs && succeeded;
                this.check(window, cx);
            }
        });
        view.read(cx).focus_handle(cx).focus(window, cx);
        self.terminal = Some((view, subscription));
        cx.notify();
    }

    fn make_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match check::keygen_args() {
            Some(program) => self.run_in_terminal(program, false, window, cx),
            None => self.set_step(Step::Failed("Cannot find your home folder.".into()), cx),
        }
    }

    fn install_key(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let mapping = match self.mapping(cx) {
            Ok(mapping) => mapping,
            Err(problem) => return self.set_step(Step::Failed(problem.into()), cx),
        };
        match check::install_key_args(&mapping, &key) {
            Some(program) => self.run_in_terminal(program, true, window, cx),
            None => self.set_step(
                Step::Failed("Your public key file does not look like one.".into()),
                cx,
            ),
        }
    }

    fn add_to_agent(&mut self, public: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.run_in_terminal(check::agent_add_args(&public), false, window, cx);
    }

    fn render_step(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let theme = cx.theme();
        let note = |text: SharedString, color| {
            div()
                .text_xs()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        Some(match &self.step {
            Step::Idle => return None,
            Step::Working(text) => note(text.clone(), theme.muted_foreground),
            Step::Failed(text) => note(text.clone(), theme.danger),
            Step::Saved(text) => note(text.clone(), theme.success),
            Step::Changed { mapping, again } => {
                let text = if *again {
                    format!(
                        "The old key is in your own SSH settings, not PilotHouse's. If the server really was reinstalled, remove it there (in a terminal: {}) and check again.",
                        check::forget_command(mapping)
                    )
                } else {
                    format!(
                        "{}'s key is not the one trusted before. This can mean someone is intercepting the connection. Ask whoever runs the server; only if it was reinstalled, forget the old key and check the new one.",
                        mapping.host
                    )
                };
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.danger.opacity(0.6))
                    .child(div().text_sm().child(text))
                    .when(!*again, |this| {
                        this.child(
                            h_flex().child(
                                Button::new("forget-key")
                                    .small()
                                    .danger()
                                    .label("Forget the old key")
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.forget(window, cx)),
                                    ),
                            ),
                        )
                    })
                    .into_any_element()
            }
            Step::Trust { hostname, keys } => v_flex()
                .gap_2()
                .p_3()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.warning.opacity(0.5))
                .child(div().text_sm().child(format!(
                    "{hostname} is new to this computer. Trust it if its fingerprint is the one its admin gives you:"
                )))
                .children(keys.iter().map(|key| {
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .child(div().w(px(150.)).truncate().child(key.kind.clone()))
                        .child(key.fingerprint.clone())
                }))
                .child(
                    h_flex().child(
                        Button::new("trust-server")
                            .small()
                            .primary()
                            .label("Trust this server")
                            .on_click(cx.listener(|this, _, window, cx| this.trust(window, cx))),
                    ),
                )
                .into_any_element(),
            Step::NeedKey { key } => {
                let (text, action) = match key.clone() {
                    Some((public, _)) if self.installed => (
                        "Your key is on the server now, but it has a passphrase, which PilotHouse's background commands cannot ask for. Add it to your SSH agent: you type the passphrase once, in the terminal below.",
                        Button::new("agent-key")
                            .small()
                            .primary()
                            .label("Add my key to the SSH agent")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_to_agent(public.clone(), window, cx)
                            })),
                    ),
                    Some((_, key)) => (
                        "The server did not accept your SSH key. Install it there: you sign in with your password once, in the terminal below.",
                        Button::new("install-key")
                            .small()
                            .primary()
                            .label("Install my key on the server")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.install_key(key.clone(), window, cx)
                            })),
                    ),
                    None => (
                        "You have no SSH key yet. Make one (a passphrase is optional), then install it on the server.",
                        Button::new("make-key")
                            .small()
                            .primary()
                            .label("Make a key")
                            .on_click(cx.listener(|this, _, window, cx| this.make_key(window, cx))),
                    ),
                };
                let windows_agent = (cfg!(windows) && self.installed).then(|| {
                    div().text_xs().text_color(theme.muted_foreground).child(
                        "If ssh-add says it cannot reach the agent, start the OpenSSH Authentication Agent service (in PowerShell as administrator: Set-Service ssh-agent -StartupType Automatic; Start-Service ssh-agent).",
                    )
                });
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .child(div().text_sm().child(text))
                    .child(h_flex().child(action))
                    .children(windows_agent)
                    .into_any_element()
            }
        })
    }
}

impl Render for NetworkDrivesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = matches!(self.step, Step::Working(_)) || self.terminal.is_some();
        let theme = cx.theme().clone();
        v_flex()
            .w_full()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("A folder here that is a server's share (a mapped drive) can have its Git, terminals and run configurations work on that server, over SSH. Files are still read and saved through the drive; agents run on this computer."),
            )
            .child(
                v_flex()
                    .gap_1()
                    .children(self.mappings.iter().enumerate().map(|(ix, mapping)| {
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .child(Icon::new(Icons::HardDrive).small().text_color(theme.info))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().truncate().child(mapping.local.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .truncate()
                                            .child(format!(
                                                "{}:{}",
                                                mapping.destination(),
                                                mapping.remote
                                            )),
                                    ),
                            )
                            .child(
                                Button::new(("edit-drive", ix))
                                    .small()
                                    .ghost()
                                    .label("Change")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.edit(ix, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("remove-drive", ix))
                                    .small()
                                    .ghost()
                                    .label("Remove")
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove(ix, cx))),
                            )
                    }))
                    .when(self.mappings.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No network drives yet."),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(theme.radius)
                    .bg(theme.secondary)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child("Add or change a drive"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.local).small()))
                            .child(
                                Button::new("browse-drive")
                                    .small()
                                    .ghost()
                                    .icon(Icons::FolderOpen)
                                    .label("Choose…")
                                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.host).small()))
                            .child(div().w(px(110.)).child(Input::new(&self.port).small())),
                    )
                    .child(Input::new(&self.remote).small())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Left empty, PilotHouse asks Samba on the server where the share is."),
                            )
                            .child(
                                Button::new("check-drive")
                                    .small()
                                    .primary()
                                    .icon(IconName::Check)
                                    .label("Check and save")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| this.check(window, cx))),
                            ),
                    )
                    .children(self.render_step(cx))
                    .when_some(self.terminal.as_ref(), |this, (view, _)| {
                        this.child(
                            div()
                                .h(px(220.))
                                .rounded(theme.radius)
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme.border)
                                .child(view.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("PilotHouse signs in with your own SSH setup (keys, agent, ~/.ssh/config) and never stores a password. Servers you trust here are kept in PilotHouse's own known_hosts."),
            )
    }
}
