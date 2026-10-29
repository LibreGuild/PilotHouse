//! Connecting a device: by its IP address (a phone already listening on
//! TCP, `adb tcpip 5555`), by pairing code (Android 11+ "Wireless
//! debugging"), or by switching a phone on USB to Wi-Fi in one step, as
//! Android Studio's ADB Wi-Fi plugin does.

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    notification::Notification,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::manager::DeviceManager;

/// adb's usual TCP port on a phone.
const ADB_PORT: u16 = 5555;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    Address,
    Pairing,
    Usb,
}

impl Way {
    const ALL: [Way; 3] = [Way::Address, Way::Usb, Way::Pairing];

    fn label(self) -> &'static str {
        match self {
            Way::Address => "IP address",
            Way::Usb => "USB to Wi-Fi",
            Way::Pairing => "Pairing code",
        }
    }
}

struct ConnectForm {
    manager: Entity<DeviceManager>,
    way: Way,
    address: Entity<InputState>,
    pair_address: Entity<InputState>,
    code: Entity<InputState>,
    connect_address: Entity<InputState>,
    error: Option<SharedString>,
}

impl ConnectForm {
    /// Phones on USB that can be moved to Wi-Fi.
    fn usb_devices(&self, cx: &App) -> Vec<(String, String)> {
        self.manager
            .read(cx)
            .devices()
            .iter()
            .filter(|device| {
                !device.emulator && device.is_ready() && !crate::adb::is_wireless(&device.serial)
            })
            .map(|device| (device.serial.clone(), device.name.clone()))
            .collect()
    }
}

impl Render for ConnectForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |text: &'static str| div().text_xs().text_color(muted).child(text);
        let selected = Way::ALL
            .iter()
            .position(|way| *way == self.way)
            .unwrap_or(0);
        let body = match self.way {
            Way::Address => v_flex()
                .gap_3()
                .child(div().text_sm().child(
                    "The phone (or another computer's adb) must be listening on the network: on the same Wi-Fi, after `adb tcpip 5555`, or with Wireless debugging on.",
                ))
                .child(
                    v_flex()
                        .gap_1()
                        .child(label("IP address, and the port if it is not 5555"))
                        .child(Input::new(&self.address)),
                ),
            Way::Usb => {
                let phones = self.usb_devices(cx);
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(
                        "Connect the phone with a cable once, with USB debugging on and the phone on the same Wi-Fi as this computer. PilotHouse switches it to Wi-Fi; then the cable can go.",
                    ))
                    .when(phones.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("No phone is connected by USB."),
                        )
                    })
                    .children(phones.into_iter().map(|(serial, name)| {
                        let entity = cx.entity().downgrade();
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().truncate().child(name))
                                    .child(div().text_xs().text_color(muted).child(serial.clone())),
                            )
                            .child(
                                Button::new(SharedString::from(format!("to-wifi-{serial}")))
                                    .small()
                                    .primary()
                                    .label("Switch to Wi-Fi")
                                    .on_click(move |_, window, cx| {
                                        _ = entity.update(cx, |form, cx| {
                                            switch_to_wifi(form.manager.clone(), serial.clone(), window, cx)
                                        });
                                        window.close_dialog(cx);
                                    }),
                            )
                    }))
            }
            Way::Pairing => v_flex()
                .gap_3()
                .child(div().text_sm().child(
                    "On the phone (Android 11 or later), open Developer options → Wireless debugging → Pair device with pairing code.",
                ))
                .child(
                    v_flex()
                        .gap_1()
                        .child(label("Pairing address and code (shown with the code)"))
                        .child(Input::new(&self.pair_address))
                        .child(Input::new(&self.code)),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(label("IP address and port (under “Wireless debugging”)"))
                        .child(Input::new(&self.connect_address)),
                ),
        };
        v_flex()
            .gap_4()
            .child(
                TabBar::new("connect-ways")
                    .segmented()
                    .small()
                    .selected_index(selected)
                    .on_click(cx.listener(|form, ix: &usize, _, cx| {
                        form.way = Way::ALL[*ix];
                        form.error = None;
                        cx.notify();
                    }))
                    .children(Way::ALL.map(|way| Tab::new().label(way.label()))),
            )
            .child(body)
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

/// `input` as an address, with adb's usual port when none is given.
fn with_port(input: &str) -> String {
    // `[fe80::1]:5555` and `host:5555` have one; `host` and `[fe80::1]` not.
    let has_port = match input.strip_prefix('[') {
        Some(rest) => rest.contains("]:"),
        None => input.contains(':'),
    };
    if has_port {
        input.to_string()
    } else {
        format!("{input}:{ADB_PORT}")
    }
}

/// Moves `serial` (on USB) to Wi-Fi and selects it there.
fn switch_to_wifi(
    manager: Entity<DeviceManager>,
    serial: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(adb) = manager.read(cx).adb().cloned() else {
        return;
    };
    manager.update(cx, |manager, cx| manager.ensure_tracking(cx));
    window.push_notification(Notification::info("Switching the phone to Wi-Fi…"), cx);
    let task = cx.background_spawn(async move { adb.switch_to_wifi(&serial, ADB_PORT) });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                let notice = match result {
                    Ok(address) => {
                        manager.update(cx, |manager, cx| manager.select(&address, cx));
                        Notification::success(format!(
                            "Connected over Wi-Fi ({address}). The cable can go."
                        ))
                    }
                    Err(err) => Notification::error(format!("{err:#}")),
                };
                window.push_notification(notice, cx);
            });
        })
        .detach();
}

pub fn open(manager: Entity<DeviceManager>, window: &mut Window, cx: &mut App) {
    let input = |placeholder: &'static str, window: &mut Window, cx: &mut App| {
        cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
    };
    let address = input("192.168.1.20", window, cx);
    let pair_address = input("Pairing address, e.g. 192.168.1.20:41231", window, cx);
    let code = input("Six-digit pairing code", window, cx);
    let connect_address = input("192.168.1.20:37123", window, cx);
    // A phone on USB is the easy way in.
    let way = if manager.read(cx).devices().iter().any(|device| {
        !device.emulator && device.is_ready() && !crate::adb::is_wireless(&device.serial)
    }) {
        Way::Usb
    } else {
        Way::Address
    };
    let form = cx.new(|_| ConnectForm {
        manager: manager.clone(),
        way,
        address: address.clone(),
        pair_address,
        code,
        connect_address,
        error: None,
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let (form, manager) = (form.clone(), manager.clone());
        dialog
            .title("Connect a device")
            .w(px(500.))
            .child(form.clone())
            .footer(ph_ui::dialog_buttons("Connect", true))
            .on_ok(move |_, window, cx| {
                let (way, address, pair_address, code) = {
                    let form = form.read(cx);
                    let value =
                        |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
                    match form.way {
                        Way::Address => (
                            form.way,
                            with_port(&value(&form.address)),
                            String::new(),
                            String::new(),
                        ),
                        Way::Pairing => (
                            form.way,
                            value(&form.connect_address),
                            value(&form.pair_address),
                            value(&form.code),
                        ),
                        Way::Usb => (form.way, String::new(), String::new(), String::new()),
                    }
                };
                // USB phones switch with their own button.
                if way == Way::Usb {
                    return true;
                }
                let error = if !valid_address(&address) {
                    Some("Enter the phone's address, like 192.168.1.20 or 192.168.1.20:37123.")
                } else if way == Way::Pairing
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

    #[test]
    fn an_address_without_a_port_gets_adbs() {
        assert_eq!(with_port("192.168.1.20"), "192.168.1.20:5555");
        assert_eq!(with_port("192.168.1.20:37123"), "192.168.1.20:37123");
        assert_eq!(with_port("[fe80::1]"), "[fe80::1]:5555");
        assert_eq!(with_port("[fe80::1]:6000"), "[fe80::1]:6000");
    }
}
