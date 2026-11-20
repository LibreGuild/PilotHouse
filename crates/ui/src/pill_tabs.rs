//! A row of pill tabs that fits the room it has: the views of a tool
//! (Git's Changes, Branch, History, Stashes). With room for every name they
//! all show; in a narrow pane the tabs not in front fold to their icons
//! (their names in tooltips), so none is cut off or pushed out of sight.
//! Without icons, or narrower still, the row scrolls sideways.

use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

/// What a tab does when it is pressed.
type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;

/// One tab of [`PillTabs`].
pub struct PillTab {
    id: SharedString,
    /// What the tab shows: its name, perhaps with a count.
    label: SharedString,
    /// Its name alone, for screen readers and the tooltip.
    name: SharedString,
    icon: Option<Icon>,
    selected: bool,
    on_click: OnClick,
}

impl PillTab {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        let label = label.into();
        Self {
            id: id.into(),
            name: label.clone(),
            label,
            icon: None,
            selected: false,
            on_click: Rc::new(on_click),
        }
    }

    /// The icon the tab folds to in a narrow pane.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Its name without what `label` adds (a count).
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// About how wide the tab is with its name, in pixels at the default
    /// interface size: the text at 14 px, its padding, and the icon.
    fn full_width(&self) -> f32 {
        let text = self.label.chars().count() as f32 * 7.6;
        let icon = if self.icon.is_some() { 20. } else { 0. };
        text + icon + 20.
    }
}

#[derive(IntoElement)]
pub struct PillTabs {
    id: ElementId,
    tabs: Vec<PillTab>,
}

impl PillTabs {
    pub fn new(id: impl Into<ElementId>, tabs: Vec<PillTab>) -> Self {
        Self {
            id: id.into(),
            tabs,
        }
    }
}

impl RenderOnce for PillTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (width, probe) = crate::measure::width_of(self.id.clone(), window, cx);
        // The names fit, with the gaps between the tabs?
        let needed: f32 = self.tabs.iter().map(PillTab::full_width).sum::<f32>()
            + 4. * self.tabs.len().saturating_sub(1) as f32;
        let fold = width.is_some_and(|width| crate::measure::design_px(width, window) < needed);
        let theme = cx.theme();
        h_flex()
            .id(self.id)
            .relative()
            .w_full()
            .min_w_0()
            .gap_1()
            // Narrower than even the folded tabs: they scroll.
            .overflow_x_scroll()
            .role(gpui_kit::accesskit::Role::TabList)
            .child(probe)
            .children(self.tabs.into_iter().map(|tab| {
                let folded = fold && !tab.selected && tab.icon.is_some();
                let (name, on_click) = (tab.name.clone(), tab.on_click.clone());
                h_flex()
                    .id(tab.id)
                    .flex_none()
                    .gap_1p5()
                    .px_2p5()
                    .py_1()
                    .rounded(theme.radius)
                    .text_sm()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .text_color(if tab.selected {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .when(tab.selected, |this| {
                        this.bg(theme.secondary)
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                    })
                    .when(!tab.selected, |this| {
                        this.hover(|style| style.text_color(theme.foreground))
                    })
                    .on_click(move |_, window, cx| on_click(window, cx))
                    .role(gpui_kit::accesskit::Role::Tab)
                    .aria_label(tab.name.clone())
                    // Shown only while there is room to spare, or the tab is
                    // in front: an icon beside every name would be clutter.
                    .when_some(tab.icon.filter(|_| fold), |this, icon| {
                        this.child(icon.small())
                    })
                    .when(!folded, |this| this.child(tab.label))
                    .when(folded, |this| {
                        this.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(crate::tip(name.clone()))
                                .build(window, cx)
                        })
                    })
                    .test_support()
            }))
    }
}
