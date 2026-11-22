//! Making a UI test: what it should check, in the user's words, and where
//! it starts, which is filled in from the page the browser shows (or the
//! app in front on the phone). Its name comes from the first words.

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use ph_control::ControlServer;
use serde_json::json;

use super::{Picked, TestsPanel, area};
use crate::model::{self, Target, TestCase};

/// The form for a new UI test.
pub(super) struct NewTest {
    scenario: Entity<TextareaState>,
    start: Entity<InputState>,
    name: Entity<InputState>,
    /// It is of a phone's app, not a page.
    phone: bool,
    /// What the start was filled in with: replaced when the other kind is
    /// chosen, unless the user typed something else.
    filled: String,
}

const NAME_HINT: &str = "Name (from what it checks, when left empty)";

/// What is typed as a start, as a test's target; `None` when it is neither
/// an address nor an app.
fn target_of(start: &str, phone: bool) -> Option<Target> {
    let start = start.trim();
    if start.is_empty() || start.contains(' ') {
        return None;
    }
    if phone {
        // `com.example.app`: words joined by dots, no address.
        let package = start.contains('.')
            && !start.contains(['/', ':'])
            && start
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
        return package.then(|| Target::Android {
            package: start.to_string(),
        });
    }
    Some(Target::Web {
        url: if start.contains("://") {
            start.to_string()
        } else {
            format!("http://{start}")
        },
    })
}

/// Whether the browser's address is a page a test could start at (not its
/// blank or start page).
fn is_page(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl TestsPanel {
    pub(super) fn open_new_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scenario = area(
            "Sign in as Ada, add a coffee to the cart; the cart should say Coffee.",
            window,
            cx,
        );
        let start = cx.new(|cx| InputState::new(window, cx).placeholder("http://localhost:3000"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(NAME_HINT));
        // The name it will get shows where one can be typed instead.
        let hint = name.clone();
        self._subscriptions.push(cx.subscribe_in(
            &scenario,
            window,
            move |_, scenario, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let from = model::name_from(&scenario.read(cx).value());
                    hint.update(cx, |hint, cx| {
                        hint.set_placeholder(
                            if from.is_empty() {
                                NAME_HINT.to_string()
                            } else {
                                from
                            },
                            window,
                            cx,
                        )
                    });
                }
            },
        ));
        scenario.update(cx, |scenario, cx| scenario.focus(window, cx));
        self.creating = Some(NewTest {
            scenario,
            start,
            name,
            phone: false,
            filled: String::new(),
        });
        self.command = None;
        self.inside = true;
        self.message = None;
        self.fill_start(window, cx);
        cx.notify();
    }

    /// Fills in where the test starts: the page the browser shows, or the
    /// app in front on the phone. Left as it is once the user typed there.
    fn fill_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.creating else {
            return;
        };
        let phone = form.phone;
        // Where the tests there are start, for when the browser or the
        // phone has nothing to say.
        let known = self
            .tests
            .iter()
            .find_map(|(_, test)| match &test.target {
                Target::Web { url } if !phone => Some(url.clone()),
                Target::Android { package } if phone => Some(package.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let asked = if phone {
            ControlServer::call("device.front", json!({}), cx)
        } else {
            ControlServer::call(
                "browser.where",
                json!({ "project_id": self.project_id }),
                cx,
            )
        };
        cx.spawn_in(window, async move |this, cx| {
            // No phone, no browser yet: the field stays for the user.
            let answer = asked.await.unwrap_or_default();
            let found = if phone {
                answer["package"].as_str().unwrap_or_default().to_string()
            } else {
                Some(answer["url"].as_str().unwrap_or_default())
                    .filter(|url| is_page(url))
                    .unwrap_or_default()
                    .to_string()
            };
            let found = if found.is_empty() { known } else { found };
            _ = this.update_in(cx, |this, window, cx| {
                let Some(form) = &mut this.creating else {
                    return;
                };
                if form.phone != phone {
                    return;
                }
                let typed = form.start.read(cx).value().to_string();
                if typed.is_empty() || typed == form.filled {
                    form.start
                        .update(cx, |start, cx| start.set_value(found.clone(), window, cx));
                    form.filled = found;
                }
            });
        })
        .detach();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.creating else {
            return;
        };
        let description = form.scenario.read(cx).value().trim().to_string();
        let typed = form.name.read(cx).value().trim().to_string();
        let name = if typed.is_empty() {
            model::name_from(&description)
        } else {
            typed
        };
        let start = form.start.read(cx).value().to_string();
        let problem = if description.is_empty() {
            Some("Say what it should check.")
        } else if name.is_empty() {
            Some("Give it a name.")
        } else {
            None
        };
        let target = target_of(&start, form.phone);
        let (Some(target), None) = (target, problem) else {
            self.message = Some((
                problem
                    .unwrap_or(if form.phone {
                        "Say which app: its package, like com.example.app."
                    } else {
                        "Say where it starts: an address, like http://localhost:3000."
                    })
                    .into(),
                true,
            ));
            cx.notify();
            return;
        };
        let test = TestCase {
            name,
            description,
            target,
            clean: None,
            steps: Vec::new(),
        };
        match model::create(&self.root, &test) {
            Ok(path) => {
                self.creating = None;
                self.message = None;
                self.tests.push((path.clone(), test));
                self.picked = Some(Picked::Check(path.clone()));
                self.inside = true;
                self.rebuild_rows(cx);
                self.record(path, cx);
                self.reload(cx);
            }
            Err(err) => self.message = Some((err.to_string().into(), true)),
        }
        let _ = window;
        cx.notify();
    }

    pub(super) fn render_new_test(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(form) = &self.creating else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let label = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(muted)
                .child(text)
        };
        v_flex()
            .id("new-ui-test")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(
                div()
                    .text_base()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("New UI test"),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("What should it check?"))
                    .child(Textarea::new(&form.scenario).h(ph_ui::rems_from_px(96.))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Where it starts"))
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                TabBar::new("new-test-kind")
                                    .segmented()
                                    .small()
                                    .selected_index(usize::from(form.phone))
                                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                        if let Some(form) = &mut this.creating {
                                            form.phone = *ix == 1;
                                            let hint = if form.phone {
                                                "com.example.app"
                                            } else {
                                                "http://localhost:3000"
                                            };
                                            form.start.update(cx, |start, cx| {
                                                start.set_placeholder(hint, window, cx)
                                            });
                                        }
                                        this.fill_start(window, cx);
                                        cx.notify();
                                    }))
                                    .child(Tab::new().label("Browser"))
                                    .child(Tab::new().label("Phone")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(ph_ui::rems_from_px(180.))
                                    .child(Input::new(&form.start).small()),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Name"))
                    .child(Input::new(&form.name).small()),
            )
            .child(div().text_xs().text_color(muted).child(
                "An agent does it once, in the browser or on the phone. What it does becomes the steps, which run again after that without AI.",
            ))
            .children(self.message.clone().filter(|(_, bad)| *bad).map(|(message, _)| {
                div().text_xs().text_color(theme.danger).child(message)
            }))
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("test-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.creating = None;
                                this.message = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("test-create")
                            .small()
                            .primary()
                            .label("Record with agent")
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_is_an_address_or_an_app() {
        assert_eq!(
            target_of(" localhost:3000/login ", false),
            Some(Target::Web {
                url: "http://localhost:3000/login".into()
            })
        );
        assert_eq!(
            target_of("https://app.example", false),
            Some(Target::Web {
                url: "https://app.example".into()
            })
        );
        assert_eq!(
            target_of("com.example.app", true),
            Some(Target::Android {
                package: "com.example.app".into()
            })
        );
        assert_eq!(target_of("http://localhost:3000", true), None);
        assert_eq!(target_of("my app", true), None);
        assert_eq!(target_of("", false), None);
        assert!(is_page("http://localhost:3000") && !is_page("about:blank"));
    }
}
