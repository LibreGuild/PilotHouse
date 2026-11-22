//! Connecting a device over the network, on one page, the easiest way
//! first:
//!
//! - **Saved**: devices connected before, each a click away again (an
//!   office's test devices on fixed addresses are not typed in every day).
//!   They can be given names of their own and forgotten.
//! - **On this network**: phones with Wireless debugging on, which adb
//!   finds by itself.
//! - **On USB**: a phone on a cable is switched to Wi-Fi in one step, as
//!   Android Studio's ADB Wi-Fi plugin does.
//! - **By address**: a device already listening (`adb tcpip 5555`, or
//!   Wireless debugging).
//! - **Pair with a code**: the first time a phone is used over Wi-Fi
//!   (Android 11 and later).
//!
//! The dialog stays open until a device is connected: what went wrong is
//! said in it, in words that say what to do, and nothing typed is lost.

use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    input::{Input, InputState},
    menu::PopupMenuItem,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
};
use ph_ui::{Icons, MenuButton as _};

use crate::adb::{Link, Nearby};
use crate::manager::{DeviceManager, SavedDevice};

/// adb's usual TCP port on a phone.
const ADB_PORT: u16 = 5555;
/// How often adb is asked which phones it sees, while the dialog is open.
const LOOK_AGAIN: Duration = Duration::from_secs(3);

/// What the dialog is doing, or how the last thing went.
#[derive(Clone, PartialEq)]
enum Progress {
    Idle,
    Working(SharedString),
    Failed(SharedString),
}

struct ConnectForm {
    manager: Entity<DeviceManager>,
    address: Entity<InputState>,
    /// The pairing fields show.
    pairing: bool,
    pair_address: Entity<InputState>,
    code: Entity<InputState>,
    /// Where the paired phone listens, when adb does not find it itself.
    connect_address: Entity<InputState>,
    /// Phones adb sees on the network.
    nearby: Vec<Nearby>,
    progress: Progress,
    /// The saved device being named, by its address, and the name typed.
    renaming: Option<(String, Entity<InputState>)>,
    _look: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectForm {
    fn new(manager: Entity<DeviceManager>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let address = input("192.168.1.20 or 192.168.1.20:5555", window, cx);
        let pair_address = input("Pairing address, e.g. 192.168.1.20:41231", window, cx);
        let code = input("Six-digit pairing code", window, cx);
        let connect_address = input(
            "Address under “Wireless debugging”, if it is not found",
            window,
            cx,
        );
        let subscriptions = vec![cx.observe(&manager, |_, _, cx| cx.notify())];
        // adb is asked which phones it sees for as long as the dialog is
        // open (the task goes with it).
        let adb = manager.read(cx).adb().cloned();
        let look = cx.spawn(async move |form, cx| {
            let Some(adb) = adb else { return };
            loop {
                let asked = adb.clone();
                let nearby = cx.background_spawn(async move { asked.nearby() }).await;
                let seen = form.update(cx, |form, cx| {
                    if form.nearby != nearby {
                        form.nearby = nearby;
                        cx.notify();
                    }
                });
                if seen.is_err() {
                    return;
                }
                cx.background_executor().timer(LOOK_AGAIN).await;
            }
        });
        Self {
            manager,
            address,
            pairing: false,
            pair_address,
            code,
            connect_address,
            nearby: Vec::new(),
            progress: Progress::Idle,
            renaming: None,
            _look: look,
            _subscriptions: subscriptions,
        }
    }

    /// Enter was pressed: it does what the field being filled in is for.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let filled = |input: &Entity<InputState>| !input.read(cx).value().trim().is_empty();
        if self.renaming.is_some() {
            self.save_name(cx);
        } else if self.pairing && filled(&self.code) {
            self.pair(window, cx);
        } else if filled(&self.address) {
            self.connect_typed(window, cx);
        }
    }

    fn busy(&self) -> bool {
        matches!(self.progress, Progress::Working(_))
    }

    fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.progress = Progress::Failed(message.into());
        cx.notify();
    }

    /// Connects to `address`; the dialog closes once it has.
    fn connect(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        if !valid_address(&address) {
            self.fail(
                "Enter the device's address, like 192.168.1.20 or 192.168.1.20:37123.",
                cx,
            );
            return;
        }
        self.progress = Progress::Working(format!("Connecting to {address}…").into());
        cx.notify();
        let connecting = self
            .manager
            .update(cx, |manager, cx| manager.connect(&address, cx));
        cx.spawn_in(window, async move |form, cx| {
            let connected = connecting.await;
            let _ = form.update_in(cx, |form, window, cx| match connected {
                Ok(serial) => form.connected(&serial, window, cx),
                Err(err) => form.fail(explain(&address, &format!("{err:#}")), cx),
            });
        })
        .detach();
    }

    /// The device is connected: says so and closes.
    fn connected(&mut self, serial: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.progress = Progress::Idle;
        window.push_notification(Notification::success(format!("Connected to {serial}.")), cx);
        window.close_dialog(cx);
    }

    fn connect_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.address.read(cx).value().trim().to_string();
        self.connect(with_port(&typed), window, cx);
    }

    /// Pairs with the code typed, then connects: to the address typed for
    /// it, else to where adb finds the phone listening.
    fn pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let (pair_address, code, typed) = (
            value(&self.pair_address),
            value(&self.code),
            value(&self.connect_address),
        );
        if !valid_address(&pair_address)
            || code.is_empty()
            || !code.chars().all(|c| c.is_ascii_digit())
        {
            self.fail(
                "Enter the pairing address and the six-digit code the phone shows.",
                cx,
            );
            return;
        }
        let Some(adb) = self.manager.read(cx).adb().cloned() else {
            return;
        };
        self.progress = Progress::Working(format!("Pairing with {pair_address}…").into());
        cx.notify();
        let host = host_of(&pair_address).to_string();
        cx.spawn_in(window, async move |form, cx| {
            let (pairing_adb, at, with) = (adb.clone(), pair_address.clone(), code.clone());
            let paired = cx
                .background_spawn(async move { pairing_adb.pair(&at, &with) })
                .await;
            if let Err(err) = paired {
                let _ = form.update(cx, |form, cx| {
                    form.fail(explain(&pair_address, &format!("{err:#}")), cx)
                });
                return;
            }
            // Paired: the phone now listens on another port of the same
            // address, which adb sees a moment later.
            let found = if typed.is_empty() {
                cx.background_spawn(async move {
                    (0..6).find_map(|attempt| {
                        if attempt > 0 {
                            std::thread::sleep(Duration::from_millis(500));
                        }
                        adb.nearby()
                            .into_iter()
                            .find(|phone| !phone.pairing && host_of(&phone.address) == host)
                            .map(|phone| phone.address)
                    })
                })
                .await
            } else {
                Some(with_port(&typed))
            };
            let _ = form.update_in(cx, |form, window, cx| {
                form.progress = Progress::Idle;
                match found {
                    Some(address) => form.connect(address, window, cx),
                    None => form.fail(
                        "Paired. Now enter the address the phone shows under “Wireless debugging” and pair again, or connect to it by address.",
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    /// Moves the phone `serial` (on USB) to Wi-Fi and chooses it there.
    fn switch_to_wifi(&mut self, serial: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let Some(adb) = self.manager.read(cx).adb().cloned() else {
            return;
        };
        self.manager
            .update(cx, |manager, cx| manager.ensure_tracking(cx));
        self.progress = Progress::Working("Switching the phone to Wi-Fi…".into());
        cx.notify();
        cx.spawn_in(window, async move |form, cx| {
            let switched = cx
                .background_spawn(async move { adb.switch_to_wifi(&serial, ADB_PORT) })
                .await;
            let _ = form.update_in(cx, |form, window, cx| match switched {
                Ok(address) => {
                    form.manager
                        .update(cx, |manager, cx| manager.select(&address, cx));
                    form.progress = Progress::Idle;
                    window.push_notification(
                        Notification::success(format!(
                            "Connected over Wi-Fi ({address}). The cable can go."
                        )),
                        cx,
                    );
                    window.close_dialog(cx);
                }
                Err(err) => form.fail(format!("{err:#}"), cx),
            });
        })
        .detach();
    }

    /// Starts naming the saved device at `address`.
    fn rename(&mut self, saved: &SavedDevice, window: &mut Window, cx: &mut Context<Self>) {
        let title = saved.title().to_string();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("A name for this device")
                .default_value(title)
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.renaming = Some((saved.address.clone(), input));
        cx.notify();
    }

    fn save_name(&mut self, cx: &mut Context<Self>) {
        if let Some((address, input)) = self.renaming.take() {
            let name = input.read(cx).value().to_string();
            self.manager
                .update(cx, |manager, cx| manager.rename(&address, &name, cx));
            cx.notify();
        }
    }

    /// One device of a list: what it is, where, and what can be done.
    fn row(
        &self,
        icon: Icons,
        title: impl IntoElement,
        detail: SharedString,
        action: impl IntoElement,
        cx: &App,
    ) -> gpui_kit::Div {
        h_flex()
            .gap_3()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                gpui_kit::component::Icon::new(icon)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(title))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    ),
            )
            .child(action)
    }

    fn render_saved(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let saved = self.manager.read(cx).saved().to_vec();
        if saved.is_empty() {
            return None;
        }
        let busy = self.busy();
        let rows = saved.into_iter().map(|device| {
            let connected = self.manager.read(cx).is_connected(&device);
            let naming = self
                .renaming
                .as_ref()
                .filter(|(address, _)| *address == device.address)
                .map(|(_, input)| input.clone());
            let id = SharedString::from(device.address.clone());
            let title = match &naming {
                Some(input) => Input::new(input).xsmall().into_any_element(),
                None => device.title().to_string().into_any_element(),
            };
            let detail: SharedString = match &device.label {
                Some(_) => format!("{} · {}", device.name, device.address).into(),
                None => device.address.clone().into(),
            };
            let action = h_flex()
                .gap_1()
                .map(|this| {
                    if naming.is_some() {
                        this.child(
                            Button::new(SharedString::from(format!("save-name-{id}")))
                                .small()
                                .primary()
                                .label("Save")
                                .on_click(cx.listener(|form, _, _, cx| form.save_name(cx))),
                        )
                    } else if connected {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Connected"),
                        )
                    } else {
                        let address = device.address.clone();
                        this.child(
                            Button::new(SharedString::from(format!("connect-saved-{id}")))
                                .small()
                                .outline()
                                .label("Connect")
                                .disabled(busy)
                                .on_click(cx.listener(move |form, _, window, cx| {
                                    form.connect(address.clone(), window, cx)
                                })),
                        )
                    }
                })
                .child({
                    let form = cx.entity().downgrade();
                    let device = device.clone();
                    Button::new(SharedString::from(format!("saved-more-{id}")))
                        .ghost()
                        .small()
                        .icon(IconName::Ellipsis)
                        .tooltip("Rename or forget this device")
                        .accessibility_label("Rename or forget this device")
                        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                            let (rename, forget) = (form.clone(), form.clone());
                            let (named, address) = (device.clone(), device.address.clone());
                            menu.item(PopupMenuItem::new("Rename").on_click(
                                move |_, window, cx| {
                                    _ = rename
                                        .update(cx, |form, cx| form.rename(&named, window, cx));
                                },
                            ))
                            .item(
                                PopupMenuItem::new("Forget").on_click(move |_, _, cx| {
                                    _ = forget.update(cx, |form, cx| {
                                        form.manager
                                            .update(cx, |manager, cx| manager.forget(&address, cx))
                                    });
                                }),
                            )
                        })
                });
            self.row(Icons::Wifi, title, detail, action, cx)
        });
        Some(section("Saved", cx).children(rows.collect::<Vec<_>>()))
    }

    /// Phones adb sees on the network that are not connected yet.
    fn render_nearby(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let manager = self.manager.read(cx);
        let connected: Vec<String> = manager
            .devices()
            .iter()
            .filter(|device| device.is_ready() && device.link() == Link::Network)
            .flat_map(|device| [device.serial.clone(), device.identity().to_string()])
            .collect();
        let saved = manager.saved().to_vec();
        let busy = self.busy();
        // A phone that waits to be paired is listed once, as that.
        let phones: Vec<&Nearby> = self
            .nearby
            .iter()
            .filter(|phone| {
                !connected.contains(&phone.address)
                    && !connected.iter().any(|known| known == phone.hardware())
                    && (phone.pairing
                        || !self
                            .nearby
                            .iter()
                            .any(|other| other.pairing && other.name == phone.name))
            })
            .collect();
        if phones.is_empty() {
            return None;
        }
        let rows = phones.into_iter().map(|phone| {
            let known = saved
                .iter()
                .find(|saved| saved.hardware == phone.hardware());
            let title = known.map_or_else(
                || format!("Phone {}", phone.hardware()),
                |saved| saved.title().to_string(),
            );
            let id = SharedString::from(phone.address.clone());
            let address = phone.address.clone();
            let action = if phone.pairing {
                Button::new(SharedString::from(format!("pair-nearby-{id}")))
                    .small()
                    .outline()
                    .label("Pair")
                    .disabled(busy)
                    .on_click(cx.listener(move |form, _, window, cx| {
                        form.pairing = true;
                        form.pair_address
                            .update(cx, |input, cx| input.set_value(address.clone(), window, cx));
                        form.code.update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    }))
            } else {
                Button::new(SharedString::from(format!("connect-nearby-{id}")))
                    .small()
                    .outline()
                    .label("Connect")
                    .disabled(busy)
                    .on_click(cx.listener(move |form, _, window, cx| {
                        form.connect(address.clone(), window, cx)
                    }))
            };
            let detail: SharedString = if phone.pairing {
                format!("{} · waiting to be paired", phone.address).into()
            } else {
                phone.address.clone().into()
            };
            self.row(Icons::Smartphone, title, detail, action, cx)
        });
        Some(section("On this network", cx).children(rows.collect::<Vec<_>>()))
    }

    /// Phones on USB that can be moved to Wi-Fi.
    fn render_usb(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let phones: Vec<(String, String)> = self
            .manager
            .read(cx)
            .devices()
            .iter()
            .filter(|device| device.is_ready() && device.link() == Link::Usb)
            .map(|device| (device.serial.clone(), device.display_name()))
            .collect();
        if phones.is_empty() {
            return None;
        }
        let busy = self.busy();
        let rows = phones.into_iter().map(|(serial, name)| {
            let action = Button::new(SharedString::from(format!("to-wifi-{serial}")))
                .small()
                .outline()
                .label("Switch to Wi-Fi")
                .disabled(busy)
                .tooltip(ph_ui::tip(
                    "The phone must be on the same Wi-Fi as this computer. Afterwards the cable can go.",
                ))
                .on_click({
                    let serial = serial.clone();
                    cx.listener(move |form, _, window, cx| {
                        form.switch_to_wifi(serial.clone(), window, cx)
                    })
                });
            self.row(Icons::Usb, name, serial.into(), action, cx)
        });
        Some(section("On USB", cx).children(rows.collect::<Vec<_>>()))
    }

    fn render_pairing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_2()
            .items_start()
            .child(
                Button::new("pair-toggle")
                    .ghost()
                    .small()
                    .icon(if self.pairing {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label("Pair with a code")
                    .on_click(cx.listener(|form, _, _, cx| {
                        form.pairing = !form.pairing;
                        cx.notify();
                    })),
            )
            .when(self.pairing, |this| {
                this.child(div().text_xs().text_color(muted).child(
                    "Once per phone (Android 11 or later): on the phone, open Developer options → Wireless debugging → Pair device with pairing code.",
                ))
                .child(div().w_full().child(Input::new(&self.pair_address).small()))
                .child(div().w_full().child(Input::new(&self.code).small()))
                .child(div().w_full().child(Input::new(&self.connect_address).small()))
                .child(
                    h_flex().child(
                        Button::new("pair")
                            .small()
                            .primary()
                            .label("Pair and connect")
                            .disabled(self.busy())
                            .on_click(cx.listener(|form, _, window, cx| form.pair(window, cx))),
                    ),
                )
            })
    }
}

/// A list's heading over its rows.
fn section(title: &'static str, cx: &App) -> gpui_kit::Div {
    v_flex().gap_1p5().child(
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(title),
    )
}

impl Render for ConnectForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let by_address = section("By address", cx)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.address).small()))
                    .child(
                        Button::new("connect-typed")
                            .small()
                            .primary()
                            .label("Connect")
                            .disabled(self.busy())
                            .on_click(cx.listener(|form, _, window, cx| {
                                form.connect_typed(window, cx)
                            })),
                    ),
            )
            .child(div().text_xs().text_color(muted).child(
                "A device that already listens on the network: after `adb tcpip 5555`, or with Wireless debugging on. Port 5555 unless one is given.",
            ));
        let status = match &self.progress {
            Progress::Idle => None,
            Progress::Working(what) => Some(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(ph_ui::Loader::new().xsmall())
                    .child(what.clone()),
            ),
            // A sentence or two, on as many lines as it takes.
            Progress::Failed(why) => Some(
                h_flex().child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(why.clone()),
                ),
            ),
        };
        v_flex()
            .id("connect-device")
            .gap_4()
            .children(self.render_saved(cx))
            .children(self.render_nearby(cx))
            .children(self.render_usb(cx))
            .child(by_address)
            .child(self.render_pairing(cx))
            .children(status)
    }
}

/// What adb said when it could not connect or pair, in words that say what
/// to do about it.
pub(crate) fn explain(address: &str, said: &str) -> String {
    let lower = said.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if has(&["wrong password", "protocol fault", "pairing"]) && !has(&["successfully"]) {
        "The phone did not take the code. It changes each time the pairing screen opens: use the address and code it shows now.".into()
    } else if has(&["refused"]) {
        format!(
            "Nothing answers at {address}. On the device, switch Wireless debugging on (or run `adb tcpip 5555` once over USB), and check the port."
        )
    } else if has(&[
        "timed out",
        "no route",
        "unreachable",
        "host is down",
        "no answer within",
    ]) {
        // macOS says "no route" too when it keeps a program off the local
        // network, which reads as the device being away.
        let allowed = if cfg!(target_os = "macos") && has(&["no route"]) {
            " If it is, allow this app under System Settings → Privacy & Security → Local Network."
        } else {
            ""
        };
        format!(
            "{address} cannot be reached. Is the device on, and on the same network as this computer?{allowed}"
        )
    } else if has(&["authenticate", "unauthorized"]) {
        "The device did not accept this computer. Pair it first (Pair with a code), or allow the prompt on its screen.".into()
    } else {
        said.to_string()
    }
}

/// The host of an `host:port`.
fn host_of(address: &str) -> &str {
    address.rsplit_once(':').map_or(address, |(host, _)| host)
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

pub fn open(manager: Entity<DeviceManager>, window: &mut Window, cx: &mut App) {
    manager.update(cx, |manager, cx| manager.ensure_tracking(cx));
    let form = cx.new(|cx| ConnectForm::new(manager, window, cx));
    let address = form.read(cx).address.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        dialog
            .title("Connect a device")
            .w(ph_ui::scaled_px(520., window))
            .child(form.clone())
            // Enter is the field's own action, and never closes the dialog:
            // only a device connected does.
            .on_ok({
                let form = form.clone();
                move |_, window, cx| {
                    form.update(cx, |form, cx| form.enter(window, cx));
                    false
                }
            })
            .footer(
                DialogFooter::new().w_full().child(div().flex_1()).child(
                    Button::new("connect-close")
                        .ghost()
                        .label("Close")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
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
        assert_eq!(host_of("192.168.1.20:37123"), "192.168.1.20");
    }

    #[test]
    fn what_adb_said_becomes_what_to_do() {
        let refused = explain(
            "10.0.0.5:5555",
            "adb connect: failed to connect to '10.0.0.5:5555': Connection refused",
        );
        assert!(refused.starts_with("Nothing answers at 10.0.0.5:5555."));
        let away = explain(
            "10.0.0.5:5555",
            "running adb connect: no answer within 15 seconds",
        );
        assert!(away.starts_with("10.0.0.5:5555 cannot be reached."));
        assert!(explain("a:1", "failed to authenticate to a:1").contains("Pair it first"));
        assert!(
            explain("a:1", "Failed: Wrong password or connection was dropped.")
                .contains("did not take the code")
        );
        // What is not known is said as adb said it.
        assert_eq!(explain("a:1", "something else"), "something else");
    }
}
