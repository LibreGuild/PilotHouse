# Utilities

Crate: `ph-utils` (each utility a function from text to text, with tests); dialog: `crates/workspace/src/utilities.rs`. Open **Utilities…** from the command palette, or a single one by name ("JWT", "Cron"…). Everything runs on this machine.

| Group | Utilities |
|---|---|
| Data | JSON (format, minify, to TOML), TOML (to JSON, format), YAML (to JSON, format, from JSON; several `---` documents become a JSON array), XML (format, minify, checks it is well-formed), CSV/TSV (to JSON, to an aligned table), SQLite (a file's tables with row counts, or a read-only query, 200 rows at most) |
| Encoding | Base64 (standard and URL-safe), URL (encode, decode, split a URL into parts), hex, HTML entities, JWT (header, claims, readable `iat`/`exp`, HS256 signature check with a secret), X.509 certificates (subject, issuer, validity and days left, names, key, CA, SHA-256 fingerprint; a whole chain at once), QR codes drawn in text, for a dark or a light screen |
| Crypto and ids | SHA-256, SHA-512, MD5; HMAC-SHA256 (hex or Base64); UUID v4 and ULID |
| Time | Unix time ↔ dates (seconds or milliseconds; UTC, local, any time zone), cron lines (the next five runs) |
| Text | Regex (matches and groups, or as JSON), text diff (unified), colors (hex ↔ RGB ↔ HSL), Markdown to HTML |

The result updates as you type; the text stays when you switch utility, so one paste can go through several. **Copy** copies the result.

Agents have the same set: `util_list` and `util_run` (`util.list`, `util.run` on the control socket).

Large logs open in Files' large-file viewer ([files.md](files.md)). Not yet: YAML, X.509 certificates, QR codes, image resizing, Mermaid.
