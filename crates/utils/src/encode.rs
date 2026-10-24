//! Encodings: Base64, URL, hex, HTML entities, and JSON Web Tokens.

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use hmac::{KeyInit as _, Mac as _};

use crate::Input;

/// Encode, decode (standard or URL-safe, with or without padding), or
/// encode URL-safe.
pub fn base64(input: &Input) -> Result<String> {
    match input.mode {
        0 => Ok(STANDARD.encode(input.text.as_bytes())),
        2 => Ok(URL_SAFE.encode(input.text.as_bytes())),
        _ => {
            let bytes = decode_base64(input.text.trim())?;
            Ok(String::from_utf8(bytes.clone()).unwrap_or_else(|_| {
                format!(
                    "(not text; {} bytes) {}",
                    bytes.len(),
                    ::hex::encode(&bytes)
                )
            }))
        }
    }
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let unpadded = text.trim_end_matches('=');
    if text.contains(['-', '_']) {
        return URL_SAFE_NO_PAD.decode(unpadded).context("Not valid Base64");
    }
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(unpadded)
        .context("Not valid Base64")
}

/// Percent-encode, decode, or split a URL into its parts.
pub fn url(input: &Input) -> Result<String> {
    match input.mode {
        0 => Ok(url::form_urlencoded::byte_serialize(input.text.as_bytes())
            .collect::<String>()
            .replace('+', "%20")),
        1 => Ok(url::form_urlencoded::parse(
            format!("x={}", input.text.trim().replace('+', "%2B")).as_bytes(),
        )
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()),
        _ => {
            let parsed =
                url::Url::parse(input.text.trim()).context("Not a full URL (with https://…)")?;
            let mut out = vec![
                format!("scheme: {}", parsed.scheme()),
                format!("host: {}", parsed.host_str().unwrap_or("")),
            ];
            if let Some(port) = parsed.port() {
                out.push(format!("port: {port}"));
            }
            out.push(format!("path: {}", parsed.path()));
            for (key, value) in parsed.query_pairs() {
                out.push(format!("query {key}: {value}"));
            }
            if let Some(fragment) = parsed.fragment() {
                out.push(format!("fragment: {fragment}"));
            }
            Ok(out.join("\n"))
        }
    }
}

pub fn hex(input: &Input) -> Result<String> {
    if input.mode == 0 {
        return Ok(::hex::encode(input.text.as_bytes()));
    }
    let clean: String = input
        .text
        .trim()
        .trim_start_matches("0x")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect();
    let bytes = ::hex::decode(clean).context("Not valid hex")?;
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|err| format!("(not text; {} bytes)", err.as_bytes().len())))
}

pub fn html(input: &Input) -> Result<String> {
    const ENTITIES: [(&str, &str); 6] = [
        ("&", "&amp;"),
        ("<", "&lt;"),
        (">", "&gt;"),
        ("\"", "&quot;"),
        ("'", "&#39;"),
        ("\u{a0}", "&nbsp;"),
    ];
    if input.mode == 0 {
        return Ok(ENTITIES
            .iter()
            .take(5)
            .fold(input.text.clone(), |text, (raw, entity)| {
                text.replace(raw, entity)
            }));
    }
    let mut text = ENTITIES
        .iter()
        .skip(1)
        .fold(input.text.clone(), |text, (raw, entity)| {
            text.replace(entity, raw)
        });
    // Numeric entities: &#65; and &#x41;.
    let numeric = regex::Regex::new(r"&#(x[0-9a-fA-F]+|[0-9]+);").expect("valid pattern");
    text = numeric
        .replace_all(&text, |caps: &regex::Captures| {
            let code = &caps[1];
            let value = match code.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => code.parse().ok(),
            };
            value
                .and_then(char::from_u32)
                .map_or_else(|| caps[0].to_string(), String::from)
        })
        .into_owned();
    Ok(text.replace("&amp;", "&"))
}

/// The header and claims, readable times, and (with a secret) whether an
/// HS256 signature holds. Nothing is sent anywhere.
pub fn jwt(input: &Input) -> Result<String> {
    let token = input.text.trim().trim_start_matches("Bearer ").trim();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        bail!("A JWT has three parts separated by dots.");
    }
    let part = |ix: usize| -> Result<serde_json::Value> {
        let bytes = URL_SAFE_NO_PAD
            .decode(parts[ix].trim_end_matches('='))
            .with_context(|| format!("part {} is not Base64", ix + 1))?;
        serde_json::from_slice(&bytes).with_context(|| format!("part {} is not JSON", ix + 1))
    };
    let (header, claims) = (part(0)?, part(1)?);
    let mut out = vec![
        "Header:".to_string(),
        serde_json::to_string_pretty(&header)?,
        "Claims:".to_string(),
        serde_json::to_string_pretty(&claims)?,
    ];
    for key in ["iat", "nbf", "exp"] {
        if let Some(seconds) = claims[key].as_i64()
            && let Ok(at) = jiff::Timestamp::from_second(seconds)
        {
            let state = match key {
                "exp" if at < jiff::Timestamp::now() => " (expired)",
                "exp" => " (still valid)",
                _ => "",
            };
            out.push(format!("{key}: {at}{state}"));
        }
    }
    let secret = input.second.trim();
    if !secret.is_empty() {
        if header["alg"] != "HS256" {
            out.push(format!(
                "Signature: only HS256 can be checked here (this is {}).",
                header["alg"]
            ));
        } else {
            let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())?;
            mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
            let signature = URL_SAFE_NO_PAD
                .decode(parts[2].trim_end_matches('='))
                .unwrap_or_default();
            let valid = mac.verify_slice(&signature).is_ok();
            out.push(format!(
                "Signature: {}",
                if valid {
                    "valid"
                } else {
                    "does NOT match this secret"
                }
            ));
        }
    }
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(text: &str, mode: usize) -> Input {
        Input {
            mode,
            ..Input::new(text)
        }
    }

    #[test]
    fn round_trips() {
        assert_eq!(base64(&with("hi there", 0)).unwrap(), "aGkgdGhlcmU=");
        assert_eq!(
            base64(&with("aGkgdGhlcmU", 1)).unwrap(),
            "hi there",
            "padding optional"
        );
        assert_eq!(url(&with("a b&c", 0)).unwrap(), "a%20b%26c");
        assert_eq!(url(&with("a%20b%26c+d", 1)).unwrap(), "a b&c+d");
        assert!(
            url(&with("https://x.dev:8080/p?q=1#f", 2))
                .unwrap()
                .contains("query q: 1")
        );
        assert_eq!(hex(&with("hi", 0)).unwrap(), "6869");
        assert_eq!(hex(&with("0x68 69", 1)).unwrap(), "hi");
        assert_eq!(
            html(&with("<a href=\"x\">&</a>", 0)).unwrap(),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
        assert_eq!(
            html(&with("&lt;&#65;&#x42;&amp;lt;", 1)).unwrap(),
            "<AB&lt;"
        );
    }

    #[test]
    fn decodes_and_checks_a_jwt() {
        // {"alg":"HS256","typ":"JWT"}.{"sub":"1","exp":1}, signed with "secret".
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwiZXhwIjoxfQ.";
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(token.trim_end_matches('.').as_bytes());
        let signed = format!(
            "{token}{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        );
        let out = jwt(&Input {
            second: "secret".into(),
            ..Input::new(signed.clone())
        })
        .unwrap();
        assert!(
            out.contains(r#""sub": "1""#)
                && out.contains("(expired)")
                && out.contains("Signature: valid")
        );
        let wrong = jwt(&Input {
            second: "nope".into(),
            ..Input::new(signed)
        })
        .unwrap();
        assert!(wrong.contains("does NOT match"));
        assert!(jwt(&with("a.b", 0)).is_err());
    }
}
