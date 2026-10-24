//! Settings → MCP & skills: the MCP servers PilotHouse gives every agent,
//! the ones agents already have (to copy over), and the project's and the
//! user's skills.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div,
};
use ph_store::Store;
use ph_ui::Icons;

use crate::mcp_config::{self, FoundServer, McpServerConfig};
use crate::skills::{self, Scope, Skill};

/// Opens a file for the person (in its project, or on its own).
pub type OpenFile = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

pub struct ToolsView {
    store: Store,
    project: Option<PathBuf>,
    open_file: OpenFile,
    servers: Vec<McpServerConfig>,
    found: Vec<FoundServer>,
    skills: Vec<Skill>,
    name: Entity<InputState>,
    command: Entity<InputState>,
    env: Entity<TextareaState>,
    skill_name: Entity<InputState>,
    error: Option<SharedString>,
}

impl ToolsView {
    pub fn new(
        store: Store,
        project: Option<PathBuf>,
        open_file: OpenFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let mut view = Self {
            store,
            project,
            open_file,
            servers: Vec::new(),
            found: Vec::new(),
            skills: Vec::new(),
            name: input("Name, e.g. memory", window, cx),
            command: input(
                "Command, e.g. npx -y @modelcontextprotocol/server-memory",
                window,
                cx,
            ),
            env: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Environment, one KEY=value per line (optional)")
                    .rows(2)
            }),
            skill_name: input("New skill's name", window, cx),
            error: None,
        };
        view.reload();
        view
    }

    fn reload(&mut self) {
        self.servers = mcp_config::load(&self.store);
        let home = dirs::home_dir().unwrap_or_default();
        let project = self.project.clone().unwrap_or_default();
        self.found = mcp_config::discover(&home, &project);
        self.skills = skills::discover(&home, &project);
    }

    fn save(&mut self, servers: Vec<McpServerConfig>, cx: &mut Context<Self>) {
        self.error = mcp_config::save(&self.store, &servers)
            .err()
            .map(|err| format!("{err:#}").into());
        self.servers = servers;
        cx.notify();
    }

    fn add(
        &mut self,
        server: McpServerConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Err(err) = mcp_config::validate(&server, &self.servers) {
            self.error = Some(err.to_string().into());
            cx.notify();
            return false;
        }
        let mut servers = self.servers.clone();
        servers.push(server);
        self.save(servers, cx);
        let _ = window;
        true
    }

    fn add_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let words = mcp_config::split_command(&self.command.read(cx).value());
        let Some((command, args)) = words.split_first() else {
            self.error = Some("A server needs a command.".into());
            cx.notify();
            return;
        };
        let server = McpServerConfig {
            name: self.name.read(cx).value().trim().to_string(),
            command: command.clone(),
            args: args.to_vec(),
            env: mcp_config::parse_env(&self.env.read(cx).value()),
            enabled: true,
        };
        if self.add(server, window, cx) {
            self.name
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.command
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.env
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn create_skill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let name = self.skill_name.read(cx).value().to_string();
        match skills::create(&project, &name, "") {
            Ok(path) => {
                self.error = None;
                self.skill_name
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.reload();
                (self.open_file)(path, window, cx);
            }
            Err(err) => self.error = Some(err.to_string().into()),
        }
        cx.notify();
    }

    fn heading(text: &'static str, note: &'static str, cx: &App) -> impl IntoElement {
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(text),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(note),
            )
    }

    fn row(cx: &App) -> gpui_kit::Div {
        h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
    }
}

impl Render for ToolsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let servers = self.servers.iter().enumerate().map(|(ix, server)| {
            let env = if server.env.is_empty() {
                String::new()
            } else {
                format!(
                    " · {} environment variable{}",
                    server.env.len(),
                    if server.env.len() == 1 { "" } else { "s" }
                )
            };
            Self::row(cx)
                .child(Icon::new(Icons::Plug).small().text_color(cx.theme().info))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().child(server.name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(format!("{}{env}", server.command_line())),
                        ),
                )
                .child(
                    Switch::new(("mcp-enabled", ix))
                        .checked(server.enabled)
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            let mut servers = this.servers.clone();
                            if let Some(server) = servers.get_mut(ix) {
                                server.enabled = *on;
                            }
                            this.save(servers, cx);
                        })),
                )
                .child(
                    Button::new(("mcp-remove", ix))
                        .xsmall()
                        .ghost()
                        .label("Remove")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut servers = this.servers.clone();
                            if ix < servers.len() {
                                servers.remove(ix);
                            }
                            this.save(servers, cx);
                        })),
                )
        });
        let found = self.found.iter().enumerate().map(|(ix, found)| {
            let taken = self
                .servers
                .iter()
                .any(|server| server.name.eq_ignore_ascii_case(&found.server.name));
            let server = found.server.clone();
            Self::row(cx)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .child(format!("{} · {}", found.server.name, found.agent)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(found.server.command_line()),
                        ),
                )
                .child(
                    Button::new(("mcp-import", ix))
                        .xsmall()
                        .ghost()
                        .label(if taken {
                            "Added"
                        } else {
                            "Use for every agent"
                        })
                        .disabled(taken)
                        .tooltip("Copies it, with its environment, into PilotHouse's list")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.add(server.clone(), window, cx);
                        })),
                )
        });
        let skills = self.skills.iter().enumerate().map(|(ix, skill)| {
            let path = skill.path.clone();
            let where_ = match skill.scope {
                Scope::Project => "this project",
                Scope::Personal => "every project",
            };
            Self::row(cx)
                .child(
                    Icon::new(Icons::Sparkles)
                        .small()
                        .text_color(cx.theme().info),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .child(format!("{} · {}, {where_}", skill.name, skill.agent)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(skill.description.clone()),
                        ),
                )
                .child(
                    Button::new(("skill-open", ix))
                        .xsmall()
                        .ghost()
                        .label("Open")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            (this.open_file)(path.clone(), window, cx)
                        })),
                )
        });

        v_flex()
            .w_full()
            .gap_3()
            .child(Self::heading(
                "MCP servers for every agent",
                "PilotHouse starts these for each chat, along with its own (board, browser, devices). Local servers only.",
                cx,
            ))
            .children(servers)
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .child(Input::new(&self.name).small())
                    .child(Input::new(&self.command).small())
                    .child(Textarea::new(&self.env).small())
                    .child(
                        h_flex().justify_end().child(
                            Button::new("mcp-add")
                                .small()
                                .primary()
                                .label("Add server")
                                .on_click(cx.listener(|this, _, window, cx| this.add_typed(window, cx))),
                        ),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
            .when(!self.found.is_empty(), |this| {
                this.child(Self::heading(
                    "In your agents' own settings",
                    "Each agent starts these itself. Copy one here to give it to every agent.",
                    cx,
                ))
                .children(found)
            })
            .child(Self::heading(
                "Skills",
                "Instructions agents load when a task needs them (SKILL.md files for Claude Code and Codex).",
                cx,
            ))
            .children(skills)
            .when(self.skills.is_empty(), |this| {
                this.child(div().text_sm().text_color(muted).child("No skills yet."))
            })
            .when(self.project.is_some(), |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.skill_name).small()))
                        .child(
                            Button::new("skill-create")
                                .small()
                                .label("New project skill")
                                .on_click(cx.listener(|this, _, window, cx| this.create_skill(window, cx))),
                        ),
                )
            })
    }
}
