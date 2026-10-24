//! Text: regular expressions, diffs, colors and Markdown.

use anyhow::{Context as _, Result, bail};

use crate::Input;

/// Every match of the pattern in the text, with its groups.
pub fn regex(input: &Input) -> Result<String> {
    let pattern = regex::Regex::new(input.text.trim()).context("Not a valid pattern")?;
    let names: Vec<Option<&str>> = pattern.capture_names().collect();
    let matches: Vec<regex::Captures> = pattern.captures_iter(&input.second).collect();
    if input.mode == 1 {
        let json: Vec<serde_json::Value> = matches
            .iter()
            .map(|caps| {
                let groups: serde_json::Map<String, serde_json::Value> = names
                    .iter()
                    .enumerate()
                    .skip(1)
                    .filter_map(|(ix, name)| {
                        let value = caps.get(ix)?.as_str().to_string();
                        Some((name.map_or_else(|| ix.to_string(), str::to_string), value.into()))
                    })
                    .collect();
                serde_json::json!({ "match": &caps[0], "start": caps.get(0).map_or(0, |m| m.start()), "groups": groups })
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&json)?);
    }
    if matches.is_empty() {
        return Ok("No matches.".into());
    }
    let mut out = vec![format!("{} matches", matches.len())];
    for (n, caps) in matches.iter().enumerate() {
        let whole = caps.get(0).expect("group 0 is the match");
        out.push(format!(
            "{}. “{}” at {}",
            n + 1,
            whole.as_str(),
            whole.start()
        ));
        for (ix, name) in names.iter().enumerate().skip(1) {
            if let Some(group) = caps.get(ix) {
                let label = name.map_or_else(|| ix.to_string(), str::to_string);
                out.push(format!("   {label}: “{}”", group.as_str()));
            }
        }
    }
    Ok(out.join("\n"))
}

/// A unified diff from the first text to the second.
pub fn diff(input: &Input) -> Result<String> {
    let diff = similar::TextDiff::from_lines(&input.text, &input.second);
    let text = diff
        .unified_diff()
        .context_radius(3)
        .header("before", "after")
        .to_string();
    Ok(if text.is_empty() {
        "They are the same.".into()
    } else {
        text
    })
}

/// A color in hex, RGB and HSL.
pub fn color(input: &Input) -> Result<String> {
    let (r, g, b) = parse_color(input.text.trim())?;
    let (h, s, l) = to_hsl(r, g, b);
    Ok(format!(
        "#{r:02x}{g:02x}{b:02x}\nrgb({r}, {g}, {b})\nhsl({h:.0}, {:.0}%, {:.0}%)",
        s * 100.0,
        l * 100.0
    ))
}

fn parse_color(text: &str) -> Result<(u8, u8, u8)> {
    let lower = text.to_lowercase();
    if let Some(hex) = lower
        .strip_prefix('#')
        .or_else(|| (lower.len() == 6).then_some(lower.as_str()))
    {
        let hex = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect(),
            6 | 8 => hex[..6].to_string(),
            _ => bail!("A hex color has 3 or 6 digits."),
        };
        let byte = |ix: usize| u8::from_str_radix(&hex[ix..ix + 2], 16).context("Not a hex color");
        return Ok((byte(0)?, byte(2)?, byte(4)?));
    }
    let numbers = |prefix: &str| -> Option<Vec<f64>> {
        let inner = lower
            .strip_prefix(prefix)?
            .trim_start_matches('a')
            .strip_prefix('(')?
            .strip_suffix(')')?;
        inner
            .split([',', ' ', '/'])
            .filter(|part| !part.is_empty())
            .map(|part| part.trim_end_matches(['%', '°']).parse().ok())
            .collect()
    };
    if let Some(values) = numbers("rgb").filter(|values| values.len() >= 3) {
        let clamp = |value: f64| value.round().clamp(0.0, 255.0) as u8;
        return Ok((clamp(values[0]), clamp(values[1]), clamp(values[2])));
    }
    if let Some(values) = numbers("hsl").filter(|values| values.len() >= 3) {
        return Ok(from_hsl(values[0], values[1] / 100.0, values[2] / 100.0));
    }
    bail!("Give a color as #rrggbb, rgb(…) or hsl(…).")
}

fn to_hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |value: f64| ((value + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

/// GitHub-flavored Markdown to HTML.
pub fn markdown(input: &Input) -> Result<String> {
    markdown::to_html_with_options(&input.text, &markdown::Options::gfm())
        .map_err(|err| anyhow::anyhow!("{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_diff_color_markdown() {
        let found = regex(&Input {
            second: "id=12, id=7".into(),
            ..Input::new(r"id=(?P<n>\d+)")
        })
        .unwrap();
        assert!(found.starts_with("2 matches") && found.contains("n: “7”"));
        assert!(regex(&Input::new("(")).is_err());
        let changed = diff(&Input {
            second: "a\nc\n".into(),
            ..Input::new("a\nb\n")
        })
        .unwrap();
        assert!(changed.contains("-b") && changed.contains("+c"));
        assert_eq!(
            color(&Input::new("#3b82f6")).unwrap().lines().nth(1),
            Some("rgb(59, 130, 246)")
        );
        assert!(
            color(&Input::new("hsl(0, 100%, 50%)"))
                .unwrap()
                .starts_with("#ff0000")
        );
        assert_eq!(
            color(&Input::new("#fff")).unwrap().lines().next(),
            Some("#ffffff")
        );
        assert!(
            markdown(&Input::new("# Hi\n\n- [x] done"))
                .unwrap()
                .contains("<h1>Hi</h1>")
        );
    }
}
