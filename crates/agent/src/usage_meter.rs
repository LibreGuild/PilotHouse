//! The chat header's usage meter: how full the agent's context is, and
//! what the chat has used so far, from the agent's own reports.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, div, px,
};
use ph_store::usage::SessionUsage;

/// `950`, `12.3k`, `1.2M`.
pub fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => trim(format!("{:.1}", n as f64 / 1_000.0)) + "k",
        _ => trim(format!("{:.1}", n as f64 / 1_000_000.0)) + "M",
    }
}

fn trim(number: String) -> String {
    number
        .strip_suffix(".0")
        .map(str::to_string)
        .unwrap_or(number)
}

/// `$0.45`, or `0.45 EUR`.
pub fn money(amount: f64, currency: Option<&str>) -> String {
    match currency {
        None | Some("USD") => format!("${amount:.2}"),
        Some(currency) => format!("{amount:.2} {currency}"),
    }
}

/// What the tooltip says.
pub fn describe(usage: &SessionUsage) -> String {
    let mut lines = Vec::new();
    if let Some((used, size)) = usage.context.filter(|(_, size)| *size > 0) {
        lines.push(format!(
            "Context: {} of {} tokens ({}%)",
            compact(used),
            compact(size),
            used * 100 / size
        ));
    }
    if usage.input_tokens + usage.output_tokens > 0 {
        let mut tokens = format!(
            "This chat: {} in, {} out",
            compact(usage.input_tokens),
            compact(usage.output_tokens)
        );
        if usage.cached_tokens > 0 {
            tokens.push_str(&format!(" ({} cached)", compact(usage.cached_tokens)));
        }
        lines.push(tokens);
    }
    if usage.cost > 0.0 {
        lines.push(format!(
            "Cost so far: {}",
            money(usage.cost, usage.currency.as_deref())
        ));
    }
    lines.join("\n")
}

/// A small context bar with the share used, or the token count; `None`
/// when the agent reported nothing.
pub fn render(usage: Option<&SessionUsage>, cx: &App) -> Option<impl IntoElement> {
    let usage = usage?;
    let details = describe(usage);
    if details.is_empty() {
        return None;
    }
    let share = usage
        .context
        .filter(|(_, size)| *size > 0)
        .map(|(used, size)| (used as f32 / size as f32).clamp(0.0, 1.0));
    let label = match share {
        Some(share) => format!("{:.0}%", share * 100.0),
        None => compact(usage.input_tokens + usage.output_tokens),
    };
    let fill = match share {
        Some(share) if share > 0.9 => cx.theme().danger,
        Some(share) if share > 0.7 => cx.theme().warning,
        _ => cx.theme().info,
    };
    Some(
        div()
            .id("chat-usage")
            .flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .when_some(share, |this, share| {
                this.child(
                    div()
                        .w(px(36.))
                        .h(px(4.))
                        .rounded_full()
                        .bg(cx.theme().muted)
                        .child(div().h_full().rounded_full().bg(fill).w(px(36. * share))),
                )
            })
            .child(label)
            .when(usage.cost > 0.0, |this| {
                this.child(money(usage.cost, usage.currency.as_deref()))
            })
            .tooltip(move |window, cx| Tooltip::new(details.clone()).build(window, cx)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_short() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_340), "12.3k");
        assert_eq!(compact(2_000), "2k");
        assert_eq!(compact(1_250_000), "1.2M");
        assert_eq!(money(0.4512, None), "$0.45");
        assert_eq!(money(3.0, Some("EUR")), "3.00 EUR");
    }

    #[test]
    fn the_tooltip_says_what_was_reported() {
        let usage = SessionUsage {
            input_tokens: 12_000,
            output_tokens: 800,
            cost: 0.12,
            context: Some((50_000, 200_000)),
            ..Default::default()
        };
        assert_eq!(
            describe(&usage),
            "Context: 50k of 200k tokens (25%)\nThis chat: 12k in, 800 out\nCost so far: $0.12"
        );
        assert_eq!(describe(&SessionUsage::default()), "");
    }
}
