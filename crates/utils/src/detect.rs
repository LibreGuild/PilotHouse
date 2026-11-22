//! Which utility a text is for: a token is a JWT, `{…}` is JSON, a long
//! number is a Unix time. The dialog offers it when the utility in front
//! cannot read what was pasted, and next to a decoded result.

/// The utilities that can read `text`, the likeliest first.
pub fn guess(text: &str) -> Vec<&'static str> {
    let text = text.trim();
    if text.is_empty() || text.len() > 4 << 20 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let one_line = !text.contains('\n');
    if text.contains("-----BEGIN CERTIFICATE-----") {
        found.push("x509");
    }
    if one_line && crate::encode::jwt_read(text).is_ok() {
        found.push("jwt");
    }
    if (text.starts_with('{') || text.starts_with('[') || text.starts_with('"'))
        && serde_json::from_str::<serde_json::Value>(text).is_ok()
    {
        found.push("json");
    }
    if text.starts_with('<') && roxmltree::Document::parse(text).is_ok() {
        found.push("xml");
    }
    if one_line {
        if crate::hash::read_id(text).is_ok() {
            found.push("ids");
        }
        if let Ok(parts) = crate::encode::url_parts(text)
            && matches!(parts.scheme.as_str(), "http" | "https")
        {
            found.push("url");
        }
        // A Unix time (seconds to nanoseconds), or a date written out.
        let digits = text.trim_start_matches('-');
        let unix = (9..=19).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit());
        let date = crate::time::moment(text).is_ok()
            && text.contains('-')
            && text.chars().filter(char::is_ascii_digit).count() >= 6;
        if unix || date {
            found.push("time");
        }
        if crate::time::normalized(text).is_ok()
            && crate::time::cron_next(text, 1, crate::time::Clock::Utc).is_ok()
        {
            found.push("cron");
        }
        if crate::color::parse(text).is_ok() && !text.chars().all(|c| c.is_ascii_digit()) {
            found.push("color");
        }
        if crate::net::read(text).is_ok() {
            found.push("ip");
        }
        if (text.starts_with("0x") || text.starts_with("0b") || text.starts_with("0o"))
            && crate::numbers::read(text).is_ok()
        {
            found.push("numbers");
        }
        if text.contains('%')
            && crate::encode::url_decode(text).is_ok_and(|decoded| decoded != text)
        {
            found.push("url");
        }
        if looks_like_base64(text) {
            found.push("base64");
        }
    }
    if !found.contains(&"json") && looks_like_yaml(text) {
        found.push("yaml");
    }
    if !found.contains(&"json") && looks_like_toml(text) {
        found.push("toml");
    }
    found.dedup();
    found
}

/// Base64 that decodes to text: long enough not to be a word, only the
/// alphabet's characters, and holding something readable.
fn looks_like_base64(text: &str) -> bool {
    if text.len() < 12 || text.starts_with("data:") && !text.contains(";base64,") {
        return false;
    }
    let body = text.split_once(";base64,").map_or(text, |(_, body)| body);
    let alphabet = body
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'));
    alphabet
        && body
            .chars()
            .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && crate::encode::base64_decode(text).is_ok_and(|decoded| {
            text.starts_with("data:") || crate::encode::as_text(&decoded.bytes).is_some()
        })
}

/// Several `key: value` lines that read as a YAML map.
fn looks_like_yaml(text: &str) -> bool {
    let keyed = text
        .lines()
        .filter(|line| {
            let line = line.trim_start_matches(['-', ' ']);
            line.split_once(": ")
                .is_some_and(|(key, _)| !key.is_empty() && !key.contains(' '))
                || line.ends_with(':')
        })
        .count();
    keyed >= 2
        && yaml_rust2::YamlLoader::load_from_str(text)
            .is_ok_and(|docs| docs.first().is_some_and(|doc| doc.as_hash().is_some()))
}

/// `key = value` lines or `[sections]` that read as TOML.
fn looks_like_toml(text: &str) -> bool {
    let shaped = text
        .lines()
        .filter(|line| line.contains(" = ") || line.trim_start().starts_with('['))
        .count();
    shaped >= 2 && text.parse::<toml::Table>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_what_a_text_is() {
        let first = |text: &str| guess(text).first().copied();
        assert_eq!(
            first("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln"),
            Some("jwt")
        );
        assert_eq!(first(r#"{"a": 1}"#), Some("json"));
        assert_eq!(first("1700000000"), Some("time"));
        assert_eq!(first("2026-10-24T09:00:00Z"), Some("time"));
        assert_eq!(first("*/5 * * * *"), Some("cron"));
        assert_eq!(first("#3b82f6"), Some("color"));
        assert_eq!(first("10.0.0.0/8"), Some("ip"));
        assert_eq!(first("0xff"), Some("numbers"));
        assert_eq!(first("https://x.test/?a=b"), Some("url"));
        assert_eq!(first("a%20b%26c"), Some("url"));
        assert_eq!(first("aGVsbG8gd29ybGQh"), Some("base64"));
        assert_eq!(first("0190b4e2-7c1d-7d3e-9f00-0a1b2c3d4e5f"), Some("ids"));
        assert_eq!(first("name: app\nports:\n  - 80\n"), Some("yaml"));
        assert_eq!(first("[package]\nname = \"x\"\n"), Some("toml"));
        assert_eq!(first("<a><b/></a>"), Some("xml"));
        assert!(guess("just some words").is_empty());
    }
}
