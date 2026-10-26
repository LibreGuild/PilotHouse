//! Connecting a phone over Wi-Fi: Android 11+ "Wireless debugging" pairs
//! once with a code, then connects by address; older phones that were put
//! in TCP mode (`adb tcpip 5555`) only connect.

use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    input::{Input, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::manager::DeviceManager;

struct ConnectForm {
    address: Entity<InputState>,
    pair_address: Entity<InputState>,
    code: Entity<InputState>,
    error: Option<SharedString>,
}

impl Render for ConnectForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |text: &'static str| div().text_xs().text_color(muted).child(text);
        v_flex()
            .gap_3()
            .child(div().text_sm().child(
                "On the phone, open Developer options → Wireless debugging. The phone and this computer must be on the same network.",
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(label("IP address and port (under “Wireless debugging”)"))
                    .child(Input::new(&self.address)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("First time only: “Pair device with pairing code”, its address and code"))
                    .child(Input::new(&self.pair_address))
                    .child(Input::new(&self.code)),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
    }
}

/// An `host:port` with nothing a shell or adb would read more into.
fn valid_address(address: &str) -> bool {
    // The port is after the last colon (`[fe80::1]:5555` has more).
    address.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-[]:".contains(c))
            && port.parse::<u16>().is_ok()
    })
}

pub fn open(manager: Entity<DeviceManager>, window: &mut Window, cx: &mut App) {
    let input = |placeholder: &'static str, window: &mut Window, cx: &mut App| {
        cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
    };
    let address = input("192.168.1.20:37123", window, cx);
    let pair_address = input("Pairing address, e.g. 192.168.1.20:41231", window, cx);
    let code = input("Six-digit pairing code", window, cx);
    let form = cx.new(|_| ConnectForm {
        address: address.clone(),
        pair_address,
        code,
        error: None,
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let (form, manager) = (form.clone(), manager.clone());
        dialog
            .title("Connect over Wi-Fi")
            .w(px(480.))
            .child(form.clone())
            .footer(ph_ui::dialog_buttons("Connect", true))
            .on_ok(move |_, window, cx| {
                let (address, pair_address, code) = {
                    let form = form.read(cx);
                    let value =
                        |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
                    (
                        value(&form.address),
                        value(&form.pair_address),
                        value(&form.code),
                    )
                };
                let error = if !valid_address(&address) {
                    Some("Enter the address shown on the phone, like 192.168.1.20:37123.")
                } else if (!code.is_empty() || !pair_address.is_empty())
                    && (!valid_address(&pair_address)
                        || code.is_empty()
                        || !code.chars().all(|c| c.is_ascii_digit()))
                {
                    Some("For pairing, enter the pairing address and the six-digit code.")
                } else {
                    None
                };
                if let Some(error) = error {
                    form.update(cx, |form, cx| {
                        form.error = Some(error.into());
                        cx.notify();
                    });
                    return false;
                }
                let Some(adb) = manager.read(cx).adb().cloned() else {
                    return true;
                };
                manager.update(cx, |manager, cx| manager.ensure_tracking(cx));
                window
                    .push_notification(Notification::info(format!("Connecting to {address}…")), cx);
                let task = cx.background_spawn(async move {
                    if !code.is_empty() {
                        adb.pair(&pair_address, &code)?;
                    }
                    adb.connect(&address)
                });
                let manager = manager.clone();
                window
                    .spawn(cx, async move |cx| {
                        let result = task.await;
                        let _ = cx.update(|window, cx| {
                            let notice = match result {
                                Ok(message) => {
                                    // The new device arrives through `track-devices`; pick it.
                                    let serial = message
                                        .strip_prefix("already connected to ")
                                        .or_else(|| message.strip_prefix("connected to "))
                                        .map(str::trim)
                                        .map(str::to_string);
                                    if let Some(serial) = serial {
                                        manager
                                            .update(cx, |manager, cx| manager.select(&serial, cx));
                                    }
                                    Notification::success(message)
                                }
                                Err(err) => Notification::error(format!("{err:#}")),
                            };
                            window.push_notification(notice, cx);
                        });
                    })
                    .detach();
                true
            })
    });
    address.update(cx, |input, cx| input.focus(window, cx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_plain_addresses() {
        assert!(valid_address("192.168.1.20:37123"));
        assert!(valid_address("phone.local:5555"));
        assert!(!valid_address("192.168.1.20"));
        assert!(!valid_address("1.2.3.4:5555; rm -rf"));
        assert!(!valid_address(":5555"));
        assert!(valid_address("[fe80::1]:5555"));
    }
}
