# Utilities

Crate: `ph-utils`: each utility a function from text to text with tests (`data`, `encode`, `cert`, `hash`, `time`, `text`, `words`, `color`, `numbers`, `net`, `sqlite`), and the dialog that shows them (`dialog/`). Everything runs on this computer; nothing is sent anywhere.

Open it with **Utilities** in the column of icons, just above Settings, from the command palette, or a single one by name ("JWT", "Cron"…). It is a dialog over whatever is in front, not a place of its own: a token is decoded or a blob of JSON laid out in the middle of other work, and Esc is back where one was. (It used to be reached only from the palette and the title bar's ⋯ menu, where few found it.)

## The dialog

- **It opens on the utility used last**, with the keyboard in its input. **One size** for every utility (as large as the interface size asks for, within the window); each view scrolls inside it.
- **Find a utility** by typing part of its name, its group or what it is for (*pretty* finds JSON, *subnet* the IP network); Enter takes the first. Each has an icon in the list.
- **Each utility has a view made for it** (below), not one input over one box of text. Results are values that are copied by a click (a hash, a date, a color's form: the copy mark shows on hover and turns into a tick), tables, pictures, or a coloured editor to select from.
- **A view keeps what was typed in it** while the dialog is open: going to another utility and back loses nothing.
- **When a utility cannot read what was pasted and another can**, it says what it looks like and offers to open it there (a JWT pasted into JSON: *This looks like a JSON Web Token. Open in JWT*). A decoded result that is JSON, a token, a URL… is offered the same way. This replaces *Use as input*.
- Side by side where the dialog is wide (input beside result, Before beside After), one over the other where it is narrow.

## Each utility

| Group | Utility | Its view |
|---|---|---|
| Data | JSON, YAML, TOML, XML | Input and result in coloured editors, with line numbers and folding. *Format*, *Minify* and conversions (JSON to YAML, TOML or CSV; YAML and TOML to each other and to JSON). Indent 2, 4 or a tab, *Sort keys*. Says *Valid JSON · 24 values · 3 levels deep · 1.2 KB*; an invalid text says what is wrong, at which line and column, with that line and a mark under the place. JSON kept in a string (`"{\"a\":1}"`) or escaped (`{\"a\":1}`) is read as the JSON it holds, and says so. An XML snippet that uses a prefix declared elsewhere (`android:`) is laid out all the same. Several YAML documents (`---`) are kept apart. |
| | CSV | Read as a table you can sort by a column and widen; separators found by themselves (commas, tabs from a spreadsheet, semicolons, `\|`); *First row names the columns*; *To JSON*. |
| | SQLite | Choose a file, type its path or drop it on the view. Its tables and views down the side with their row counts (a column list on hover); a click writes a query for the first 200 rows. Write any query and **Run** (⌘↵): the rows in a grid with each column's type, NULL shown as such. Opened read-only: only statements that read are run. |
| Encoding | Base64, URL encoding, hex, HTML entities | The text and its encoded form side by side, both editable: typing on either side writes the other, so there is no Encode or Decode to choose. Base64: *URL-safe*; a `data:` URI is read; bytes that are not text are shown as a hex dump, a picture as itself (PNG, JPEG, WebP), with *Save as file…*; a file can be encoded (*Open file…*) and copied as a data URI. Hex: `ab12`, `ab 12` or `ab:12`, upper case. HTML: also every character beyond ASCII. URL: a URL on the text side is taken apart (scheme, host, port, path, each query value decoded, fragment). |
| | JWT | The token coloured in place by part (header, claims, signature). Badges: the algorithm, *Valid, expires in 3 hours* or *Expired 2 days ago* or *Not valid until…*, when it was issued. Each claim by name (*Expires (exp)*), dates in local time and how far from now. The header. The secret, masked, to check an HS256, HS384 or HS512 signature (or *Secret is Base64*); other algorithms say they need a public key. An encrypted token (five parts) says only its key reads it. |
| | X.509 certificate | Paste PEM (one or a chain) or open a `.pem`, `.crt`, `.cer` or `.der` file. A card for each: its name, *Leaf*, *Intermediate*, *Root* or *Self-signed*, *Valid · 245 days left* (amber under 30 days, red once expired) and a bar of how much of its life has passed; subject, issuer, dates, names, key, signature, CA, serial, SHA-256 and SHA-1 fingerprints. A chain says whether each is signed by the next. |
| | QR code | A real picture: dark squares on white with the margin a reader needs, so it scans from the screen in either theme. How much damage it survives (7 to 30 %). *Copy picture*, *Save as PNG…*. |
| Crypto and ids | Hash | MD5, SHA-1, SHA-256, SHA-384, SHA-512 at once, of the text or of a file (*Hash a file…*, read a piece at a time), and the SRI value (`sha384-…`). Paste a checksum to compare (hex in any case, Base64, `sha256:…`, SRI): the one that matches is marked. MD5 and SHA-1 say they are not for security. |
| | HMAC | SHA-256, SHA-1 or SHA-512; the key masked. Hex, Base64, and as GitHub's header (`sha256=…`). Paste a webhook's signature (`sha256=…`, `v1,…`, hex or Base64) to check it. |
| | UUID and ULID | Made at once, 1 to 50: UUID v4, UUID v7 (starts with the time, sorts by it), ULID; upper case, no dashes; *New ones*, *Copy all*. *Read an id*: its kind and version, and when it was made (v1, v6, v7, ULID). |
| | Passwords and keys | Passwords of 12 to 64 characters with the kinds chosen (and *No look-alikes*), each kind at least once; keys of 16 to 64 random bytes in hex or Base64. How strong, in bits. From the system's secure random source; nothing is kept. |
| Time | Unix time | *Now*, ticking. A Unix time in seconds, milliseconds, microseconds or nanoseconds (told apart by size), an ISO date, an HTTP or email date, or a date in local time. Unix seconds and milliseconds, ISO, here, HTTP date, the day (weekday, ISO week, day of the year), how far from now, and every time zone added (found by typing a city; kept for next time). |
| | Cron | The line in words (*Every 15 minutes of hours 9 through 17, on Monday through Friday*), its five fields, and the next ten runs in local time or UTC (servers and GitHub Actions run cron in UTC). Six presets. Reads `@daily` and friends, month and day names, Quartz's `?`; a six-field line says its first field is probably seconds. Looks five years ahead (29 February). |
| Text | Regex | Matches marked in the text as the pattern is typed, each listed with its groups, line and place; *Copy matches*. Flags *i*, *m*, *s*, *x*. Look-around and back-references work, as in JavaScript and PCRE. *Replace* with `$1`, `${name}`, the text after it. |
| | Text diff | Before and After, compared as the Git tab compares files: side by side or in one column, `+3 −2`, *Ignore spaces*, *Swap*, *Copy as patch*. A missing last line break does not count. |
| | Text tools | *Cases*: camelCase, PascalCase, snake_case, SCREAMING_SNAKE, kebab-case, Title, Sentence, dot.case. *Count and find*: characters, words, lines, bytes, JavaScript's length, and what cannot be seen or is not what it looks like (a zero-width space, a no-break space, typographic quotes, a Cyrillic letter that looks Latin), by line, with *Copy without them*. *Lines*: sort A to Z (numbers by value) or by length, remove repeats, trim, drop blank lines, reverse. |
| | Color | A swatch and a picker. HEX, its CSS name, RGB, HSL, OKLCH, Android, Flutter and SwiftUI forms. How white text, black text and any text color read on it (WCAG ratio, AA, AAA). Nine steps from light to dark; a click takes one. |
| | Markdown | Preview as it reads, or the HTML. Pictures are not loaded (nothing leaves the computer). |
| Numbers and networks | Number bases | Decimal, hex, octal, binary (in fours), how 8 to 64 bits hold a negative number, the signed value of the narrowest width, and the character it stands for. |
| | IP network | `10.0.0.0/24`, `192.168.1.0 255.255.255.0`, `fd00::/8` or an address: network, mask, first and last, how many, wildcard, usable hosts, what kind (private, loopback, link-local…). Whether an address is in it. |

## For agents

The same utilities as text: `util_list` and `util_run` (`util.list`, `util.run` on the control socket). `util_run` takes a utility's id, its text, a second text and a mode by name. An agent's `sqlite` opens only files inside its workspace's folders; the dialog (you) can open any.

Large logs open in Files' large-file viewer ([files.md](files.md)). Not yet: image resizing, Mermaid.
