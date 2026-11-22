//! Sharing an artifact with people on the same network: the dialog where it
//! is switched on, its link is copied, and it is switched off again.
//!
//! It says what sharing is before anything is opened (who can come, for how
//! long, that it is not the internet), and nothing is shared until *Share*
//! is pressed. Once shared, the dialog shows the link, how many have the
//! page open, and the one choice there is: whether they take part or only
//! look. *Stop sharing* is one press, here and nowhere hidden.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div,
};
use ph_ui::Icons;

use crate::hub::{Artifacts, Sharing};

pub struct ShareView {
    artifacts: Entity<Artifacts>,
    id: String,
    /// What guests may do, as chosen before it is shared.
    take_part: bool,
    /// Sharing is being opened or changed.
    busy: bool,
    error: Option<SharedString>,
    _artifacts: Subscription,
}

impl ShareView {
    /// Opens the dialog for artifact `id`.
    pub fn open(
        artifacts: Entity<Artifacts>,
        id: &str,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        // The link carries this computer's address, which changes with the
        // network: asked anew each time the dialog opens.
        artifacts.update(cx, |artifacts, cx| artifacts.refresh_address(cx));
        let view = cx.new(|cx| Self {
            _artifacts: cx.observe(&artifacts, |_, _, cx| cx.notify()),
            artifacts,
            id: id.to_string(),
            take_part: true,
            busy: false,
            error: None,
        });
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title("Share on your network")
                .w(ph_ui::scaled_px(520., window))
                // Enter is not "OK": the dialog has its own buttons.
                .on_ok(|_, _, _| false)
                .child(view.clone())
        });
    }

    fn sharing(&self, cx: &Context<Self>) -> Option<Sharing> {
        self.artifacts.read(cx).sharing(&self.id)
    }

    /// Shares it, or changes how it is shared.
    fn share(&mut self, take_part: bool, new_link: bool, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        let shared = self.artifacts.update(cx, |artifacts, cx| {
            artifacts.share(&self.id, take_part, new_link, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = shared.await;
            _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.error = result.err().map(|err| format!("{err:#}").into());
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_take_part(&mut self, take_part: bool, cx: &mut Context<Self>) {
        self.take_part = take_part;
        // Shared already: it holds from now on, for those who are here too.
        if self.sharing(cx).is_some() {
            self.share(take_part, false, cx);
        }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.artifacts
            .update(cx, |artifacts, cx| artifacts.unshare(&self.id, cx));
        cx.notify();
    }
}

/// How many have the page open, in words.
fn here(guests: usize) -> String {
    match guests {
        0 => "Nobody has it open yet".into(),
        1 => "1 person has it open".into(),
        n => format!("{n} people have it open"),
    }
}

impl Render for ShareView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = ph_brand::NAME;
        let sharing = self.sharing(cx);
        let title = self
            .artifacts
            .read(cx)
            .find(&self.id)
            .map(|artifact| artifact.manifest.title.clone())
            .unwrap_or_default();
        let (muted, success, border, radius, danger, warning) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.success,
                theme.border,
                theme.radius,
                theme.danger,
                theme.warning,
            )
        };
        let take_part = sharing
            .as_ref()
            .map_or(self.take_part, |sharing| sharing.take_part);
        let choice = v_flex()
            .gap_0p5()
            .child(
                Checkbox::new("share-take-part")
                    .checked(take_part)
                    .label("They can take part")
                    .disabled(self.busy)
                    .on_click(
                        cx.listener(|this, checked: &bool, _, cx| this.set_take_part(*checked, cx)),
                    ),
            )
            .child(
                div()
                    .pl_6()
                    .text_xs()
                    .text_color(muted)
                    .child("Play, vote, write what the page shares. Off: they only look."),
            );
        v_flex()
            .id("artifact-share")
            .gap_3()
            .text_sm()
            .map(|this| match &sharing {
                None => this
                    .child(div().child(format!(
                        "People on your network who have the link can open “{title}” while {name} is running on this computer. It is not put on the internet."
                    )))
                    .child(choice),
                Some(sharing) => this
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_color(success)
                            .child(Icon::new(Icons::Wifi).small())
                            .child(format!("Shared · {}", here(sharing.guests))),
                    )
                    .map(|this| match &sharing.link {
                        Some(link) => {
                            let copied = link.clone();
                            this.child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("share-link")
                                            .flex_1()
                                            .min_w_0()
                                            .px_2()
                                            .py_1()
                                            .rounded(radius)
                                            .border_1()
                                            .border_color(border)
                                            .text_xs()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .truncate()
                                            .child(link.clone())
                                            .test_support(),
                                    )
                                    .child(
                                        Button::new("share-copy")
                                            .small()
                                            .primary()
                                            .label("Copy link")
                                            .on_click(move |_, _, cx| {
                                                ph_ui::copy_with_toast(
                                                    copied.clone(),
                                                    "the link",
                                                    cx,
                                                )
                                            }),
                                    ),
                            )
                        }
                        None => this.child(div().text_color(muted).child(
                            "This computer is on no network right now, so there is no link to give.",
                        )),
                    })
                    .child(choice)
                    .child(div().text_xs().text_color(muted).child(format!(
                        "What you entered in it for yourself is not shown to them. Sharing ends when you stop it, or quit {name}."
                    ))),
            })
            // Said before a link is given, not found out by a colleague.
            .when(self.artifacts.read(cx).behind_a_tunnel(), |this| {
                this.child(
                    h_flex()
                        .gap_1p5()
                        .items_start()
                        .text_xs()
                        .text_color(warning)
                        .child(Icon::new(Icons::Info).xsmall())
                        // Wraps: the sentence is longer than the dialog is wide.
                        .child(div().flex_1().min_w_0().child(
                            "A VPN is on here. It may keep people on your network from reaching this computer until it is off.",
                        )),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_color(danger).child(error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .map(|this| match &sharing {
                        None => this
                            .child(div().flex_1())
                            .child(
                                Button::new("share-cancel")
                                    .small()
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("share-start")
                                    .small()
                                    .primary()
                                    .label("Share")
                                    .loading(self.busy)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.share(this.take_part, false, cx)
                                    })),
                            ),
                        Some(sharing) => {
                            let take_part = sharing.take_part;
                            this.child(
                                // Whoever has the old link is shut out.
                                Button::new("share-new-link")
                                    .small()
                                    .ghost()
                                    .label("New link")
                                    .tooltip("The link given before stops working")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.share(take_part, true, cx)
                                    })),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("share-stop")
                                    .small()
                                    .danger()
                                    .outline()
                                    .label("Stop sharing")
                                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                            )
                            .child(
                                Button::new("share-done")
                                    .small()
                                    .label("Done")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                        }
                    }),
            )
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn says_how_many_are_here() {
        assert_eq!(super::here(0), "Nobody has it open yet");
        assert_eq!(super::here(1), "1 person has it open");
        assert_eq!(super::here(4), "4 people have it open");
    }
}
