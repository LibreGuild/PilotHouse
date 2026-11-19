//! The Bridge: every task of every workspace, by where it stands. It is the
//! window's home, and answers "what needs me?" before any one workspace is
//! looked at.
//!
//! A task is a chat with an agent (or an agent's CLI in a terminal, or a
//! review being readied). The columns are what the user does about it:
//!
//! - **Needs you**: the agent asks something, or stopped with an error.
//! - **Working**: the agent is at it.
//! - **Ready for review**: a turn finished and nobody marked the task done.
//!   News not looked at yet comes first.
//! - **Done**: marked done, or left alone for a week.
//!
//! It took the place of the lists beside the workspace (which showed one
//! workspace's chats at a time) and of the Agents pop-up (which showed only
//! what was loaded): the same cards, for everything, always in the same
//! place. Reviews of other people's pull requests have their own view; the
//! Bridge shows that some wait, and those an agent is working on.

use std::collections::HashSet;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
};
use ph_agent::{ChatState, ChatSummary};
use ph_store::projects::ProjectId;
use ph_store::sessions::{Session, SessionId};
use ph_ui::{Icons, MenuButton as _};

use crate::actions::{
    AddFolderToWorkspace, AddProject, FinishTask, NewTask, NewWorkspace, OpenInNewWindow,
    RemoveWorkspace, RenameWorkspace,
};
use crate::context_panel::ContextTab;
use crate::workspace::{View, Workspace};

/// A task nobody touched for this long counts as done.
const QUIET_AFTER: i64 = 7 * 24 * 60 * 60 * 1000;
/// Cards a column lists before *Show all*.
const SHOWN: usize = 12;

/// What the Bridge shows, kept while the window lives.
#[derive(Default)]
pub(crate) struct BridgeState {
    /// Only this workspace's tasks.
    pub filter: Option<ProjectId>,
    /// The columns listing every card of theirs.
    pub all: HashSet<&'static str>,
}

/// What a card opens.
#[derive(Clone, PartialEq)]
pub(crate) enum Target {
    Chat(SessionId),
    /// An agent CLI in a terminal (its pane id).
    Terminal(String),
    /// A review about to start, while what it needs is readied (a clone):
    /// of the pull request with this number, or (0) the one beside the
    /// repository's chat.
    Preparing(u64),
}

/// One task on the Bridge.
pub(crate) struct Card {
    pub project: ProjectId,
    /// A chat of the Reviews view (a review), shown there.
    pub github: bool,
    pub target: Target,
    pub title: String,
    pub place: String,
    pub color: Option<Hsla>,
    pub updated_at: i64,
    pub summary: ChatSummary,
    /// The stored chat, for its menu.
    session: Option<Session>,
    pinned: bool,
    settled: bool,
    /// Its workspace has folders, so there can be changes to review.
    has_folders: bool,
    /// PilotHouse started it, not the user: a schedule's chat, a UI
    /// test's recording or repair, a review.
    automated: bool,
    /// Where it works (its own folder, else its workspace's first), for
    /// the branch and pull request found there.
    folder: Option<std::path::PathBuf>,
    /// What it did, in numbers, as of its last turn.
    facts: Option<ph_store::sessions::SessionFacts>,
    /// What its agent asks, while it waits for the answer.
    request: Option<gpui_kit::Entity<ph_agent::PermissionRequest>>,
}

/// The Bridge's columns, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Column {
    NeedsYou,
    Working,
    Review,
    Done,
}

impl Column {
    const ALL: [Column; 4] = [
        Column::NeedsYou,
        Column::Working,
        Column::Review,
        Column::Done,
    ];

    fn of(card: &Card, now: i64) -> Self {
        Self::place(
            card.summary.state,
            card.settled,
            card.automated,
            now - card.updated_at > QUIET_AFTER,
        )
    }

    /// Where a chat in `state` stands: `settled` when it was marked done,
    /// `automated` when PilotHouse started it, `quiet` when nobody touched
    /// it for a week.
    pub(crate) fn place(state: ChatState, settled: bool, automated: bool, quiet: bool) -> Self {
        match state {
            ChatState::NeedsYou => Column::NeedsYou,
            ChatState::Working => Column::Working,
            // A turn that failed asks for a look until it is put away (a
            // chat that ends on an error counts as failed whenever it is
            // loaded, however old).
            ChatState::Failed if settled || quiet => Column::Done,
            ChatState::Failed => Column::NeedsYou,
            ChatState::Done => Column::Review,
            // Runs PilotHouse started (a schedule's, a test's) wait for a
            // look only while their news is fresh: they would bury the
            // user's own tasks otherwise. The inbox lists what they did.
            ChatState::Idle if settled || quiet || automated => Column::Done,
            ChatState::Idle => Column::Review,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Column::NeedsYou => "Needs you",
            Column::Working => "Working",
            Column::Review => "Ready for review",
            Column::Done => "Done",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Column::NeedsYou => "needs-you",
            Column::Working => "working",
            Column::Review => "review",
            Column::Done => "done",
        }
    }

    fn empty(self) -> &'static str {
        match self {
            Column::NeedsYou => "No agent waits for you.",
            Column::Working => "No agent is at work.",
            Column::Review => "Nothing to look at.",
            Column::Done => "Tasks you mark done show here.",
        }
    }

    pub(crate) fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Column::NeedsYou => theme.warning,
            Column::Working => theme.info,
            Column::Review => theme.success,
            Column::Done => theme.muted_foreground,
        }
    }
}

/// Every loaded chat of every workspace (and the Chats space) that is live
/// in this run, and the agents in terminals, newest first: for the command
/// palette.
pub(crate) fn live_cards(workspace: &Workspace, cx: &App) -> Vec<Card> {
    cards(workspace, true, cx)
}

/// The tasks: with `live`, only the chats loaded in this run that are not
/// marked done; else every stored chat too.
fn cards(workspace: &Workspace, live: bool, cx: &App) -> Vec<Card> {
    let dark = cx.theme().is_dark();
    let now = jiff::Timestamp::now().as_millisecond();
    let places: Vec<(ProjectId, String, Option<Hsla>, Option<std::path::PathBuf>)> = workspace
        .projects()
        .iter()
        .map(|project| {
            let color = ph_ui::workspace_color(project.shade(ph_ui::WORKSPACE_COLORS.len()), dark);
            (
                project.id,
                project.name.clone(),
                Some(color),
                project.folders().into_iter().next(),
            )
        })
        .collect();
    let mut cards = Vec::new();
    // Reviews: those an agent works on, or is being readied for.
    if let Some(github) = workspace.github_view() {
        for (project, name) in github.read(cx).spaces() {
            let Some(chats) = ph_agent::ChatHub::existing(*project, cx) else {
                continue;
            };
            let place = format!("Reviews · {name}");
            // The agent a review runs with, as Reviews last read it (the
            // store is not read while drawing).
            let reviewer = github
                .read(cx)
                .review_agent()
                .unwrap_or(&ph_agent::agents::AGENTS[0]);
            // A review whose repository is still being cloned is at work
            // already, to whoever asked for it.
            if let Some(preparing) = chats.read(cx).preparing() {
                cards.push(Card {
                    project: *project,
                    github: true,
                    target: Target::Preparing(0),
                    title: preparing.title.to_string(),
                    place: place.clone(),
                    color: None,
                    updated_at: i64::MAX,
                    summary: ChatSummary {
                        state: ChatState::Working,
                        title: preparing.title.clone(),
                        agent: reviewer,
                        line: Some(format!(
                            "{} {}s",
                            preparing.label,
                            preparing.since.elapsed().as_secs()
                        )),
                    },
                    session: None,
                    pinned: false,
                    settled: false,
                    has_folders: false,
                    automated: false,
                    folder: None,
                    facts: None,
                    request: None,
                });
            }
            // Those started from the queue, several at once, each have
            // their own.
            for (number, starting) in github.read(cx).starting_behind(*project) {
                cards.push(Card {
                    project: *project,
                    github: true,
                    target: Target::Preparing(number),
                    title: starting.title.to_string(),
                    place: place.clone(),
                    color: None,
                    updated_at: i64::MAX,
                    summary: ChatSummary {
                        state: ChatState::Working,
                        title: starting.title.clone(),
                        agent: reviewer,
                        line: Some(format!(
                            "{} {}s",
                            starting.label,
                            starting.since.elapsed().as_secs()
                        )),
                    },
                    session: None,
                    pinned: false,
                    settled: false,
                    has_folders: false,
                    automated: false,
                    folder: None,
                    facts: None,
                    request: None,
                });
            }
            for (session, summary) in chats.read(cx).live(cx) {
                let busy = matches!(summary.state, ChatState::Working | ChatState::NeedsYou);
                // Finished reviews are read in Reviews, not here.
                if !busy && (!live || workspace.is_settled(session)) {
                    continue;
                }
                cards.push(Card {
                    project: *project,
                    github: true,
                    target: Target::Chat(session),
                    title: summary.title.to_string(),
                    place: place.clone(),
                    color: None,
                    // Loaded in this run and busy, or just finished: now.
                    updated_at: now,
                    summary,
                    session: None,
                    pinned: false,
                    settled: false,
                    has_folders: false,
                    automated: false,
                    folder: None,
                    facts: None,
                    request: chats.read(cx).pending_request(session, cx),
                });
            }
        }
    }
    for (project, place, color, folder) in places.iter().cloned() {
        let has_folders = folder.is_some();
        let chats = ph_agent::ChatHub::existing(project, cx);
        for stored in workspace.sessions_of(project, cx).iter() {
            if in_terminal(stored, cx) {
                continue;
            }
            let loaded = chats
                .as_ref()
                .and_then(|chats| chats.read(cx).summary(stored.id, cx));
            let settled = workspace.is_settled(stored.id);
            let summary = match loaded {
                Some(summary) => {
                    let busy = matches!(summary.state, ChatState::Working | ChatState::NeedsYou);
                    // Chats marked done leave the live list until they work
                    // again.
                    if live && settled && !busy {
                        continue;
                    }
                    summary
                }
                None if live => continue,
                None => stored_summary(stored),
            };
            cards.push(Card {
                project,
                github: false,
                target: Target::Chat(stored.id),
                title: stored.title.clone(),
                place: place.clone(),
                color,
                updated_at: stored.updated_at,
                summary,
                session: Some(stored.clone()),
                pinned: workspace.is_pinned(stored.id),
                settled,
                has_folders,
                automated: is_automated(stored),
                folder: stored
                    .cwd
                    .as_ref()
                    .map(std::path::PathBuf::from)
                    .or_else(|| folder.clone()),
                facts: stored.facts.clone().filter(|facts| !facts.is_empty()),
                request: chats
                    .as_ref()
                    .and_then(|chats| chats.read(cx).pending_request(stored.id, cx)),
            });
        }
    }
    // Agents in terminals, of the same places.
    for agent in crate::terminal_agents::TerminalAgents::all(cx) {
        let Some((_, place, color, _)) = places.iter().find(|(id, ..)| *id == agent.project) else {
            continue;
        };
        let label = agent
            .terminal
            .upgrade()
            .and_then(|terminal| terminal.read(cx).label().cloned())
            .unwrap_or_else(|| agent.agent.name.into());
        cards.push(Card {
            project: agent.project,
            github: false,
            target: Target::Terminal(agent.pane.clone()),
            title: agent
                .title
                .clone()
                .unwrap_or_else(|| format!("{label} (terminal)")),
            place: format!("{place} · terminal"),
            color: *color,
            updated_at: agent.updated_at,
            summary: ChatSummary {
                state: agent.state,
                title: label,
                agent: agent.agent,
                line: agent.line.clone(),
            },
            session: None,
            pinned: false,
            settled: false,
            has_folders: false,
            automated: false,
            folder: None,
            facts: None,
            request: None,
        });
    }
    cards.sort_by_key(|card| std::cmp::Reverse(card.updated_at));
    cards
}

/// An agent CLI's chat whose terminal is open: that terminal's card.
fn in_terminal(stored: &Session, cx: &App) -> bool {
    stored.kind == "terminal"
        && crate::terminal_agents::TerminalAgents::of_session(stored.id, cx).is_some()
}

/// A chat not loaded in this run, as it was stored: nothing is going on.
fn stored_summary(stored: &Session) -> ChatSummary {
    ChatSummary {
        state: ChatState::Idle,
        title: stored.title.clone().into(),
        agent: ph_agent::agents::find(&stored.agent_id).unwrap_or(&ph_agent::agents::AGENTS[0]),
        line: stored.last_line.clone(),
    }
}

/// A chat among the tabs over a workspace's chat.
pub(crate) struct OpenChat {
    pub session: Session,
    pub summary: ChatSummary,
    pub pinned: bool,
    pub settled: bool,
    /// Where it stands on the Bridge.
    pub column: Column,
}

/// Of a workspace's chats `sessions`, those still open (one needs the user,
/// is being worked on, or waits for a look) and `front` whatever its state,
/// in the order they were started: a tab keeps its place while another
/// works. What the Bridge lists under *Done* is not among them.
pub(crate) fn open_chats(
    workspace: &Workspace,
    project: ProjectId,
    sessions: &[Session],
    front: Option<SessionId>,
    cx: &App,
) -> Vec<OpenChat> {
    let now = jiff::Timestamp::now().as_millisecond();
    let chats = ph_agent::ChatHub::existing(project, cx);
    let mut open: Vec<OpenChat> = sessions
        .iter()
        .filter(|stored| !in_terminal(stored, cx))
        .filter_map(|stored| {
            let summary = chats
                .as_ref()
                .and_then(|chats| chats.read(cx).summary(stored.id, cx))
                .unwrap_or_else(|| stored_summary(stored));
            let settled = workspace.is_settled(stored.id);
            let column = Column::place(
                summary.state,
                settled,
                is_automated(stored),
                now - stored.updated_at > QUIET_AFTER,
            );
            (column != Column::Done || front == Some(stored.id)).then(|| OpenChat {
                session: stored.clone(),
                summary,
                pinned: workspace.is_pinned(stored.id),
                settled,
                column,
            })
        })
        .collect();
    open.sort_by_key(|chat| chat.session.id);
    open
}

/// How many tasks need the user and how many are being worked on, across
/// the window's workspaces and reviews.
pub(crate) fn counts(workspace: &Workspace, cx: &App) -> (usize, usize) {
    let mut counts = (0, 0);
    let github: Vec<ProjectId> = workspace
        .github_view()
        .map(|github| github.read(cx).spaces().iter().map(|(id, _)| *id).collect())
        .unwrap_or_default();
    for project in workspace
        .projects()
        .iter()
        .map(|project| project.id)
        .chain(github)
    {
        if let Some(chats) = ph_agent::ChatHub::existing(project, cx) {
            let activity = chats.read(cx).activity(cx);
            counts.0 += activity.waiting;
            counts.1 += activity.working;
        }
    }
    // Reviews started from the queue, while their repository is cloned.
    if let Some(github) = workspace.github_view() {
        counts.1 += github.read(cx).starting_behind_count();
    }
    for agent in crate::terminal_agents::TerminalAgents::all(cx) {
        match agent.state {
            ChatState::NeedsYou => counts.0 += 1,
            ChatState::Working => counts.1 += 1,
            _ => {}
        }
    }
    counts
}

/// How many finished tasks nobody looked at yet, for the status bar.
pub(crate) fn unseen(workspace: &Workspace) -> usize {
    workspace
        .projects()
        .iter()
        .map(|project| project.id)
        .map(|id| workspace.status(id).unseen)
        .sum()
}

/// The Bridge, filling the window between the column of icons and its
/// right edge.
pub(crate) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let entity = cx.entity().downgrade();
    let state = workspace.bridge();
    let now = jiff::Timestamp::now().as_millisecond();
    let all = cards(workspace, false, cx);
    let filter = state.filter.filter(|id| workspace.has_project(*id));
    let shown: Vec<&Card> = all
        .iter()
        .filter(|card| match filter {
            Some(id) => card.project == id && !card.github,
            None => true,
        })
        .collect();
    // Two columns in a narrow window, one in a very narrow one: four cards
    // side by side need the room.
    let (width, probe) = ph_ui::measure::width_of("bridge-width", window, cx);
    let rem = f32::from(window.rem_size());
    let columns = match width.map(|width| f32::from(width) / rem) {
        Some(rems) if rems < 34. => 1,
        Some(rems) if rems < 62. => 2,
        _ => 4,
    };

    let nothing = workspace.projects().is_empty() && all.is_empty();
    let body = if nothing {
        welcome(window, cx).into_any_element()
    } else {
        div()
            .id("bridge-columns")
            .flex_1()
            .min_h_0()
            .grid()
            .grid_cols(columns)
            .gap_3()
            // Side by side each column scrolls alone; stacked, they scroll
            // as one.
            .when(columns < 4, |this| this.overflow_y_scroll())
            .children(Column::ALL.into_iter().map(|column| {
                let mut cards: Vec<&Card> = shown
                    .iter()
                    .copied()
                    .filter(|card| Column::of(card, now) == column)
                    .collect();
                // What was not looked at yet leads the review column;
                // pinned tasks lead theirs.
                cards.sort_by_key(|card| {
                    (
                        !card.pinned,
                        card.summary.state != ChatState::Done,
                        std::cmp::Reverse(card.updated_at),
                    )
                });
                let reviews = (column == Column::Review && filter.is_none())
                    .then(|| reviews_card(workspace, &entity, cx))
                    .flatten();
                column_view(
                    column,
                    cards,
                    reviews,
                    state.all.contains(column.key()),
                    columns == 4,
                    &entity,
                    cx,
                )
            }))
            .into_any_element()
    };

    // An island of the frame, as a workspace's screen, Reviews and an
    // organisation's view are: the frame is what stays around, and what is
    // in front sits on it as a rounded panel. (The Bridge was drawn
    // straight on the frame: the one view without the panel, its title and
    // first column hard against the column of icons.)
    let panel = v_flex()
        .relative()
        .size_full()
        .px_4()
        .pt_3()
        .pb_4()
        .gap_3()
        .child(probe)
        .child(header(workspace, filter, &entity, cx))
        .child(body);
    div()
        .id("bridge")
        .size_full()
        .pr_2()
        .pb(crate::frame::gap())
        .child(crate::frame::island(window, cx).child(panel))
        .test_support()
        .into_any_element()
}

/// The Bridge's title, the workspaces to narrow it to, and a new task.
fn header(
    workspace: &Workspace,
    filter: Option<ProjectId>,
    entity: &WeakEntity<Workspace>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme();
    let dark = theme.is_dark();
    let chip = |id: SharedString,
                label: SharedString,
                color: Option<Hsla>,
                selected: bool,
                to: Option<ProjectId>| {
        let entity = entity.clone();
        h_flex()
            .id(id)
            .flex_none()
            .h(ph_ui::rems_from_px(26.))
            .px_2p5()
            .gap_1p5()
            .rounded_full()
            .border_1()
            .border_color(if selected { theme.input } else { theme.border })
            .when(selected, |this| this.bg(theme.secondary_hover))
            .text_xs()
            .text_color(if selected {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .cursor_pointer()
            .hover(|style| style.text_color(theme.foreground))
            .children(color.map(|color| {
                div()
                    .flex_none()
                    .size(ph_ui::rems_from_px(7.))
                    .rounded_full()
                    .bg(color)
            }))
            .child(label.clone())
            .aria_label(label)
            .on_click(move |_, _, cx| {
                _ = entity.update(cx, |workspace, cx| workspace.set_bridge_filter(to, cx));
            })
            .test_support()
    };
    let mut chips = vec![chip(
        "bridge-all".into(),
        "All workspaces".into(),
        None,
        filter.is_none(),
        None,
    )];
    for project in workspace.projects() {
        chips.push(chip(
            SharedString::from(format!("bridge-filter-{}", project.id)),
            project.name.clone().into(),
            Some(ph_ui::workspace_color(
                project.shade(ph_ui::WORKSPACE_COLORS.len()),
                dark,
            )),
            filter == Some(project.id),
            Some(project.id),
        ));
    }
    h_flex()
        .flex_none()
        .flex_wrap()
        .gap_2()
        .pl_1()
        .child(
            div()
                .mr_1()
                .text_lg()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child("Bridge"),
        )
        // With one place there is nothing to narrow to.
        .when(chips.len() > 2, |this| this.children(chips))
        .child(div().flex_1())
        .child(new_task_button(workspace, entity.clone()))
        .into_any_element()
}

/// *New task*: a chat in a workspace of the user's choosing. The Bridge is
/// of no one workspace, so it asks which.
fn new_task_button(workspace: &Workspace, entity: WeakEntity<Workspace>) -> impl IntoElement {
    let places: Vec<(ProjectId, String)> = workspace
        .projects()
        .iter()
        .map(|project| (project.id, project.name.clone()))
        .collect();
    // Those whose task can have a branch and worktree of its own. (A file
    // is looked for, no `git` run: this is asked while the Bridge draws.)
    let repositories: Vec<(ProjectId, String)> = workspace
        .projects()
        .iter()
        .filter(|project| {
            project
                .folders()
                .iter()
                .any(|folder| ph_git::is_repository(folder))
        })
        .map(|project| (project.id, project.name.clone()))
        .collect();
    Button::new("bridge-new-task")
        .primary()
        .small()
        .icon(IconName::Plus)
        .label("New task")
        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let menu = places.iter().fold(menu, |menu, (id, name)| {
                let (entity, id) = (entity.clone(), *id);
                menu.item(
                    PopupMenuItem::new(format!("In {name}"))
                        .icon(Icon::new(Icons::SquarePen))
                        .on_click(move |_, window, cx| {
                            _ = entity
                                .update(cx, |workspace, cx| workspace.new_chat_in(id, window, cx));
                        }),
                )
            });
            // For a chat that needs no folder: the Scratch workspace, made
            // when there is none (it was removed, or given folders).
            let menu = {
                let entity = entity.clone();
                menu.item(
                    PopupMenuItem::new("Without a folder")
                        .icon(Icon::new(Icons::MessagesSquare))
                        .on_click(move |_, window, cx| {
                            _ = entity.update(cx, |workspace, cx| {
                                if let Some(scratch) = workspace.scratch(cx) {
                                    workspace.new_chat_in(scratch, window, cx)
                                }
                            });
                        }),
                )
            };
            // A task apart from the workspace's own checkout: its branch
            // in a worktree, so several agents do not write over each
            // other and each ends as one pull request.
            let menu = if repositories.is_empty() {
                menu
            } else {
                repositories.iter().fold(
                    menu.separator().label("On a branch of its own"),
                    |menu, (id, name)| {
                        let (entity, id) = (entity.clone(), *id);
                        menu.item(
                            PopupMenuItem::new(format!("From {name}…"))
                                .icon(Icon::new(Icons::GitBranch))
                                .on_click(move |_, window, cx| {
                                    _ = entity.update(cx, |workspace, cx| {
                                        workspace.new_task_in(id, window, cx)
                                    });
                                }),
                        )
                    },
                )
            };
            menu.separator()
                .item(
                    PopupMenuItem::new("Open a folder as a workspace")
                        .icon(Icon::new(Icons::FolderOpen))
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(AddProject), cx)),
                )
                .item(
                    PopupMenuItem::new("New workspace without folders")
                        .icon(Icon::new(Icons::FolderPlus))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(NewWorkspace), cx)
                        }),
                )
        })
}

/// A column: its name and count, then its cards, scrolling on its own
/// while the columns stand side by side.
fn column_view(
    column: Column,
    cards: Vec<&Card>,
    extra: Option<AnyElement>,
    all: bool,
    scrolls: bool,
    entity: &WeakEntity<Workspace>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let count = cards.len();
    let hidden = if all { 0 } else { count.saturating_sub(SHOWN) };
    let key = column.key();
    // What waits for a look can be put away in one go: someone who comes
    // from a version without the Bridge finds a week of chats here.
    let settle: Vec<(ProjectId, SessionId)> = if column == Column::Review {
        cards
            .iter()
            .filter_map(|card| Some((card.project, card.session.as_ref()?.id)))
            .collect()
    } else {
        Vec::new()
    };
    let settle_all = (settle.len() > 1).then(|| {
        let entity = entity.clone();
        Button::new("bridge-review-menu")
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .accessibility_label("Ready for review menu")
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                let (entity, settle) = (entity.clone(), settle.clone());
                menu.item(
                    PopupMenuItem::new(format!("Mark all {} as done", settle.len())).on_click(
                        move |_, _, cx| {
                            _ = entity
                                .update(cx, |workspace, cx| workspace.settle_all(&settle, cx));
                        },
                    ),
                )
            })
    });
    let more = (count > SHOWN).then(|| {
        let entity = entity.clone();
        Button::new(SharedString::from(format!("bridge-more-{key}")))
            .ghost()
            .xsmall()
            .label(if all {
                "Show fewer".to_string()
            } else {
                format!("Show {hidden} more")
            })
            .on_click(move |_, _, cx| {
                _ = entity.update(cx, |workspace, cx| workspace.toggle_bridge_all(key, cx));
            })
    });
    let empty = (count == 0 && extra.is_none()).then(|| {
        div()
            .px_1()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(column.empty())
    });
    let rows: Vec<AnyElement> = cards
        .into_iter()
        .take(if all { usize::MAX } else { SHOWN })
        .map(|card| card_view(card, column, entity, cx))
        .collect();
    v_flex()
        .id(SharedString::from(format!("bridge-column-{key}")))
        .min_w_0()
        .min_h_0()
        .gap_2()
        .child(
            h_flex()
                .flex_none()
                .h(ph_ui::rems_from_px(24.))
                .px_1()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .size(ph_ui::rems_from_px(8.))
                        .rounded_full()
                        .bg(column.color(cx)),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(column.title()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(count.to_string()),
                )
                .child(div().flex_1())
                .children(settle_all),
        )
        .child(
            v_flex()
                .id(SharedString::from(format!("bridge-list-{key}")))
                .gap_2()
                .when(scrolls, |this| this.flex_1().min_h_0().overflow_y_scroll())
                .children(extra)
                .children(rows)
                .children(empty)
                .children(more),
        )
        .aria_label(column.title())
        .test_support()
        .into_any_element()
}

/// A task's card: where it lives, its agent and age; its title; what it
/// asks, does or said last; and what to do about it.
fn card_view(
    card: &Card,
    column: Column,
    entity: &WeakEntity<Workspace>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme();
    let state = card.summary.state;
    let key = match &card.target {
        Target::Chat(session) => format!("{session}"),
        Target::Terminal(pane) => pane.clone(),
        Target::Preparing(number) => format!("preparing-{}-{number}", card.project),
    };
    let group = SharedString::from(format!("bridge-card-{key}"));
    let edge = match state {
        ChatState::NeedsYou => theme.warning.opacity(0.45),
        ChatState::Failed => theme.danger.opacity(0.45),
        _ => theme.border,
    };
    let line_color = match state {
        ChatState::NeedsYou => theme.warning,
        ChatState::Failed => theme.danger,
        _ => theme.muted_foreground,
    };
    let line = match state {
        ChatState::Failed => Some(
            card.summary
                .line
                .clone()
                .filter(|line| !line.is_empty())
                .unwrap_or_else(|| "Stopped with an error.".into()),
        ),
        _ => card.summary.line.clone().filter(|line| !line.is_empty()),
    };
    let open = {
        let (entity, project, target, github) = (
            entity.clone(),
            card.project,
            card.target.clone(),
            card.github,
        );
        move |window: &mut Window, cx: &mut App| {
            _ = entity.update(cx, |workspace, cx| {
                workspace.open_task_card(project, &target, github, window, cx)
            });
        }
    };
    let busy = matches!(state, ChatState::Working | ChatState::NeedsYou);
    let menu = card.session.clone().map(|session| {
        chat_menu(
            entity.clone(),
            card.project,
            session,
            (card.pinned, card.settled),
            busy,
        )
    });
    // What the column is for, one press away.
    let actions: Option<AnyElement> = match (column, &card.target) {
        (Column::NeedsYou, _) => {
            // A question answered by picking an option is answered here:
            // the options as the agent gave them, and the chat one press
            // away for whoever wants to read around it first.
            let choices = card
                .request
                .as_ref()
                .filter(|request| request.read(cx).is_choice())
                .map(|request| (request.clone(), request.read(cx).options.clone()));
            let open = open.clone();
            Some(match choices {
                Some((request, options)) => h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(options.into_iter().enumerate().map(|(ix, option)| {
                        let request = request.clone();
                        let id = option.id.clone();
                        let button =
                            Button::new(SharedString::from(format!("bridge-choose-{key}-{ix}")))
                                .small()
                                .label(option.label.clone())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    request.update(cx, |request, cx| request.choose(&id, cx));
                                });
                        // What lets the agent go on stands out; the first
                        // such option most.
                        match (option.allows, ix) {
                            (true, 0) => button.primary(),
                            (true, _) => button.outline(),
                            (false, _) => button.ghost(),
                        }
                    }))
                    .child(
                        Button::new(SharedString::from(format!("bridge-answer-{key}")))
                            .ghost()
                            .small()
                            .label("Open")
                            .tooltip("Read the chat before answering")
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                open(window, cx)
                            }),
                    )
                    .into_any_element(),
                None => Button::new(SharedString::from(format!("bridge-answer-{key}")))
                    .primary()
                    .small()
                    .label(if state == ChatState::Failed {
                        "See what happened"
                    } else {
                        "Answer"
                    })
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        open(window, cx)
                    })
                    .into_any_element(),
            })
        }
        (Column::Review, Target::Chat(session)) if card.session.is_some() => {
            let (project, session) = (card.project, *session);
            let (review, done) = (entity.clone(), entity.clone());
            Some(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .when(card.has_folders, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("bridge-review-{key}")))
                                .outline()
                                .small()
                                .label("Review changes")
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    _ = review.update(cx, |workspace, cx| {
                                        workspace.review_task(project, session, window, cx)
                                    });
                                }),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("bridge-done-{key}")))
                            .ghost()
                            .small()
                            .icon(Icons::Check)
                            .label("Done")
                            .tooltip("Mark this task as done")
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                _ = done.update(cx, |workspace, cx| {
                                    workspace.settle_all(&[(project, session)], cx)
                                });
                            }),
                    )
                    .into_any_element(),
            )
        }
        _ => None,
    };
    v_flex()
        .id(SharedString::from(format!("bridge-card-{key}")))
        .group(group.clone())
        .relative()
        .flex_none()
        .p_3()
        .gap_2()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(edge)
        // A step off the panel they lie on (which has the theme's
        // background), so a card reads as a thing of its own; those put
        // away, fainter.
        .bg(if column == Column::Done {
            theme.secondary.opacity(0.45)
        } else {
            theme.secondary
        })
        .cursor_pointer()
        .hover(|style| style.border_color(theme.muted_foreground.opacity(0.5)))
        .on_click(move |_, window, cx| open(window, cx))
        .child(
            h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(theme.muted_foreground)
                .children(card.color.map(|color| {
                    div()
                        .flex_none()
                        .size(ph_ui::rems_from_px(7.))
                        .rounded_full()
                        .bg(color)
                }))
                .child(div().min_w_0().truncate().child(card.place.clone()))
                .child("·")
                .child(card.summary.agent.mark(ph_ui::rems_from_px(12.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(card.summary.agent.name),
                )
                .when(card.pinned, |this| {
                    this.child(Icon::new(Icons::Pin).xsmall())
                })
                .child(
                    div()
                        .flex_none()
                        .group_hover(group.clone(), |style| style.invisible())
                        .child(crate::sidebar::age(card.updated_at)),
                ),
        )
        .child(
            h_flex()
                .items_start()
                .gap_2()
                .when(state == ChatState::Working, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .pt_0p5()
                            .child(ph_ui::Loader::new().xsmall()),
                    )
                })
                .when(state == ChatState::Done, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .mt_1p5()
                            .size(ph_ui::rems_from_px(8.))
                            .rounded_full()
                            .bg(theme.success),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .line_clamp(2)
                        .child(card.title.clone()),
                ),
        )
        .when_some(line, |this, line| {
            this.child(
                div()
                    .text_xs()
                    .text_color(line_color)
                    .line_clamp(3)
                    .child(line),
            )
        })
        .children(facts_row(card, &key, cx))
        .children(actions)
        // The task's menu, over its age, while the pointer is on the card.
        .children(menu.map(|menu| {
            div()
                .absolute()
                .top_2()
                .right_2()
                .invisible()
                .group_hover(group, |style| style.visible())
                .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation()
                })
                .child(menu)
        }))
        .test_support()
        .into_any_element()
}

/// What a task is besides its chat, in a row of its card: the branch it
/// works on, what it changed and how it checked that, and its pull request
/// with where the checks stand. Nothing of it known: no row.
fn facts_row(card: &Card, key: &str, cx: &App) -> Option<AnyElement> {
    use crate::task_facts::Checks;
    let theme = cx.theme();
    let place = card
        .folder
        .as_deref()
        .and_then(|folder| crate::task_facts::of(folder, cx));
    let branch = place.and_then(|place| place.branch.clone());
    let pull = place.and_then(|place| place.pull.clone());
    let facts = card.facts.as_ref();
    if branch.is_none() && pull.is_none() && facts.is_none() {
        return None;
    }
    let plural = |count: usize, one: &str| {
        if count == 1 {
            format!("1 {one}")
        } else {
            format!("{count} {one}s")
        }
    };
    let mut row = h_flex()
        .flex_wrap()
        .gap_x_2p5()
        .gap_y_0p5()
        .text_xs()
        .text_color(theme.muted_foreground);
    if let Some(branch) = branch {
        row = row.child(
            h_flex()
                .min_w_0()
                .max_w(ph_ui::rems_from_px(190.))
                .gap_1()
                .child(Icon::new(Icons::GitBranch).xsmall())
                .child(div().min_w_0().truncate().child(branch)),
        );
    }
    if let Some(facts) = facts {
        if facts.files > 0 {
            row = row.child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        div()
                            .text_color(theme.success)
                            .child(format!("+{}", facts.added)),
                    )
                    .child(
                        div()
                            .text_color(theme.danger)
                            .child(format!("−{}", facts.removed)),
                    )
                    .child(plural(facts.files, "file")),
            );
        }
        if let Some(ok) = facts.tests_ok {
            let label = match (ok, facts.tests_passed, facts.tests_failed) {
                (true, Some(passed), _) => format!("{} passed", plural(passed, "test")),
                (true, None, _) => "Tests passed".to_string(),
                (false, _, Some(failed)) if failed > 0 => {
                    format!("{} failed", plural(failed, "test"))
                }
                (false, ..) => "Tests failed".to_string(),
            };
            let color = if ok { theme.success } else { theme.danger };
            row = row.child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(Icon::new(Icons::FlaskConical).xsmall().text_color(color))
                    .child(label),
            );
        }
        if facts.screenshots > 0 {
            row = row.child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(Icon::new(Icons::Camera).xsmall())
                    .child(plural(facts.screenshots, "screenshot")),
            );
        }
    }
    if let Some(pull) = pull {
        let (mark, about) = match pull.checks {
            Some(Checks::Passed) => (Some(theme.success), "checks passed".to_string()),
            Some(Checks::Failed(failed)) => (
                Some(theme.danger),
                format!("{} failed", plural(failed, "check")),
            ),
            Some(Checks::Running) => (Some(theme.warning), "checks running".to_string()),
            None => (None, "no checks".to_string()),
        };
        let tip = SharedString::from(format!(
            "{} #{}, {about}. Opens it in the browser.",
            if pull.draft {
                "Draft pull request"
            } else {
                "Pull request"
            },
            pull.number
        ));
        let url = pull.url.clone();
        row = row.child(
            h_flex()
                .id(SharedString::from(format!("bridge-pull-{key}")))
                .flex_none()
                .gap_1()
                .cursor_pointer()
                .hover(|style| style.text_color(theme.foreground))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&url);
                })
                .child(Icon::new(Icons::GitPullRequest).xsmall())
                .child(format!("#{}", pull.number))
                .children(mark.map(|color| {
                    div()
                        .flex_none()
                        .size(ph_ui::rems_from_px(7.))
                        .rounded_full()
                        .bg(color)
                })),
        );
    }
    Some(row.into_any_element())
}

/// In the review column: pull requests other people asked the user to
/// review are read in Reviews. Said here so they are not missed.
fn reviews_card(
    workspace: &Workspace,
    entity: &WeakEntity<Workspace>,
    cx: &App,
) -> Option<AnyElement> {
    let waiting = workspace
        .github_view()
        .map_or(0, |github| github.read(cx).waiting_for_review());
    if waiting == 0 {
        return None;
    }
    let theme = cx.theme();
    let entity = entity.clone();
    Some(
        h_flex()
            .id("bridge-reviews")
            .flex_none()
            .p_3()
            .gap_2()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .cursor_pointer()
            .hover(|style| style.border_color(theme.muted_foreground.opacity(0.5)))
            .child(
                Icon::new(Icons::GitPullRequest)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(div().flex_1().min_w_0().text_sm().child(if waiting == 1 {
                "1 pull request waits for your review".to_string()
            } else {
                format!("{waiting} pull requests wait for your review")
            }))
            .child(Icon::new(IconName::ChevronRight).small())
            .on_click(move |_, window, cx| {
                _ = entity.update(cx, |workspace, cx| {
                    workspace.set_view(View::GitHub, window, cx)
                });
            })
            .test_support()
            .into_any_element(),
    )
}

/// The Bridge before there is anything on it: how to begin.
fn welcome(window: &Window, cx: &App) -> impl IntoElement {
    div().flex_1().min_h_0().child(
        crate::frame::island(window, cx).child(
            ph_ui::EmptyState::new("Nothing under way yet")
                .icon(Icons::ShipWheel)
                .description(
                    "Open a folder to work on it with agents, or start a chat that needs none. Every task shows here, with what it needs from you.",
                )
                .action(
                    h_flex()
                        .flex_wrap()
                        .justify_center()
                        .gap_2()
                        .child(
                            Button::new("bridge-open-folder")
                                .primary()
                                .icon(Icons::FolderOpen)
                                .label("Open a folder")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(AddProject), cx)
                                }),
                        )
                        .child(
                            Button::new("bridge-new-chat")
                                .outline()
                                .label("New chat")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(crate::actions::NewSession), cx)
                                }),
                        ),
                ),
        ),
    )
}

/// What can be done to workspace `id`, as menu items (the right click on
/// its badge, and its name in the title bar). Each opens it first, then
/// runs the action on the workspace in front.
pub(crate) fn workspace_menu_items(
    menu: PopupMenu,
    workspace: WeakEntity<Workspace>,
    id: ProjectId,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let item = |label: &'static str, icon: Icons, action: fn() -> Box<dyn gpui_kit::Action>| {
        let workspace = workspace.clone();
        PopupMenuItem::new(label)
            .icon(Icon::new(icon))
            .on_click(move |_, window, cx| {
                let here = workspace
                    .update(cx, |workspace, cx| {
                        workspace.open_project(id, window, cx);
                        workspace.view() == View::Workspaces
                            && workspace.active_project_id() == Some(id)
                    })
                    .unwrap_or(false);
                // Shown in another window: that window came forward, and
                // the action is not run here, on whatever this one shows.
                if here {
                    window.dispatch_action(action(), cx);
                }
            })
    };
    let colours = {
        let workspace = workspace.clone();
        PopupMenu::build(window, cx, move |menu, _, _| {
            ph_ui::WORKSPACE_COLORS
                .iter()
                .enumerate()
                .fold(menu, |menu, (ix, (name, _))| {
                    let workspace = workspace.clone();
                    menu.item(PopupMenuItem::new(*name).on_click(move |_, _, cx| {
                        _ = workspace.update(cx, |workspace, cx| {
                            workspace.set_color(id, Some(ix as u8), cx)
                        });
                    }))
                })
        })
    };
    let task = crate::tasks::of(crate::AppState::store(cx), id);
    let new_chat = workspace.clone();
    let menu = menu
        .item(
            PopupMenuItem::new("New chat")
                .icon(Icon::new(Icons::SquarePen))
                .on_click(move |_, window, cx| {
                    _ = new_chat.update(cx, |workspace, cx| workspace.new_chat_in(id, window, cx));
                }),
        )
        .item(item("New task in a worktree", Icons::GitBranch, || {
            Box::new(NewTask)
        }))
        .when(task.is_some(), |menu| {
            menu.item(item("Finish this task", Icons::Check, || {
                Box::new(FinishTask)
            }))
        })
        .separator()
        .item(item("Add a folder", Icons::FolderPlus, || {
            Box::new(AddFolderToWorkspace)
        }));
    menu.item(item("Rename", Icons::Pencil, || Box::new(RenameWorkspace)))
        .item(item("Open in a new window", Icons::ExternalLink, || {
            Box::new(OpenInNewWindow)
        }))
        .item(PopupMenuItem::submenu("Colour", colours))
        .separator()
        .item(item(
            concat!("Remove from ", ph_brand::name!()),
            Icons::Trash,
            || Box::new(RemoveWorkspace),
        ))
}

/// A task's menu: pin it, mark it done, rename or delete it.
fn chat_menu(
    workspace: WeakEntity<Workspace>,
    project: ProjectId,
    session: Session,
    marks: (bool, bool),
    busy: bool,
) -> impl IntoElement {
    let id = session.id;
    Button::new(SharedString::from(format!("chat-menu-{id}")))
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .accessibility_label("Task menu")
        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            chat_menu_items(menu, &workspace, project, &session, marks, busy)
        })
}

/// What can be done with a chat (pinned and marked done as `marks` say):
/// the menu of its card, and of its tab over a workspace's chat.
pub(crate) fn chat_menu_items(
    menu: PopupMenu,
    workspace: &WeakEntity<Workspace>,
    project: ProjectId,
    session: &Session,
    (pinned, settled): (bool, bool),
    busy: bool,
) -> PopupMenu {
    let id = session.id;
    let (pin, settle, rename, delete) = (
        workspace.clone(),
        workspace.clone(),
        workspace.clone(),
        workspace.clone(),
    );
    let (rename_session, delete_session) = (session.clone(), session.clone());
    menu.item(
        PopupMenuItem::new(if pinned { "Unpin" } else { "Pin to the top" })
            .icon(Icon::new(if pinned { Icons::PinOff } else { Icons::Pin }))
            .on_click(move |_, _, cx| {
                _ = pin.update(cx, |workspace, cx| workspace.toggle_pin(id, cx));
            }),
    )
    .item(
        PopupMenuItem::new(if settled { "Not done" } else { "Mark as done" })
            .icon(Icon::new(if settled { Icons::Undo2 } else { Icons::Check }))
            .on_click(move |_, _, cx| {
                _ = settle.update(cx, |workspace, cx| {
                    workspace.toggle_settled(project, id, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("Rename")
            .icon(Icon::new(Icons::Pencil))
            .on_click(move |_, window, cx| {
                _ = rename.update(cx, |workspace, cx| {
                    workspace.rename_chat(project, &rename_session, window, cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new(if busy {
            "Delete (still working)"
        } else {
            "Delete"
        })
        .icon(Icon::new(Icons::Trash))
        .disabled(busy)
        .on_click(move |_, window, cx| {
            _ = delete.update(cx, |workspace, cx| {
                workspace.delete_chat(project, &delete_session, window, cx)
            });
        }),
    )
}

/// Opens tool `tab` beside a task's chat (the review of its changes).
pub(crate) const REVIEW_TOOL: ContextTab = ContextTab::Changes;

/// A chat PilotHouse started rather than the user: a schedule's, a UI
/// test's recording or repair, a review of the user's own changes.
fn is_automated(session: &Session) -> bool {
    session.kind == "review"
        || session.title.ends_with("(scheduled)")
        || session.title.starts_with("Record test:")
        || session.title.starts_with("Repair test:")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60 * 1000;

    fn card(state: ChatState, settled: bool, automated: bool, age: i64) -> Card {
        let agent = &ph_agent::agents::AGENTS[0];
        Card {
            project: 1,
            github: false,
            target: Target::Chat(1),
            title: "Task".into(),
            place: "demo".into(),
            color: None,
            updated_at: 100 * 24 * HOUR - age,
            summary: ChatSummary {
                state,
                title: "Task".into(),
                agent,
                line: None,
            },
            session: None,
            pinned: false,
            settled,
            has_folders: true,
            automated,
            folder: None,
            facts: None,
            request: None,
        }
    }

    fn column(state: ChatState, settled: bool, automated: bool, age: i64) -> Column {
        Column::of(&card(state, settled, automated, age), 100 * 24 * HOUR)
    }

    #[test]
    fn a_task_stands_under_what_it_needs() {
        assert_eq!(
            column(ChatState::NeedsYou, false, false, HOUR),
            Column::NeedsYou
        );
        assert_eq!(
            column(ChatState::Working, true, false, HOUR),
            Column::Working
        );
        assert_eq!(column(ChatState::Done, false, false, HOUR), Column::Review);
        assert_eq!(column(ChatState::Idle, false, false, HOUR), Column::Review);
        // Marked done, or not touched for more than a week.
        assert_eq!(column(ChatState::Idle, true, false, HOUR), Column::Done);
        assert_eq!(
            column(ChatState::Idle, false, false, 8 * 24 * HOUR),
            Column::Done
        );
    }

    #[test]
    fn a_failed_task_can_be_put_away() {
        assert_eq!(
            column(ChatState::Failed, false, false, HOUR),
            Column::NeedsYou
        );
        assert_eq!(column(ChatState::Failed, true, false, HOUR), Column::Done);
        // An old chat that ends on an error, merely opened again.
        assert_eq!(
            column(ChatState::Failed, false, false, 30 * 24 * HOUR),
            Column::Done
        );
    }

    #[test]
    fn runs_do_not_bury_the_tasks() {
        // A schedule's chat waits for a look while its news is fresh, and
        // asks like any other; read, it is done.
        assert_eq!(column(ChatState::Done, false, true, HOUR), Column::Review);
        assert_eq!(
            column(ChatState::NeedsYou, false, true, HOUR),
            Column::NeedsYou
        );
        assert_eq!(column(ChatState::Idle, false, true, HOUR), Column::Done);
    }
}
