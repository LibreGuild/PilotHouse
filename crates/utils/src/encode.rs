//! Encodings: Base64, URL, hex, HTML entities, and JSON Web Tokens.
//!
//! Each encoding has its two directions as functions of their own, which
//! the dialog runs as the user types on either side; the text functions at
//! the end are what agents run.

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use serde_json::Value;

use crate::Input;

/// `bytes` as Base64, standard or URL-safe (`-` and `_`).
pub fn base64_encode(bytes: &[u8], url_safe: bool) -> String {
    if url_safe {
        URL_SAFE.encode(bytes)
    } else {
        STANDARD.encode(bytes)
    }
}

/// What Base64 held, and its type when it came as a data URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub bytes: Vec<u8>,
    pub mime: Option<String>,
}

/// Base64, standard or URL-safe, with or without padding or line breaks,
/// or a `data:` URI.
pub fn base64_decode(text: &str) -> Result<Decoded> {
    let text = text.trim();
    let (mime, body) = match text.strip_prefix("data:") {
        Some(rest) => {
            let (head, body) = rest
                .split_once(',')
                .context("A data URI has a comma before its data")?;
            if !head.ends_with(";base64") {
                bail!("This data URI is not Base64 (it has no ;base64)");
            }
            let mime = head.trim_end_matches(";base64");
            ((!mime.is_empty()).then(|| mime.to_string()), body)
        }
        None => (None, text),
    };
    let clean: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let unpadded = clean.trim_end_matches('=');
    let bytes = if clean.contains(['-', '_']) {
        URL_SAFE_NO_PAD.decode(unpadded)
    } else {
        STANDARD_NO_PAD.decode(unpadded)
    }
    .map_err(|err| anyhow::anyhow!("Not valid Base64: {err}"))?;
    Ok(Decoded { bytes, mime })
}

/// `bytes` as a `data:` URI of type `mime`.
pub fn data_uri(bytes: &[u8], mime: &str) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

/// The type of a file from its first bytes, for those worth showing or
/// naming: pictures, PDF, archives.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    const SIGNATURES: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
        (b"PK\x03\x04", "application/zip"),
        (b"\x1f\x8b", "application/gzip"),
        (b"BM", "image/bmp"),
    ];
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    SIGNATURES
        .iter()
        .find(|(signature, _)| bytes.starts_with(signature))
        .map(|(_, mime)| *mime)
}

/// A picture's width and height from its header, for the kinds that are
/// shown (PNG, JPEG, GIF, WebP), without decoding it.
pub fn picture_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as u32);
    let le16 = |at: usize| Some(u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as u32);
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    match sniff(bytes)? {
        "image/png" => Some((be32(16)?, be32(20)?)),
        "image/gif" => Some((le16(6)?, le16(8)?)),
        "image/jpeg" => {
            // The first start-of-frame marker holds the size.
            let mut at = 2;
            while at + 9 < bytes.len() {
                if bytes[at] != 0xff {
                    return None;
                }
                let marker = bytes[at + 1];
                let length = be16(at + 2)? as usize;
                if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                    return Some((be16(at + 7)?, be16(at + 5)?));
                }
                at += 2 + length;
            }
            None
        }
        "image/webp" => match bytes.get(12..16)? {
            b"VP8X" => {
                let three = |at: usize| -> Option<u32> {
                    Some(u32::from_le_bytes([
                        *bytes.get(at)?,
                        *bytes.get(at + 1)?,
                        *bytes.get(at + 2)?,
                        0,
                    ]))
                };
                Some((three(24)? + 1, three(27)? + 1))
            }
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Text that is UTF-8 and has no control characters other than line
/// breaks and tabs, so it reads as text rather than as bytes.
pub fn as_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .then_some(text)
}

/// The first `limit` bytes as `xxd` shows them: offset, sixteen bytes in
/// hex, and those that are printable as characters.
pub fn hex_dump(bytes: &[u8], limit: usize) -> String {
    let mut lines: Vec<String> = bytes
        .iter()
        .take(limit)
        .collect::<Vec<_>>()
        .chunks(16)
        .enumerate()
        .map(|(row, chunk)| {
            let hex: Vec<String> = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
            let text: String = chunk
                .iter()
                .map(|byte| {
                    if byte.is_ascii_graphic() || **byte == b' ' {
                        **byte as char
                    } else {
                        '.'
                    }
                })
                .collect();
            format!("{:08x}  {:<47}  {text}", row * 16, hex.join(" "))
        })
        .collect();
    if bytes.len() > limit {
        lines.push(format!("… {} more bytes", bytes.len() - limit));
    }
    lines.join("\n")
}

/// Percent-encodes everything but letters, digits and `-._~` (a space is
/// `%20`, never `+`), so the result fits anywhere in a URL.
pub fn url_encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
        .replace("%7E", "~")
}

/// Percent-decodes; a `+` stays a `+` (it means a space only in a form).
pub fn url_decode(text: &str) -> Result<String> {
    let bytes: Vec<u8> = url::form_urlencoded::parse(
        format!("x={}", text.trim().replace('+', "%2B").replace('&', "%26")).as_bytes(),
    )
    .next()
    .map(|(_, value)| value.into_owned().into_bytes())
    .unwrap_or_default();
    String::from_utf8(bytes).context("The decoded bytes are not text")
}

/// A URL taken apart; the query's values decoded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlParts {
    pub scheme: String,
    pub user: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub fragment: Option<String>,
}

/// The parts of a full URL (one with its scheme).
pub fn url_parts(text: &str) -> Result<UrlParts> {
    let parsed = url::Url::parse(text.trim()).context("Not a full URL (with https://…)")?;
    Ok(UrlParts {
        scheme: parsed.scheme().to_string(),
        user: (!parsed.username().is_empty()).then(|| parsed.username().to_string()),
        host: parsed.host_str().map(str::to_string),
        port: parsed.port(),
        path: parsed.path().to_string(),
        query: parsed.query_pairs().into_owned().collect(),
        fragment: parsed.fragment().map(str::to_string),
    })
}

/// `bytes` in hex, with `separator` between bytes.
pub fn hex_encode(bytes: &[u8], separator: &str, upper: bool) -> String {
    bytes
        .iter()
        .map(|byte| {
            if upper {
                format!("{byte:02X}")
            } else {
                format!("{byte:02x}")
            }
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// Hex, with or without `0x`, spaces, colons or dashes between bytes.
pub fn hex_decode(text: &str) -> Result<Vec<u8>> {
    let clean: String = text
        .trim()
        .trim_start_matches("0x")
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, ':' | '-' | ','))
        .collect();
    ::hex::decode(clean).map_err(|err| anyhow::anyhow!("Not valid hex: {err}"))
}

/// Named entities read back, beyond the five every escape makes.
const NAMED: &[(&str, &str)] = &[
    ("&nbsp;", "\u{a0}"),
    ("&copy;", "©"),
    ("&reg;", "®"),
    ("&trade;", "™"),
    ("&hellip;", "…"),
    ("&mdash;", "—"),
    ("&ndash;", "–"),
    ("&lsquo;", "‘"),
    ("&rsquo;", "’"),
    ("&ldquo;", "“"),
    ("&rdquo;", "”"),
    ("&laquo;", "«"),
    ("&raquo;", "»"),
    ("&bull;", "•"),
    ("&middot;", "·"),
    ("&euro;", "€"),
    ("&pound;", "£"),
    ("&yen;", "¥"),
    ("&cent;", "¢"),
    ("&sect;", "§"),
    ("&deg;", "°"),
    ("&times;", "×"),
    ("&divide;", "÷"),
    ("&plusmn;", "±"),
    ("&larr;", "←"),
    ("&rarr;", "→"),
    ("&uarr;", "↑"),
    ("&darr;", "↓"),
    ("&apos;", "'"),
];

/// `&`, `<`, `>`, `"` and `'` as entities; with `non_ascii`, every other
/// character beyond ASCII too (`&#xE9;`), for a page in another encoding.
pub fn html_escape(text: &str, non_ascii: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if non_ascii && !c.is_ascii() => out.push_str(&format!("&#x{:X};", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Entities back to characters: named, `&#65;` and `&#x41;`.
pub fn html_unescape(text: &str) -> String {
    // Made once: this runs again at every key typed into the utility.
    static ENTITY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").expect("valid pattern")
    });
    ENTITY
        .replace_all(text, |caps: &regex::Captures| {
            let whole = &caps[0];
            let code = &caps[1];
            let numeric = code
                .strip_prefix("#x")
                .or_else(|| code.strip_prefix("#X"))
                .map(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| code.strip_prefix('#').map(|digits| digits.parse().ok()));
            match numeric {
                Some(value) => value
                    .and_then(char::from_u32)
                    .map_or_else(|| whole.to_string(), String::from),
                None => match whole {
                    "&amp;" => "&".into(),
                    "&lt;" => "<".into(),
                    "&gt;" => ">".into(),
                    "&quot;" => "\"".into(),
                    other => NAMED
                        .iter()
                        .find(|(name, _)| *name == other)
                        .map_or_else(|| other.to_string(), |(_, raw)| raw.to_string()),
                },
            }
        })
        .into_owned()
}

/// A JSON Web Token read (not checked).
#[derive(Debug, Clone, PartialEq)]
pub struct Jwt {
    pub header: Value,
    pub claims: Value,
    /// The three parts as given: header, claims, signature.
    pub parts: [String; 3],
}

impl Jwt {
    pub fn algorithm(&self) -> &str {
        self.header["alg"].as_str().unwrap_or("?")
    }
}

/// The token in `text` (a `Bearer ` in front is left out).
pub fn jwt_read(text: &str) -> Result<Jwt> {
    let token = text.trim();
    let token = token
        .strip_prefix("Bearer ")
        .or_else(|| token.strip_prefix("bearer "))
        .unwrap_or(token)
        .trim();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() == 5 {
        bail!("This is an encrypted token (JWE, five parts): only the key can read it.");
    }
    if parts.len() != 3 {
        bail!("A JWT has three parts separated by dots.");
    }
    let part = |ix: usize| -> Result<Value> {
        let bytes = URL_SAFE_NO_PAD
            .decode(parts[ix].trim_end_matches('='))
            .with_context(|| format!("Part {} is not Base64", ix + 1))?;
        serde_json::from_slice(&bytes).with_context(|| format!("Part {} is not JSON", ix + 1))
    };
    Ok(Jwt {
        header: part(0)?,
        claims: part(1)?,
        parts: [
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
        ],
    })
}

/// Whether a token's signature holds for a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signature {
    Valid,
    Invalid,
    /// Not checked here, and why.
    Unchecked(String),
}

/// Checks an HMAC signature (HS256, HS384, HS512) with `secret`, given as
/// text or, with `base64_secret`, as Base64 (as some servers keep it).
/// Other algorithms need a public key, which this does not take.
pub fn jwt_verify(jwt: &Jwt, secret: &str, base64_secret: bool) -> Signature {
    let key = if base64_secret {
        match base64_decode(secret) {
            Ok(decoded) => decoded.bytes,
            Err(_) => return Signature::Unchecked("The secret is not Base64.".into()),
        }
    } else {
        secret.as_bytes().to_vec()
    };
    let signed = format!("{}.{}", jwt.parts[0], jwt.parts[1]);
    let Ok(signature) = URL_SAFE_NO_PAD.decode(jwt.parts[2].trim_end_matches('=')) else {
        return Signature::Invalid;
    };
    fn verify<M: hmac::Mac + hmac::KeyInit>(key: &[u8], signed: &str, signature: &[u8]) -> bool {
        let Ok(mut mac) = <M as hmac::KeyInit>::new_from_slice(key) else {
            return false;
        };
        mac.update(signed.as_bytes());
        mac.verify_slice(signature).is_ok()
    }
    let valid = match jwt.algorithm() {
        "HS256" => verify::<hmac::Hmac<sha2::Sha256>>(&key, &signed, &signature),
        "HS384" => verify::<hmac::Hmac<sha2::Sha384>>(&key, &signed, &signature),
        "HS512" => verify::<hmac::Hmac<sha2::Sha512>>(&key, &signed, &signature),
        "none" => return Signature::Unchecked("This token is not signed (alg none).".into()),
        other => {
            return Signature::Unchecked(format!(
                "{other} is checked with the issuer's public key, not a secret."
            ));
        }
    };
    if valid {
        Signature::Valid
    } else {
        Signature::Invalid
    }
}

/// What the registered claims are, for those who do not know them by name.
pub fn claim_meaning(key: &str) -> Option<&'static str> {
    Some(match key {
        "iss" => "Issuer",
        "sub" => "Subject",
        "aud" => "Audience",
        "exp" => "Expires",
        "nbf" => "Not before",
        "iat" => "Issued",
        "jti" => "Token id",
        "azp" => "Authorized party",
        "scope" | "scp" => "Scopes",
        "sid" => "Session",
        "nonce" => "Nonce",
        "email" => "Email",
        "name" => "Name",
        "roles" => "Roles",
        "auth_time" => "Signed in",
        _ => return None,
    })
}

/// Encode, decode (standard or URL-safe, with or without padding), or
/// encode URL-safe.
pub fn base64(input: &Input) -> Result<String> {
    match input.mode {
        0 => Ok(base64_encode(input.text.as_bytes(), false)),
        2 => Ok(base64_encode(input.text.as_bytes(), true)),
        _ => {
            let decoded = base64_decode(&input.text)?;
            Ok(match as_text(&decoded.bytes) {
                Some(text) => text.to_string(),
                None => format!(
                    "(not text; {} bytes{}) {}",
                    decoded.bytes.len(),
                    sniff(&decoded.bytes)
                        .map(|mime| format!(", {mime}"))
                        .unwrap_or_default(),
                    ::hex::encode(&decoded.bytes)
                ),
            })
        }
    }
}

/// Percent-encode, decode, or split a URL into its parts.
pub fn url(input: &Input) -> Result<String> {
    match input.mode {
        0 => Ok(url_encode(&input.text)),
        1 => url_decode(&input.text),
        _ => {
            let parts = url_parts(&input.text)?;
            let mut out = vec![
                format!("scheme: {}", parts.scheme),
                format!("host: {}", parts.host.unwrap_or_default()),
            ];
            if let Some(user) = parts.user {
                out.push(format!("user: {user}"));
            }
            if let Some(port) = parts.port {
                out.push(format!("port: {port}"));
            }
            out.push(format!("path: {}", parts.path));
            for (key, value) in parts.query {
                out.push(format!("query {key}: {value}"));
            }
            if let Some(fragment) = parts.fragment {
                out.push(format!("fragment: {fragment}"));
            }
            Ok(out.join("\n"))
        }
    }
}

pub fn hex(input: &Input) -> Result<String> {
    if input.mode == 0 {
        return Ok(hex_encode(input.text.as_bytes(), "", false));
    }
    let bytes = hex_decode(&input.text)?;
    Ok(match as_text(&bytes) {
        Some(text) => text.to_string(),
        None => format!("(not text; {} bytes)", bytes.len()),
    })
}

pub fn html(input: &Input) -> Result<String> {
    Ok(if input.mode == 0 {
        html_escape(&input.text, false)
    } else {
        html_unescape(&input.text)
    })
}

/// The header and claims, readable times, and (with a secret) whether an
/// HMAC signature holds. Nothing is sent anywhere.
pub fn jwt(input: &Input) -> Result<String> {
    let token = jwt_read(&input.text)?;
    let mut out = vec![
        "Header:".to_string(),
        serde_json::to_string_pretty(&token.header)?,
        "Claims:".to_string(),
        serde_json::to_string_pretty(&token.claims)?,
    ];
    for key in ["iat", "nbf", "exp"] {
        if let Some(seconds) = token.claims[key].as_i64()
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
        out.push(match jwt_verify(&token, secret, false) {
            Signature::Valid => "Signature: valid".to_string(),
            Signature::Invalid => "Signature: does NOT match this secret".to_string(),
            Signature::Unchecked(why) => format!("Signature: not checked. {why}"),
        });
    }
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{KeyInit as _, Mac as _};

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
        assert_eq!(url(&with("a b&c~", 0)).unwrap(), "a%20b%26c~");
        assert_eq!(url(&with("a%20b%26c+d", 1)).unwrap(), "a b&c+d");
        assert_eq!(url_decode("x&y=1").unwrap(), "x&y=1");
        assert!(
            url(&with("https://x.dev:8080/p?q=1#f", 2))
                .unwrap()
                .contains("query q: 1")
        );
        assert_eq!(hex(&with("hi", 0)).unwrap(), "6869");
        assert_eq!(hex(&with("0x68 69", 1)).unwrap(), "hi");
        assert_eq!(hex_decode("68:69").unwrap(), b"hi");
        assert_eq!(hex_encode(b"hi", " ", true), "68 69");
        assert_eq!(
            html(&with("<a href=\"x\">&</a>", 0)).unwrap(),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
        assert_eq!(
            html(&with("&lt;&#65;&#x42;&amp;lt;&copy;&bogus;", 1)).unwrap(),
            "<AB&lt;©&bogus;"
        );
        assert_eq!(html_escape("é<", true), "&#xE9;&lt;");
    }

    #[test]
    fn reads_data_uris_and_binary() {
        let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 1];
        let uri = data_uri(&png, "image/png");
        let decoded = base64_decode(&uri).unwrap();
        assert_eq!(decoded.bytes, png);
        assert_eq!(decoded.mime.as_deref(), Some("image/png"));
        assert_eq!(sniff(&decoded.bytes), Some("image/png"));
        assert!(as_text(&decoded.bytes).is_none());
        assert!(base64(&with(&uri, 1)).unwrap().contains("image/png"));
        let dump = hex_dump(b"hello, world and more bytes", 20);
        assert!(dump.starts_with("00000000  68 65 6c 6c 6f"), "{dump}");
        assert!(dump.ends_with("… 7 more bytes"), "{dump}");
        assert!(base64_decode("data:text/plain,hi").is_err());
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend_from_slice(&640u32.to_be_bytes());
        header.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(picture_size(&header), Some((640, 480)));
        assert_eq!(picture_size(b"GIF89a\x10\x00\x20\x00"), Some((16, 32)));
        assert_eq!(picture_size(b"plain"), None);
    }

    #[test]
    fn decodes_and_checks_a_jwt() {
        // {"alg":"HS256","typ":"JWT"}.{"sub":"1","exp":1}, signed with a
        // secret made for the run (as the utility makes keys): any secret
        // will do, so none is written here.
        let secret = crate::hash::random_key(16, false).unwrap();
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwiZXhwIjoxfQ.";
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(token.trim_end_matches('.').as_bytes());
        let signed = format!(
            "{token}{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        );
        let out = jwt(&Input {
            second: secret.clone(),
            ..Input::new(format!("Bearer {signed}"))
        })
        .unwrap();
        assert!(
            out.contains(r#""sub": "1""#)
                && out.contains("(expired)")
                && out.contains("Signature: valid")
        );
        let wrong = jwt(&Input {
            second: "nope".into(),
            ..Input::new(signed.clone())
        })
        .unwrap();
        assert!(wrong.contains("does NOT match"));
        // The same secret given as Base64.
        let read = jwt_read(&signed).unwrap();
        assert_eq!(read.algorithm(), "HS256");
        assert_eq!(
            jwt_verify(&read, &STANDARD.encode(secret.as_bytes()), true),
            Signature::Valid
        );
        assert!(jwt(&with("a.b", 0)).is_err());
        assert!(
            jwt_read("a.b.c.d.e")
                .unwrap_err()
                .to_string()
                .contains("encrypted")
        );
    }
}
