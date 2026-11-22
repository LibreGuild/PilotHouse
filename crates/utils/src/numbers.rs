//! Numbers in decimal, hex, octal and binary, as signed and unsigned
//! integers of each width, and the character a number stands for.

use anyhow::{Context as _, Result, bail};

use crate::Input;

/// The number `text` writes: `255`, `0xff`, `0o377`, `0b1111_1111`, `-12`,
/// `1,000,000`. Digits with a letter from a to f and no prefix are hex.
pub fn read(text: &str) -> Result<i128> {
    let clean: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, '_' | ',' | ' ' | '\''))
        .collect();
    if clean.is_empty() {
        bail!("Type a number: 255, 0xff, 0o377 or 0b11111111.");
    }
    let (negative, digits) = match clean.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, clean.as_str()),
    };
    let lower = digits.to_ascii_lowercase();
    let (radix, digits) = if let Some(rest) = lower.strip_prefix("0x") {
        (16, rest.to_string())
    } else if let Some(rest) = lower.strip_prefix('#') {
        (16, rest.to_string())
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (2, rest.to_string())
    } else if let Some(rest) = lower.strip_prefix("0o") {
        (8, rest.to_string())
    } else if lower.chars().any(|c| matches!(c, 'a'..='f')) {
        (16, lower.clone())
    } else {
        (10, lower.clone())
    };
    let value = u128::from_str_radix(&digits, radix)
        .with_context(|| format!("`{text}` is not a number in base {radix}"))?;
    let value = i128::try_from(value).context("Too big: up to 128 bits")?;
    Ok(if negative { -value } else { value })
}

/// `value` in each base, then as each width holds it (two's complement
/// for negative numbers), then as a character.
pub fn forms(value: i128) -> Vec<(String, String)> {
    let bits = |value: u128, width: u32| -> String {
        let text = format!("{value:0width$b}", width = width as usize);
        grouped(&text, 4, ' ')
    };
    let mut out = vec![
        ("Decimal".to_string(), grouped_decimal(value)),
        (
            "Hex".to_string(),
            if value < 0 {
                format!("-0x{:X}", value.unsigned_abs())
            } else {
                format!("0x{value:X}")
            },
        ),
        (
            "Octal".to_string(),
            if value < 0 {
                format!("-0o{:o}", value.unsigned_abs())
            } else {
                format!("0o{value:o}")
            },
        ),
    ];
    if value >= 0 {
        let magnitude = value as u128;
        let width = (128 - magnitude.leading_zeros()).max(1);
        let width = width.div_ceil(8) * 8;
        out.push(("Binary".to_string(), bits(magnitude, width)));
    } else {
        // How each width stores it, as long as it fits.
        for width in [8u32, 16, 32, 64] {
            let low = -(1i128 << (width - 1));
            if value >= low {
                let stored = (value as u128) & ((1u128 << width) - 1);
                out.push((
                    format!("{width}-bit"),
                    format!(
                        "0x{stored:0digits$X} · {}",
                        bits(stored, width),
                        digits = (width / 4) as usize
                    ),
                ));
            }
        }
    }
    if value >= 0 {
        // As a signed number of the narrowest width whose top bit it sets.
        for width in [8u32, 16, 32, 64] {
            let top = 1i128 << (width - 1);
            if value >= top && value < (top << 1) {
                out.push((format!("As i{width}"), grouped_decimal(value - (top << 1))));
                break;
            }
        }
    }
    if let Ok(code) = u32::try_from(value)
        && let Some(c) = char::from_u32(code)
    {
        let shown = if c.is_control() || c == ' ' {
            format!("U+{code:04X} (not printable)")
        } else {
            format!("{c}  U+{code:04X}")
        };
        out.push(("Character".to_string(), shown));
    }
    out
}

fn grouped_decimal(value: i128) -> String {
    let digits = value.unsigned_abs().to_string();
    let grouped = grouped(&digits, 3, ',');
    if value < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// `text` in groups of `size` from the right.
fn grouped(text: &str, size: usize, separator: char) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (ix, c) in chars.iter().enumerate() {
        if ix > 0 && (chars.len() - ix).is_multiple_of(size) {
            out.push(separator);
        }
        out.push(*c);
    }
    out
}

/// A number in every base.
pub fn numbers(input: &Input) -> Result<String> {
    let value = read(&input.text)?;
    Ok(forms(value)
        .into_iter()
        .map(|(name, text)| format!("{name}: {text}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_base() {
        assert_eq!(read("255").unwrap(), 255);
        assert_eq!(read("0xFF").unwrap(), 255);
        assert_eq!(read("ff").unwrap(), 255);
        assert_eq!(read("#ff").unwrap(), 255);
        assert_eq!(read("0b1111_1111").unwrap(), 255);
        assert_eq!(read("0o377").unwrap(), 255);
        assert_eq!(read("1,000,000").unwrap(), 1_000_000);
        assert_eq!(read("-12").unwrap(), -12);
        assert!(read("0b102").is_err());
        assert!(read("").is_err());
    }

    #[test]
    fn shows_widths_and_characters() {
        let forms = |value: i128| -> Vec<(String, String)> { forms(value) };
        let of = |value: i128, name: &str| {
            forms(value)
                .into_iter()
                .find(|(label, _)| label == name)
                .map(|(_, text)| text)
        };
        assert_eq!(of(255, "Binary").as_deref(), Some("1111 1111"));
        assert_eq!(of(255, "As i8").as_deref(), Some("-1"));
        assert_eq!(of(65, "Character").as_deref(), Some("A  U+0041"));
        assert_eq!(of(-1, "8-bit").as_deref(), Some("0xFF · 1111 1111"));
        assert_eq!(of(1_234_567, "Decimal").as_deref(), Some("1,234,567"));
        assert!(of(-200, "8-bit").is_none());
        assert!(forms(-200).iter().any(|(label, _)| label == "16-bit"));
    }
}
