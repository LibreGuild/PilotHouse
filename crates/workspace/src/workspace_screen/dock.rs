//! The workspace's dockable layout: the chat, the editor and every tool are
//! panes of a [`DockArea`]. A pane's tab drags to any group, to a group's
//! edge (a new split), or into the left, right or bottom dock, so each
//! developer arranges the screen their way. The layout is kept per
//! workspace.
//!
//! By default the tools sit in a column beside the chat, inside the main
//! area (not a side dock), so the bottom panel spans the editor, the chat
//! and the tools: the terminal and Logcat, whose long lines want the
//! window's whole width, open there. The column hides and comes back as a
//! whole (its tabs and the one in front are remembered).
//!
//! A pane is only a frame: what it shows belongs to the screen (the chat
//! panel, the files, a tool panel made the first time it is shown), so a
//! pane can move, close and come back without touching what runs in it.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::dock::{
    BasePanel, BasePanelView, DockArea, DockAreaState, DockLayout, DockPlacement, InsertTarget,
    NodeId, Panel, PanelControl, PanelEvent, PanelId, PanelInfo, PanelState, TabGroup,
    panel_handle, register_panel,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, WeakEntity,
    Window, div,
};

use super::WorkspaceScreen;
use crate::context_panel::ContextTab;

/// Bumped when a saved layout can no longer be read as it is. Version 1
/// (tools in the right dock) is still read, and moved to version 2's
/// places once.
pub(super) const LAYOUT_VERSION: usize = 2;

/// What a pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Pane {
    Chat,
    /// The open files (hidden while none is open).
    Editor,
    Tool(ContextTab),
}

impl Pane {
    pub(crate) fn all() -> impl Iterator<Item = Pane> {
        [Pane::Chat, Pane::Editor]
            .into_iter()
            .chain(ContextTab::ALL.into_iter().map(Pane::Tool))
    }

    /// Its name in saved layouts; never change one.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Pane::Chat => "pilothouse.chat",
            Pane::Editor => "pilothouse.editor",
            Pane::Tool(tab) => match tab {
                ContextTab::Files => "pilothouse.files",
                ContextTab::Changes => "pilothouse.changes",
                ContextTab::Pulls => "pilothouse.pulls",
                ContextTab::Board => "pilothouse.board",
                ContextTab::Terminal => "pilothouse.terminal",
                ContextTab::Browser => "pilothouse.browser",
                ContextTab::Devices => "pilothouse.devices",
                ContextTab::Runs => "pilothouse.runs",
                ContextTab::Tests => "pilothouse.tests",
                ContextTab::Logcat => "pilothouse.logcat",
            },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Pane::Chat => "Chat",
            Pane::Editor => "Editor",
            Pane::Tool(tab) => tab.label(),
        }
    }

    fn icon(self) -> ph_ui::Icons {
        match self {
            Pane::Chat => ph_ui::Icons::MessageSquare,
            Pane::Editor => ph_ui::Icons::FileCode,
            Pane::Tool(tab) => tab.icon(),
        }
    }

    /// A tool whose home is the bottom panel: long lines, read across.
    pub(crate) fn in_bottom_panel(self) -> bool {
        matches!(self, Pane::Tool(tab) if tab.in_bottom_panel())
    }

    /// A tool whose home is the column beside the chat.
    fn in_column(self) -> bool {
        matches!(self, Pane::Tool(_)) && !self.in_bottom_panel()
    }
}

/// One pane: draws what the screen has for it.
pub(crate) struct DockPane {
    pane: Pane,
    screen: WeakEntity<WorkspaceScreen>,
    focus: FocusHandle,
    /// The tab group it is in, for zooming.
    group: Option<WeakEntity<TabGroup>>,
}

impl DockPane {
    pub(super) fn new(pane: Pane, screen: WeakEntity<WorkspaceScreen>, cx: &mut App) -> Self {
        Self {
            pane,
            screen,
            focus: cx.focus_handle(),
            group: None,
        }
    }

    /// The tab group it is in (to zoom it).
    pub(super) fn group(&self) -> Option<Entity<TabGroup>> {
        self.group.as_ref().and_then(WeakEntity::upgrade)
    }
}

impl EventEmitter<PanelEvent> for DockPane {}

impl Focusable for DockPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DockPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane;
        let content = self
            .screen
            .upgrade()
            .map(|screen| screen.update(cx, |screen, cx| screen.render_pane(pane, window, cx)));
        div().track_focus(&self.focus).size_full().children(content)
    }
}

impl BasePanel for DockPane {
    fn panel_name(&self) -> &'static str {
        self.pane.name()
    }

    /// The chat and the editor stay; tools close and come back from the
    /// icon bar.
    fn closable(&self, _: &App) -> bool {
        matches!(self.pane, Pane::Tool(_))
    }

    /// The editor shows while a file is open.
    fn visible(&self, cx: &App) -> bool {
        match self.pane {
            Pane::Editor => self
                .screen
                .upgrade()
                .is_some_and(|screen| screen.read(cx).has_open_files(cx)),
            _ => true,
        }
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        let (pane, screen) = (self.pane, self.screen.clone());
        // Outside this update: the screen may be the one editing the dock.
        cx.defer(move |cx| {
            _ = screen.update(cx, |screen, cx| screen.pane_shown(pane, active, cx));
        });
    }

    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }

    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let (pane, screen) = (self.pane, self.screen.clone());
        self.group = None;
        cx.defer(move |cx| {
            _ = screen.update(cx, |screen, cx| screen.pane_shown(pane, false, cx));
        });
    }
}

impl Panel for DockPane {
    fn tab_name(&self, _: &App) -> Option<gpui_kit::SharedString> {
        Some(self.pane.label().into())
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
        h_flex()
            .gap_1p5()
            .child(
                Icon::new(self.pane.icon())
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(self.pane.label())
    }

    /// The pane's ⋯ menu moves it anywhere, even where a drag cannot (the
    /// last pane of a dock never drags out of it).
    fn dropdown_menu(
        &mut self,
        menu: gpui_kit::component::menu::PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::component::menu::PopupMenu {
        use gpui_kit::component::menu::PopupMenuItem;
        let (pane, screen) = (self.pane, self.screen.clone());
        let (here, with_chat) = screen
            .upgrade()
            .map(|screen| {
                let panes = &screen.read(cx).panes;
                (panes.placement(pane, cx), panes.is_with_chat(pane, cx))
            })
            .unwrap_or_default();
        let menu = MoveTo::ALL
            .into_iter()
            .filter(|to| to.applies(pane, here, with_chat))
            .fold(menu.separator(), |menu, to| {
                let screen = screen.clone();
                menu.item(
                    PopupMenuItem::new(to.label()).on_click(move |_, window, cx| {
                        _ = screen.update(cx, |screen, cx| screen.move_pane(pane, to, window, cx));
                    }),
                )
            });
        menu.separator().item(
            PopupMenuItem::new("Reset layout").on_click(move |_, window, cx| {
                _ = screen.update(cx, |screen, cx| screen.reset_layout(window, cx));
            }),
        )
    }

    /// Zoom sits in the pane's ⋯ menu (and ⌘⇧J), not as a button on
    /// every pane header.
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Menu)
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

/// Where a pane's menu can move it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveTo {
    Left,
    Right,
    Bottom,
    /// Into the chat's tab group.
    WithChat,
    BesideChat,
    BelowChat,
}

impl MoveTo {
    const ALL: [MoveTo; 6] = [
        MoveTo::WithChat,
        MoveTo::BesideChat,
        MoveTo::BelowChat,
        MoveTo::Left,
        MoveTo::Right,
        MoveTo::Bottom,
    ];

    fn label(self) -> &'static str {
        match self {
            MoveTo::Left => "Move to the left side",
            MoveTo::Right => "Move to the right side",
            MoveTo::Bottom => "Move to the bottom",
            MoveTo::WithChat => "Move into the chat's tabs",
            MoveTo::BesideChat => "Move beside the chat",
            MoveTo::BelowChat => "Move below the chat",
        }
    }

    fn dock(self) -> Option<DockPlacement> {
        match self {
            MoveTo::Left => Some(DockPlacement::Left),
            MoveTo::Right => Some(DockPlacement::Right),
            MoveTo::Bottom => Some(DockPlacement::Bottom),
            MoveTo::WithChat | MoveTo::BesideChat | MoveTo::BelowChat => None,
        }
    }

    /// Offered for `pane` docked at `here` (`with_chat`: in the chat's tab
    /// group): not where it already is, and the chat is not moved next to
    /// itself.
    fn applies(self, pane: Pane, here: Option<DockPlacement>, with_chat: bool) -> bool {
        match self.dock() {
            Some(dock) => here != Some(dock),
            None => pane != Pane::Chat && !(self == MoveTo::WithChat && with_chat),
        }
    }
}

/// Each dock area's panes by name, so a saved layout is rebuilt with the
/// screen's own panes (the registry's builders cannot reach the screen).
#[derive(Default)]
struct DockPanes(HashMap<EntityId, HashMap<&'static str, Arc<dyn BasePanelView>>>);

impl Global for DockPanes {}

/// Registers the builders that hand saved layouts their panes (once).
fn register(cx: &mut App) {
    if cx.has_global::<DockPanes>() {
        return;
    }
    cx.set_global(DockPanes::default());
    for pane in Pane::all() {
        let name = pane.name();
        register_panel(cx, name, move |build, _, cx| {
            let area = build.dock_area().entity_id();
            cx.global::<DockPanes>()
                .0
                .get(&area)
                .and_then(|panes| panes.get(name))
                .cloned()
                .expect("a saved layout is loaded only with its screen's panes")
        });
    }
}

/// A screen's panes, made with it.
pub(super) struct Panes {
    pub(super) area: Entity<DockArea>,
    panes: HashMap<Pane, Entity<DockPane>>,
    /// The tools column while it is hidden: its tabs in order, and the one
    /// in front.
    hidden_column: Option<(Vec<Pane>, Pane)>,
}

impl Panes {
    pub(super) fn new(
        area: Entity<DockArea>,
        screen: WeakEntity<WorkspaceScreen>,
        cx: &mut App,
    ) -> Self {
        register(cx);
        let panes: HashMap<Pane, Entity<DockPane>> = Pane::all()
            .map(|pane| {
                let screen = screen.clone();
                (pane, cx.new(|cx| DockPane::new(pane, screen, cx)))
            })
            .collect();
        let handles = panes
            .iter()
            .map(|(pane, entity)| (pane.name(), panel_handle(entity.clone())))
            .collect();
        let area_id = area.entity_id();
        cx.global_mut::<DockPanes>().0.insert(area_id, handles);
        // A screen closed (an idle workspace) takes its panes along.
        cx.observe_release(&area, move |_, cx| {
            cx.global_mut::<DockPanes>().0.remove(&area_id);
        })
        .detach();
        Self {
            area,
            panes,
            hidden_column: None,
        }
    }

    pub(super) fn pane(&self, pane: Pane) -> &Entity<DockPane> {
        &self.panes[&pane]
    }

    fn handle(&self, pane: Pane) -> Arc<dyn BasePanelView> {
        panel_handle(self.pane(pane).clone())
    }

    fn id(&self, pane: Pane) -> PanelId {
        PanelId::from(self.pane(pane).entity_id())
    }

    /// The pane a panel of the layout is.
    fn pane_of(&self, id: PanelId) -> Option<Pane> {
        self.panes
            .iter()
            .find(|(_, entity)| PanelId::from(entity.entity_id()) == id)
            .map(|(pane, _)| *pane)
    }

    /// The tools column: the first tab group of the main area that holds a
    /// column tool and is not the chat's.
    fn column_node(&self, area: &DockArea) -> Option<NodeId> {
        let tree = area.layout(DockPlacement::Center)?;
        let chat = tree.find_panel_node(self.id(Pane::Chat));
        tree.panels()
            .filter(|id| self.pane_of(*id).is_some_and(Pane::in_column))
            .filter_map(|id| tree.find_panel_node(id))
            .find(|node| Some(*node) != chat)
    }

    /// The tools column's tabs, in order.
    fn column_panes(&self, cx: &App) -> Vec<Pane> {
        let area = self.area.read(cx);
        let Some(node) = self.column_node(area) else {
            return Vec::new();
        };
        let Some(tree) = area.layout(DockPlacement::Center) else {
            return Vec::new();
        };
        tree.panels()
            .filter(|id| tree.find_panel_node(*id) == Some(node))
            .filter_map(|id| self.pane_of(id))
            .collect()
    }

    /// Where `pane` is docked, if it is.
    pub(super) fn placement(&self, pane: Pane, cx: &App) -> Option<DockPlacement> {
        let id = self.id(pane);
        let area = self.area.read(cx);
        [
            DockPlacement::Center,
            DockPlacement::Left,
            DockPlacement::Right,
            DockPlacement::Bottom,
        ]
        .into_iter()
        .find(|placement| {
            area.layout(*placement)
                .is_some_and(|tree| tree.panels().any(|panel| panel == id))
        })
    }

    /// Whether `pane` is a tab of the chat's group.
    pub(super) fn is_with_chat(&self, pane: Pane, cx: &App) -> bool {
        let Some(tree) = self.area.read(cx).layout(DockPlacement::Center) else {
            return false;
        };
        let chat = tree.find_panel_node(self.id(Pane::Chat));
        chat.is_some() && chat == tree.find_panel_node(self.id(pane))
    }

    /// Whether `pane` is on screen: docked, in an open dock.
    pub(super) fn is_docked_open(&self, pane: Pane, cx: &App) -> bool {
        match self.placement(pane, cx) {
            Some(DockPlacement::Center) => true,
            Some(placement) => self.area.read(cx).is_dock_open(placement),
            None => false,
        }
    }

    /// The layout a workspace starts with: the chat and the editor as tabs
    /// of one group (opening a file brings the editor forward) and, when
    /// `open`, the `column` tool beside them and the `bottom` one (the
    /// terminal, Logcat) below. Also what *Reset layout* goes back to, so
    /// every dock goes away.
    pub(super) fn set_default(
        &mut self,
        column: ContextTab,
        bottom: Option<ContextTab>,
        open: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.hidden_column = None;
        let main = DockLayout::tabs()
            .panel_view(self.handle(Pane::Chat), cx)
            .panel_view(self.handle(Pane::Editor), cx);
        let tool = Pane::Tool(column);
        let center = if open && tool.in_column() {
            let width = column_width(window, cx);
            DockLayout::h_split().child(main, None).child(
                DockLayout::tabs().panel_view(self.handle(tool), cx),
                Some(width),
            )
        } else {
            main
        };
        self.area.update(cx, |area, cx| {
            if area.is_zoomed() {
                area.set_zoomed_out(window, cx);
            }
            for placement in [
                DockPlacement::Left,
                DockPlacement::Right,
                DockPlacement::Bottom,
            ] {
                area.remove_dock(placement, window, cx);
            }
            area.set_center(center, window, cx);
        });
        if open && let Some(bottom) = bottom.map(Pane::Tool).filter(|pane| pane.in_bottom_panel()) {
            self.show(bottom, window, cx);
        }
    }

    /// Restores a saved layout; `false` if it cannot be (none, or from an
    /// older build), leaving the area as it was.
    pub(super) fn load(
        &mut self,
        saved: Option<DockAreaState>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(state) = saved.filter(|state| {
            matches!(state.version, Some(version) if version == 1 || version == LAYOUT_VERSION)
        }) else {
            return false;
        };
        let from_v1 = state.version == Some(1);
        // Every pane the layout names must be one this build has.
        let known = |state: &PanelState| known_names(state);
        let docks_known = [&state.left_dock, &state.right_dock, &state.bottom_dock]
            .into_iter()
            .flatten()
            .all(|dock| known(dock.panel()));
        if !known(&state.center) || !docks_known {
            return false;
        }
        if self
            .area
            .update(cx, |area, cx| area.load(state, window, cx))
            .is_err()
        {
            return false;
        }
        if from_v1 {
            self.move_from_v1(window, cx);
        }
        true
    }

    /// A layout of version 1 had the tools in the right dock: they move to
    /// the column beside the chat, the terminal to the bottom panel, once.
    /// A tool the user put anywhere else stays there.
    fn move_from_v1(&mut self, window: &mut Window, cx: &mut App) {
        let (right, was_open) = {
            let area = self.area.read(cx);
            let right: Vec<Pane> = area
                .layout(DockPlacement::Right)
                .map(|tree| tree.panels().filter_map(|id| self.pane_of(id)).collect())
                .unwrap_or_default();
            (right, area.is_dock_open(DockPlacement::Right))
        };
        let front = right.first().copied();
        for pane in right {
            if pane.in_bottom_panel() {
                self.move_to(pane, MoveTo::Bottom, window, cx);
                // Not opened by the move: it was in the right dock's back.
                self.area.update(cx, |area, cx| {
                    if area.is_dock_open(DockPlacement::Bottom) {
                        area.toggle_dock(DockPlacement::Bottom, window, cx);
                    }
                });
            } else if pane.in_column() {
                self.area.update(cx, |area, cx| {
                    area.remove_panel(self.pane(pane).clone(), window, cx);
                });
                self.show(pane, window, cx);
            }
        }
        let column = self.column_panes(cx);
        if !was_open && !column.is_empty() {
            let front = front
                .filter(|pane| column.contains(pane))
                .unwrap_or(column[0]);
            self.hide_column(column, front, window, cx);
        }
        self.area.update(cx, |area, cx| {
            if area
                .layout(DockPlacement::Right)
                .is_some_and(|tree| tree.panels().next().is_none())
            {
                area.remove_dock(DockPlacement::Right, window, cx);
            }
            area.set_version(Some(LAYOUT_VERSION), cx);
        });
    }

    /// Shows `pane`: selects it where it is docked (opening its dock), or
    /// puts it in its home when it is not: the tools column (back as it was
    /// when it was hidden), or the bottom panel.
    pub(super) fn show(&mut self, pane: Pane, window: &mut Window, cx: &mut App) {
        if pane.in_column()
            && self.placement(pane, cx).is_none()
            && let Some((hidden, _)) = self.hidden_column.take()
        {
            // The whole column comes back, this tool in front.
            for hidden in hidden.iter().filter(|hidden| **hidden != pane) {
                if self.placement(*hidden, cx).is_none() {
                    self.place(*hidden, window, cx);
                }
            }
            if !hidden.contains(&pane) {
                self.place(pane, window, cx);
            }
        }
        let id = self.id(pane);
        let placement = match self.placement(pane, cx) {
            Some(placement) => placement,
            None => self.place(pane, window, cx),
        };
        self.area.update(cx, |area, cx| {
            if area.is_zoomed() {
                area.set_zoomed_out(window, cx);
            }
            if placement != DockPlacement::Center && !area.is_dock_open(placement) {
                area.toggle_dock(placement, window, cx);
            }
            area.select_panel(id, window, cx);
        });
    }

    /// Puts `pane`, not in the layout, in its home; returns where.
    fn place(&mut self, pane: Pane, window: &mut Window, cx: &mut App) -> DockPlacement {
        let (id, handle) = (self.id(pane), self.handle(pane));
        if pane.in_bottom_panel() {
            self.area.update(cx, |area, cx| {
                if area.has_dock(DockPlacement::Bottom) {
                    area.add_panel_view(handle, DockPlacement::Bottom, None, window, cx);
                } else {
                    area.set_dock(
                        DockPlacement::Bottom,
                        DockLayout::tabs().panel_view(handle, cx),
                        window,
                        cx,
                    );
                    let height = bottom_height(window);
                    area.set_dock_size(DockPlacement::Bottom, height, window, cx);
                }
            });
            return DockPlacement::Bottom;
        }
        let column = self.column_node(self.area.read(cx));
        let chat = self
            .area
            .read(cx)
            .layout(DockPlacement::Center)
            .and_then(|tree| tree.find_panel_node(self.id(Pane::Chat)));
        let width = column_width(window, cx);
        self.area.update(cx, |area, cx| {
            // Into the main area's first group, then where it belongs.
            area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
            let target = match (pane.in_column(), column, chat) {
                (true, Some(node), _) => Some(InsertTarget::Tabs {
                    node,
                    ix: None,
                    activate: true,
                }),
                (true, None, Some(node)) => Some(InsertTarget::Split {
                    node,
                    placement: gpui_kit::component::Placement::Right,
                    size: Some(width),
                }),
                _ => None,
            };
            if let Some(target) = target {
                area.move_panel(id, target, window, cx);
            }
        });
        DockPlacement::Center
    }

    /// Hides the tools column, or brings it back as it was (`fallback` in
    /// it when there was none).
    pub(super) fn toggle_column(
        &mut self,
        fallback: ContextTab,
        is_shown: impl Fn(Pane) -> bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let column = self.column_panes(cx);
        if !column.is_empty() {
            let front = column
                .iter()
                .copied()
                .find(|pane| is_shown(*pane))
                .unwrap_or(column[0]);
            self.hide_column(column, front, window, cx);
            return;
        }
        let front = self
            .hidden_column
            .as_ref()
            .map(|(_, front)| *front)
            .unwrap_or(Pane::Tool(fallback));
        self.show(front, window, cx);
    }

    fn hide_column(&mut self, column: Vec<Pane>, front: Pane, window: &mut Window, cx: &mut App) {
        let entities: Vec<Entity<DockPane>> =
            column.iter().map(|pane| self.pane(*pane).clone()).collect();
        self.area.update(cx, |area, cx| {
            for entity in entities {
                area.remove_panel(entity, window, cx);
            }
        });
        self.hidden_column = Some((column, front));
    }

    /// Opens or closes the bottom panel; `false` when there is none yet.
    pub(super) fn toggle_bottom(&self, window: &mut Window, cx: &mut App) -> bool {
        self.area.update(cx, |area, cx| {
            if !area.has_dock(DockPlacement::Bottom) {
                return false;
            }
            if area.is_zoomed() {
                area.set_zoomed_out(window, cx);
            }
            area.toggle_dock(DockPlacement::Bottom, window, cx);
            true
        })
    }

    /// Moves `pane` as its menu asked (docking it first if it is not).
    pub(super) fn move_to(&mut self, pane: Pane, to: MoveTo, window: &mut Window, cx: &mut App) {
        let (id, chat) = (self.id(pane), self.id(Pane::Chat));
        let (handle, entity) = (self.handle(pane), self.pane(pane).clone());
        if self.placement(pane, cx).is_none() {
            self.show(pane, window, cx);
        }
        let from = self.placement(pane, cx);
        self.area.update(cx, |area, cx| {
            if area.is_zoomed() {
                area.set_zoomed_out(window, cx);
            }
            match to.dock() {
                None => {
                    let Some(node) = area
                        .layout(DockPlacement::Center)
                        .and_then(|tree| tree.find_panel_node(chat))
                    else {
                        return;
                    };
                    let target = match to {
                        MoveTo::WithChat => InsertTarget::Tabs {
                            node,
                            ix: None,
                            activate: true,
                        },
                        MoveTo::BesideChat => InsertTarget::Split {
                            node,
                            placement: gpui_kit::component::Placement::Right,
                            size: None,
                        },
                        _ => InsertTarget::Split {
                            node,
                            placement: gpui_kit::component::Placement::Bottom,
                            size: None,
                        },
                    };
                    area.move_panel(id, target, window, cx);
                }
                Some(dock) => {
                    // Into the dock's first group, or a new dock.
                    let group = area.layout(dock).and_then(|tree| {
                        let first = tree.panels().next()?;
                        tree.find_panel_node(first)
                    });
                    match group {
                        Some(node) => area.move_panel(
                            id,
                            InsertTarget::Tabs {
                                node,
                                ix: None,
                                activate: true,
                            },
                            window,
                            cx,
                        ),
                        None => {
                            area.remove_panel(entity, window, cx);
                            area.set_dock(
                                dock,
                                DockLayout::tabs().panel_view(handle, cx),
                                window,
                                cx,
                            );
                            // A side dock's width, or the bottom panel's height.
                            let size = if dock == DockPlacement::Bottom {
                                bottom_height(window)
                            } else {
                                dock_size(window, cx)
                            };
                            area.set_dock_size(dock, size, window, cx);
                        }
                    }
                    if !area.is_dock_open(dock) {
                        area.toggle_dock(dock, window, cx);
                    }
                }
            }
            // A dock the pane left empty goes away.
            if let Some(from) = from
                && from != DockPlacement::Center
                && area
                    .layout(from)
                    .is_some_and(|tree| tree.panels().next().is_none())
            {
                area.remove_dock(from, window, cx);
            }
            area.select_panel(id, window, cx);
        });
    }
}

/// The bottom panel's height when it is made: room for a screenful of
/// output, the editor and chat keeping most of the window.
fn bottom_height(window: &Window) -> gpui_kit::Pixels {
    (window.viewport_size().height * 0.34).clamp(gpui_kit::px(180.), gpui_kit::px(420.))
}

/// Share of the main area the tools column takes when it is made: about
/// 70/30, the chat keeping the room (a file tree or a Git list needs little
/// width). Dragged wider or narrower, the column keeps that.
const COLUMN_SHARE: f32 = 0.28;

/// The tools column's width when it is made.
fn column_width(window: &Window, cx: &gpui_kit::App) -> gpui_kit::Pixels {
    let rail = gpui_kit::px(58.);
    let main = window.viewport_size().width - crate::sidebar::width(cx) - rail;
    (main * COLUMN_SHARE).clamp(gpui_kit::px(280.), gpui_kit::px(520.))
}

/// A new dock's width: the usual tool width, less when the window leaves
/// the chat too little room.
fn dock_size(window: &Window, cx: &gpui_kit::App) -> gpui_kit::Pixels {
    use ph_theme::layout;
    let rail = gpui_kit::px(58.);
    let available = window.viewport_size().width - crate::sidebar::width(cx) - rail;
    (available - layout::CHAT_MIN)
        .min(layout::CONTEXT_PANEL_DEFAULT)
        .max(layout::CONTEXT_PANEL_MIN)
}

/// Whether every panel `state` names is a pane of this build (splits and
/// tab groups hold panels; only a panel names one).
fn known_names(state: &PanelState) -> bool {
    match state.info {
        PanelInfo::Panel(_) => Pane::all().any(|pane| pane.name() == state.panel_name),
        PanelInfo::Stack { .. } | PanelInfo::Tabs { .. } => state.children.iter().all(known_names),
    }
}
