//! Offline developer utilities, and the Utilities dialog that shows them.
//! Nothing here touches the network.
//!
//! Each utility is a function from text to text ([`UTILITIES`]), which
//! agents run (`util_list`, `util_run`); the modules also give the dialog
//! what it draws (a token's claims, a certificate's dates, a regex's
//! matches) rather than the text.
//!
//! - [`data`]: JSON, TOML, YAML, XML, CSV.
//! - [`sqlite`]: read-only queries on a SQLite file.
//! - [`encode`]: Base64, URL, hex, HTML, JWT.
//! - [`cert`]: X.509 certificates, QR codes.
//! - [`hash`]: hashes, HMAC, UUID, ULID, passwords and keys.
//! - [`time`]: Unix times and dates, cron.
//! - [`text`]: regex, diff, Markdown. [`words`]: cases, counts, lines.
//! - [`color`]: colors. [`numbers`]: number bases. [`net`]: IP networks.
//! - [`detect`]: which utility a text is for.
//! - [`dialog`]: the Utilities dialog.

pub mod cert;
pub mod color;
pub mod data;
pub mod detect;
pub mod dialog;
pub mod encode;
pub mod hash;
pub mod net;
pub mod numbers;
pub mod sqlite;
pub mod text;
pub mod time;
pub mod words;

use anyhow::Result;

/// What a utility is given: its main text, a second one for those that
/// need two (diff, HMAC key, regex text), and the chosen mode.
#[derive(Debug, Clone, Default)]
pub struct Input {
    pub text: String,
    pub second: String,
    pub mode: usize,
}

impl Input {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

/// One utility.
pub struct Utility {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    /// Extra words the palette matches.
    pub keywords: &'static [&'static str],
    /// What the main input is, as a placeholder.
    pub input: &'static str,
    /// The second input's placeholder, for utilities that take one.
    pub second: Option<&'static str>,
    /// Ways to run it (the first is the default), e.g. Encode / Decode.
    pub modes: &'static [&'static str],
    pub run: fn(&Input) -> Result<String>,
}

pub const UTILITIES: &[Utility] = &[
    Utility {
        id: "json",
        name: "JSON",
        group: "Data",
        keywords: &["format", "pretty", "minify", "validate"],
        input: "JSON",
        second: None,
        modes: &["Format", "Minify", "To TOML", "To YAML", "To CSV"],
        run: data::json,
    },
    Utility {
        id: "toml",
        name: "TOML",
        group: "Data",
        keywords: &["format", "validate", "cargo"],
        input: "TOML",
        second: None,
        modes: &["To JSON", "Format", "To YAML"],
        run: data::toml,
    },
    Utility {
        id: "yaml",
        name: "YAML",
        group: "Data",
        keywords: &["format", "validate", "kubernetes", "compose", "workflow"],
        input: "YAML (JSON for From JSON)",
        second: None,
        modes: &["To JSON", "Format", "From JSON", "To TOML"],
        run: data::yaml,
    },
    Utility {
        id: "xml",
        name: "XML",
        group: "Data",
        keywords: &["format", "pretty", "validate", "html", "svg"],
        input: "XML",
        second: None,
        modes: &["Format", "Minify"],
        run: data::xml,
    },
    Utility {
        id: "csv",
        name: "CSV",
        group: "Data",
        keywords: &["table", "spreadsheet", "tsv"],
        input: "CSV, first row the header (JSON for From JSON)",
        second: None,
        modes: &["To JSON", "To table", "From JSON"],
        run: data::csv,
    },
    Utility {
        id: "sqlite",
        name: "SQLite",
        group: "Data",
        keywords: &["database", "db", "query", "sql", "browser"],
        input: "Path to a SQLite file",
        second: Some("A query (read only); empty lists the tables"),
        modes: &["Query"],
        run: sqlite::query,
    },
    Utility {
        id: "base64",
        name: "Base64",
        group: "Encoding",
        keywords: &["encode", "decode", "b64"],
        input: "Text, or Base64 to decode",
        second: None,
        modes: &["Encode", "Decode", "Encode (URL-safe)"],
        run: encode::base64,
    },
    Utility {
        id: "url",
        name: "URL encoding",
        group: "Encoding",
        keywords: &["percent", "escape", "query", "encode", "decode"],
        input: "Text, or a %-encoded string",
        second: None,
        modes: &["Encode", "Decode", "Parse URL"],
        run: encode::url,
    },
    Utility {
        id: "hex",
        name: "Hex",
        group: "Encoding",
        keywords: &["bytes", "encode", "decode"],
        input: "Text, or hex to decode",
        second: None,
        modes: &["Encode", "Decode"],
        run: encode::hex,
    },
    Utility {
        id: "html",
        name: "HTML entities",
        group: "Encoding",
        keywords: &["escape", "unescape", "entities"],
        input: "Text, or HTML with entities",
        second: None,
        modes: &["Escape", "Unescape"],
        run: encode::html,
    },
    Utility {
        id: "jwt",
        name: "JWT",
        group: "Encoding",
        keywords: &["token", "decode", "verify", "jose"],
        input: "A JSON Web Token",
        second: Some("Secret to check an HS256 signature (optional)"),
        modes: &["Decode"],
        run: encode::jwt,
    },
    Utility {
        id: "x509",
        name: "X.509 certificate",
        group: "Encoding",
        keywords: &[
            "certificate",
            "pem",
            "tls",
            "ssl",
            "cert",
            "expiry",
            "chain",
        ],
        input: "PEM certificate or chain",
        second: None,
        modes: &["Describe"],
        run: cert::x509,
    },
    Utility {
        id: "qr",
        name: "QR code",
        group: "Encoding",
        keywords: &["qrcode", "barcode", "phone", "link", "scan"],
        input: "Text or link",
        second: None,
        modes: &["For a dark screen", "For a light screen"],
        run: cert::qr,
    },
    Utility {
        id: "hash",
        name: "Hash",
        group: "Crypto and ids",
        keywords: &[
            "md5", "sha1", "sha256", "sha512", "digest", "checksum", "sri",
        ],
        input: "Text to hash",
        second: None,
        modes: &["SHA-256", "SHA-512", "MD5", "SHA-1"],
        run: hash::hash,
    },
    Utility {
        id: "hmac",
        name: "HMAC",
        group: "Crypto and ids",
        keywords: &["signature", "sha256", "webhook"],
        input: "Message",
        second: Some("Key"),
        modes: &["HMAC-SHA256 (hex)", "HMAC-SHA256 (Base64)"],
        run: hash::hmac,
    },
    Utility {
        id: "ids",
        name: "UUID and ULID",
        group: "Crypto and ids",
        keywords: &["uuid", "ulid", "guid", "random", "id", "v7"],
        input: "How many (default 5)",
        second: None,
        modes: &["UUID v4", "ULID", "UUID v7"],
        run: hash::ids,
    },
    Utility {
        id: "secrets",
        name: "Passwords and keys",
        group: "Crypto and ids",
        keywords: &[
            "password", "random", "secret", "key", "token", "api", "generate",
        ],
        input: "Length: characters for a password (default 24), bytes for a key (default 32)",
        second: None,
        modes: &["Password", "Hex key", "Base64 key"],
        run: hash::secrets,
    },
    Utility {
        id: "time",
        name: "Unix time",
        group: "Time",
        keywords: &["epoch", "timestamp", "date", "timezone", "iso"],
        input: "A Unix time (seconds or milliseconds), a date (2026-10-24T09:00), or empty for now",
        second: Some("Time zone, e.g. Europe/Berlin (optional)"),
        modes: &["Convert"],
        run: time::convert,
    },
    Utility {
        id: "cron",
        name: "Cron",
        group: "Time",
        keywords: &["schedule", "crontab", "next"],
        input: "A cron line: minute hour day month weekday (e.g. */15 9-17 * * 1-5), or @daily",
        second: None,
        modes: &["Next runs"],
        run: time::cron,
    },
    Utility {
        id: "regex",
        name: "Regex",
        group: "Text",
        keywords: &["regular expression", "match", "test", "pattern", "replace"],
        input: "Pattern",
        second: Some("Text to search"),
        modes: &["Matches", "Matches as JSON"],
        run: text::regex,
    },
    Utility {
        id: "diff",
        name: "Text diff",
        group: "Text",
        keywords: &["compare", "difference", "patch"],
        input: "Before",
        second: Some("After"),
        modes: &["Unified diff"],
        run: text::diff,
    },
    Utility {
        id: "text",
        name: "Text tools",
        group: "Text",
        keywords: &[
            "case",
            "camel",
            "snake",
            "kebab",
            "count",
            "words",
            "characters",
            "length",
            "invisible",
            "zero-width",
            "sort",
            "lines",
            "unique",
            "dedupe",
        ],
        input: "Text",
        second: None,
        modes: &["Cases", "Count", "Sort lines"],
        run: words::text,
    },
    Utility {
        id: "color",
        name: "Color",
        group: "Text",
        keywords: &[
            "hex", "rgb", "hsl", "oklch", "colour", "convert", "contrast", "wcag",
        ],
        input: "A color: #3b82f6, rgb(59 130 246), hsl(217 91% 60%), a CSS name or 0xFF3B82F6",
        second: None,
        modes: &["Convert"],
        run: color::color,
    },
    Utility {
        id: "markdown",
        name: "Markdown",
        group: "Text",
        keywords: &["md", "html", "convert", "gfm"],
        input: "Markdown",
        second: None,
        modes: &["To HTML"],
        run: text::markdown,
    },
    Utility {
        id: "numbers",
        name: "Number bases",
        group: "Numbers and networks",
        keywords: &[
            "hex", "binary", "octal", "decimal", "radix", "bits", "convert", "unicode",
        ],
        input: "A number: 255, 0xff, 0o377, 0b11111111 or -12",
        second: None,
        modes: &["Convert"],
        run: numbers::numbers,
    },
    Utility {
        id: "ip",
        name: "IP network",
        group: "Numbers and networks",
        keywords: &[
            "cidr", "subnet", "mask", "netmask", "ipv4", "ipv6", "range", "address",
        ],
        input: "A network (10.0.0.0/24, fd00::/8) or an address",
        second: Some("An address, to check whether it is in the network (optional)"),
        modes: &["Describe"],
        run: net::ip,
    },
];

pub fn find(id: &str) -> Option<&'static Utility> {
    UTILITIES.iter().find(|utility| utility.id == id)
}

/// Runs utility `id`, by its id.
pub fn run(id: &str, input: &Input) -> Result<String> {
    let utility = find(id).ok_or_else(|| {
        let ids: Vec<&str> = UTILITIES.iter().map(|utility| utility.id).collect();
        anyhow::anyhow!("no utility named `{id}`; there are: {}", ids.join(", "))
    })?;
    if input.mode >= utility.modes.len() {
        anyhow::bail!("{} has {} modes", utility.name, utility.modes.len());
    }
    (utility.run)(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_every_utility_runs() {
        for (ix, utility) in UTILITIES.iter().enumerate() {
            assert!(
                UTILITIES[ix + 1..]
                    .iter()
                    .all(|other| other.id != utility.id),
                "{} twice",
                utility.id
            );
            assert!(!utility.modes.is_empty());
        }
        assert!(run("nope", &Input::default()).is_err());
        assert!(
            run(
                "json",
                &Input {
                    mode: 9,
                    ..Input::new("{}")
                }
            )
            .is_err()
        );
    }
}
