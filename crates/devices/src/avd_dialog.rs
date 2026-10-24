//! The dialogs for making, wiping and deleting an emulator.

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputState},
    menu::PopupMenuItem,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::avd::{Profile, SystemImage};
use crate::manager::DeviceManager;
use ph_ui::MenuButton as _;

/// The new-emulator form: a name, a system image and a screen.
struct NewEmulator {
    name: Entity<InputState>,
    images: Vec<SystemImage>,
    image: usize,
    profile: Profile,
    error: Option<SharedString>,
}

impl Render for NewEmulator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |text: &'static str| div().text_xs().text_color(muted).child(text);
        if self.images.is_empty() {
            return v_flex().gap_2().child(
                div().text_sm().child(
                    "No Android system images are installed. Install one in Android Studio (Tools → SDK Manager → SDK Platforms, “Show package details”), then come back.",
                ),
            );
        }
        let form = cx.weak_entity();
        let current = self.images[self.image].label();
        let images: Vec<(usize, String)> = self
            .images
            .iter()
            .map(SystemImage::label)
            .enumerate()
            .collect();
        let selected = self.image;
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Name"))
                    .child(Input::new(&self.name)),
            )
            .child(
                v_flex().gap_1().child(label("System image")).child(
                    Button::new("avd-image")
                        .outline()
                        .small()
                        .label(current)
                        .dropdown_caret(true)
                        .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                            images.iter().fold(menu, |menu, (ix, image)| {
                                let (form, ix) = (form.clone(), *ix);
                                menu.item(
                                    PopupMenuItem::new(image.clone())
                                        .checked(ix == selected)
                                        .on_click(move |_, _, cx| {
                                            _ = form.update(cx, |form, cx| {
                                                form.image = ix;
                                                cx.notify();
                                            });
                                        }),
                                )
                            })
                        }),
                ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Screen"))
                    .child(
                        h_flex()
                            .gap_2()
                            .children(Profile::ALL.into_iter().map(|profile| {
                                Button::new(SharedString::from(profile.label()))
                                    .small()
                                    .when(self.profile == profile, |this| this.primary())
                                    .when(self.profile != profile, |this| this.outline())
                                    .label(profile.label())
                                    .on_click(cx.listener(move |form, _, _, cx| {
                                        form.profile = profile;
                                        cx.notify();
                                    }))
                            })),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
    }
}

/// Opens the new-emulator dialog; OK makes it and starts it.
pub fn open_new(manager: Entity<DeviceManager>, window: &mut Window, cx: &mut App) {
    let images = manager.read(cx).system_images();
    let default_name = images
        .first()
        .map(|image| format!("Phone API {}", image.api))
        .unwrap_or_else(|| "Phone".into());
    let name = cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_value(default_name, window, cx);
        input
    });
    let has_images = !images.is_empty();
    let form = cx.new(|_| NewEmulator {
        name: name.clone(),
        images,
        image: 0,
        profile: Profile::Phone,
        error: None,
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let (form, manager) = (form.clone(), manager.clone());
        dialog
            .title("New emulator")
            .w(px(460.))
            .child(form.clone())
            .footer(ph_ui::dialog_buttons(
                if has_images {
                    "Create and start"
                } else {
                    "Close"
                },
                has_images,
            ))
            .on_ok(move |_, window, cx| {
                if !has_images {
                    return true;
                }
                let (display, image, profile) = {
                    let form = form.read(cx);
                    let display = form.name.read(cx).value().to_string();
                    (display, form.images[form.image].clone(), form.profile)
                };
                let made = manager.update(cx, |manager, cx| {
                    manager.create_avd(&display, &image, profile, cx)
                });
                match made {
                    Ok(avd) => {
                        manager
                            .update(cx, |manager, cx| manager.start_emulator(&avd, cx))
                            .detach();
                        window.push_notification(
                            Notification::success(format!("Made {display}; it is starting.")),
                            cx,
                        );
                        true
                    }
                    Err(err) => {
                        form.update(cx, |form, cx| {
                            form.error = Some(format!("{err:#}").into());
                            cx.notify();
                        });
                        false
                    }
                }
            })
    });
    name.update(cx, |input, cx| input.focus(window, cx));
}

/// Asks, then erases `avd`'s data.
pub fn confirm_wipe(
    manager: Entity<DeviceManager>,
    avd: String,
    window: &mut Window,
    cx: &mut App,
) {
    let shown = avd.replace('_', " ");
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let (manager, avd, shown) = (manager.clone(), avd.clone(), shown.clone());
        dialog
            .confirm()
            .title(format!("Wipe {shown}?"))
            .description("Its apps, files and settings are erased, and it starts like new. Its configuration stays. This cannot be undone.")
            .button_props(DialogButtonProps::default().ok_text("Wipe data"))
            .on_ok(move |_, window, cx| {
                let notice = match manager.update(cx, |manager, _| manager.wipe_avd(&avd)) {
                    Ok(freed) => Notification::success(format!(
                        "Wiped {shown} ({} freed).",
                        crate::devices_panel::human_size(freed)
                    )),
                    Err(err) => Notification::error(format!("{err:#}")),
                };
                window.push_notification(notice, cx);
                true
            })
    });
}

/// Asks, then removes `avd` from this computer.
pub fn confirm_delete(
    manager: Entity<DeviceManager>,
    avd: String,
    window: &mut Window,
    cx: &mut App,
) {
    let shown = avd.replace('_', " ");
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let (manager, avd, shown) = (manager.clone(), avd.clone(), shown.clone());
        dialog
            .confirm()
            .title(format!("Delete {shown}?"))
            .description("The emulator and everything on it are removed from this computer. The system image stays installed. This cannot be undone.")
            .button_props(DialogButtonProps::default().ok_text("Delete emulator"))
            .on_ok(move |_, window, cx| {
                let notice = match manager.update(cx, |manager, cx| manager.delete_avd(&avd, cx)) {
                    Ok(()) => Notification::success(format!("Deleted {shown}.")),
                    Err(err) => Notification::error(format!("{err:#}")),
                };
                window.push_notification(notice, cx);
                true
            })
    });
}
