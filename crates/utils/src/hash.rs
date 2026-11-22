//! Hashes, HMAC, random ids (UUID v4 and v7, ULID) and random secrets.

use std::io::Read as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use sha2::Digest as _;

use crate::Input;

/// One hash of some bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hash {
    pub name: &'static str,
    pub hex: String,
    pub base64: String,
}

/// The hashes a checksum or a lock file is usually given in.
pub const ALGORITHMS: [&str; 5] = ["MD5", "SHA-1", "SHA-256", "SHA-384", "SHA-512"];

/// Hashes being worked out, fed a piece at a time (a file is not read
/// whole).
struct Hashers {
    md5: md5::Md5,
    sha1: sha1::Sha1,
    sha256: sha2::Sha256,
    sha384: sha2::Sha384,
    sha512: sha2::Sha512,
}

impl Hashers {
    fn new() -> Self {
        Self {
            md5: md5::Md5::new(),
            sha1: sha1::Sha1::new(),
            sha256: sha2::Sha256::new(),
            sha384: sha2::Sha384::new(),
            sha512: sha2::Sha512::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.md5.update(bytes);
        self.sha1.update(bytes);
        self.sha256.update(bytes);
        self.sha384.update(bytes);
        self.sha512.update(bytes);
    }

    fn finish(self) -> Vec<Hash> {
        let digests: [Vec<u8>; 5] = [
            self.md5.finalize().to_vec(),
            self.sha1.finalize().to_vec(),
            self.sha256.finalize().to_vec(),
            self.sha384.finalize().to_vec(),
            self.sha512.finalize().to_vec(),
        ];
        ALGORITHMS
            .iter()
            .zip(digests)
            .map(|(name, digest)| Hash {
                name,
                hex: ::hex::encode(&digest),
                base64: STANDARD.encode(&digest),
            })
            .collect()
    }
}

/// Every hash of `bytes`.
pub fn hashes(bytes: &[u8]) -> Vec<Hash> {
    let mut hashers = Hashers::new();
    hashers.update(bytes);
    hashers.finish()
}

/// Every hash of the file at `path`, read a piece at a time, and its size.
pub fn file_hashes(path: &Path) -> Result<(u64, Vec<Hash>)> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut hashers = Hashers::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hashers.update(&buffer[..read]);
    }
    Ok((size, hashers.finish()))
}

/// The hash `expected` is, if one of `hashes`: hex in either case, or
/// Base64, with a `sha256:`, `sha256=` or `sha256-` (SRI) in front left out.
pub fn matching<'a>(expected: &str, hashes: &'a [Hash]) -> Option<&'a Hash> {
    let expected = expected.trim();
    let expected = expected
        .split_once([':', '=', '-'])
        .filter(|(prefix, _)| {
            prefix.len() <= 7 && prefix.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map_or(expected, |(_, rest)| rest)
        .trim();
    hashes
        .iter()
        .find(|hash| hash.hex.eq_ignore_ascii_case(expected) || hash.base64 == expected)
}

pub fn hash(input: &Input) -> Result<String> {
    let all = hashes(input.text.as_bytes());
    let name = match input.mode {
        0 => "SHA-256",
        1 => "SHA-512",
        2 => "MD5",
        _ => "SHA-1",
    };
    Ok(all
        .into_iter()
        .find(|hash| hash.name == name)
        .map(|hash| hash.hex)
        .unwrap_or_default())
}

/// What an HMAC is made with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MacAlgorithm {
    Sha1,
    #[default]
    Sha256,
    Sha512,
}

/// The HMAC of `message` with `key`.
pub fn hmac_of(algorithm: MacAlgorithm, key: &[u8], message: &[u8]) -> Vec<u8> {
    fn sign<M: hmac::Mac + hmac::KeyInit>(key: &[u8], message: &[u8]) -> Vec<u8> {
        let mut mac =
            <M as hmac::KeyInit>::new_from_slice(key).expect("HMAC takes a key of any length");
        mac.update(message);
        mac.finalize().into_bytes().to_vec()
    }
    match algorithm {
        MacAlgorithm::Sha1 => sign::<hmac::Hmac<sha1::Sha1>>(key, message),
        MacAlgorithm::Sha256 => sign::<hmac::Hmac<sha2::Sha256>>(key, message),
        MacAlgorithm::Sha512 => sign::<hmac::Hmac<sha2::Sha512>>(key, message),
    }
}

/// Whether `signature` (hex or Base64, with a `sha256=` or `v1,` in front
/// as webhooks send it) is `mac`.
pub fn signature_matches(signature: &str, mac: &[u8]) -> bool {
    let signature = signature.trim();
    let signature = signature
        .split_once(['=', ','])
        .filter(|(prefix, _)| {
            prefix.len() <= 7 && prefix.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map_or(signature, |(_, rest)| rest)
        .trim();
    ::hex::decode(signature).is_ok_and(|bytes| bytes == mac)
        || STANDARD.decode(signature).is_ok_and(|bytes| bytes == mac)
        || URL_SAFE_NO_PAD
            .decode(signature.trim_end_matches('='))
            .is_ok_and(|bytes| bytes == mac)
}

pub fn hmac(input: &Input) -> Result<String> {
    let bytes = hmac_of(
        MacAlgorithm::Sha256,
        input.second.as_bytes(),
        input.text.as_bytes(),
    );
    Ok(if input.mode == 0 {
        ::hex::encode(bytes)
    } else {
        STANDARD.encode(bytes)
    })
}

/// A kind of random id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IdKind {
    #[default]
    UuidV4,
    /// Time-ordered: sorts by when it was made, which keeps database
    /// indexes compact.
    UuidV7,
    Ulid,
}

/// `count` new ids of `kind` (5 by default, 100 at most), one per line.
pub fn ids(input: &Input) -> Result<String> {
    let count = input
        .text
        .trim()
        .parse::<usize>()
        .unwrap_or(5)
        .clamp(1, 100);
    let kind = match input.mode {
        0 => IdKind::UuidV4,
        1 => IdKind::Ulid,
        _ => IdKind::UuidV7,
    };
    let ids: Result<Vec<String>> = (0..count).map(|_| new_id(kind)).collect();
    Ok(ids?.join("\n"))
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|err| anyhow::anyhow!("no randomness: {err}"))?;
    Ok(bytes)
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

/// A new id.
pub fn new_id(kind: IdKind) -> Result<String> {
    match kind {
        IdKind::UuidV4 => {
            let mut bytes = random::<16>()?;
            bytes[6] = (bytes[6] & 0x0f) | 0x40;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            Ok(uuid_text(&bytes))
        }
        IdKind::UuidV7 => {
            let mut bytes = random::<16>()?;
            bytes[..6].copy_from_slice(&(now_millis() as u64).to_be_bytes()[2..]);
            bytes[6] = (bytes[6] & 0x0f) | 0x70;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            Ok(uuid_text(&bytes))
        }
        IdKind::Ulid => {
            let random = u128::from_be_bytes({
                let mut bytes = [0u8; 16];
                bytes[6..].copy_from_slice(&random::<10>()?);
                bytes
            });
            let value = (now_millis() << 80) | random;
            Ok((0..26)
                .rev()
                .map(|ix| CROCKFORD[((value >> (ix * 5)) & 0x1f) as usize] as char)
                .collect())
        }
    }
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn uuid_text(bytes: &[u8; 16]) -> String {
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// What an id is, read from itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdFacts {
    /// "UUID v7", "ULID"…
    pub kind: String,
    /// When it was made, for the kinds that hold it.
    pub made: Option<jiff::Timestamp>,
    /// Anything else worth saying (its variant, the nil UUID).
    pub note: Option<String>,
}

/// Reads a UUID (any version) or a ULID.
pub fn read_id(text: &str) -> Result<IdFacts> {
    let text = text.trim().trim_matches(|c| c == '{' || c == '}');
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    if hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = ::hex::decode(&hex)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Ok(IdFacts {
                kind: "The nil UUID".into(),
                made: None,
                note: Some("All zeros: an id that stands for none.".into()),
            });
        }
        if bytes.iter().all(|byte| *byte == 0xff) {
            return Ok(IdFacts {
                kind: "The max UUID".into(),
                made: None,
                note: None,
            });
        }
        let version = bytes[6] >> 4;
        let variant = match bytes[8] >> 5 {
            0..=3 => "NCS (very old)",
            4 | 5 => "RFC 9562",
            6 => "Microsoft (GUID)",
            _ => "reserved",
        };
        let made = match version {
            7 => {
                let mut millis = [0u8; 8];
                millis[2..].copy_from_slice(&bytes[..6]);
                jiff::Timestamp::from_millisecond(u64::from_be_bytes(millis) as i64).ok()
            }
            1 | 6 => {
                // 100-nanosecond steps since 1582-10-15.
                let ticks: u64 = if version == 1 {
                    let low =
                        u64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
                    let mid = u64::from(u16::from_be_bytes([bytes[4], bytes[5]]));
                    let high = u64::from(u16::from_be_bytes([bytes[6], bytes[7]]) & 0x0fff);
                    (high << 48) | (mid << 32) | low
                } else {
                    let high =
                        u64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
                    let mid = u64::from(u16::from_be_bytes([bytes[4], bytes[5]]));
                    let low = u64::from(u16::from_be_bytes([bytes[6], bytes[7]]) & 0x0fff);
                    (high << 28) | (mid << 12) | low
                };
                const GREGORIAN_TO_UNIX: i64 = 122_192_928_000_000_000;
                jiff::Timestamp::from_nanosecond((ticks as i64 - GREGORIAN_TO_UNIX) as i128 * 100)
                    .ok()
            }
            _ => None,
        };
        let about = match version {
            1 => "time and the computer's address",
            3 => "an MD5 of a name",
            4 => "random",
            5 => "a SHA-1 of a name",
            6 => "time, reordered to sort",
            7 => "time, then random; sorts by when it was made",
            8 => "made by the application's own rules",
            _ => "not a known version",
        };
        return Ok(IdFacts {
            kind: format!("UUID v{version}"),
            made,
            note: Some(format!("{about}; variant {variant}")),
        });
    }
    let upper = text.to_ascii_uppercase();
    if upper.len() == 26 && upper.bytes().all(|byte| CROCKFORD.contains(&byte)) {
        let value = upper.bytes().fold(0u128, |value, byte| {
            (value << 5) | CROCKFORD.iter().position(|c| *c == byte).unwrap_or(0) as u128
        });
        let millis = (value >> 80) as i64;
        return Ok(IdFacts {
            kind: "ULID".into(),
            made: jiff::Timestamp::from_millisecond(millis).ok(),
            note: Some("time, then random; sorts by when it was made".into()),
        });
    }
    bail!("Not a UUID or a ULID.")
}

/// The characters a password may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Characters {
    pub lower: bool,
    pub upper: bool,
    pub digits: bool,
    pub symbols: bool,
    /// Without those that look alike (`l`, `1`, `I`, `O`, `0`), for one
    /// that is read out or typed from paper.
    pub unambiguous: bool,
}

impl Default for Characters {
    fn default() -> Self {
        Self {
            lower: true,
            upper: true,
            digits: true,
            symbols: true,
            unambiguous: false,
        }
    }
}

impl Characters {
    fn alphabet(self) -> Vec<char> {
        let mut all = String::new();
        if self.lower {
            all.push_str("abcdefghijklmnopqrstuvwxyz");
        }
        if self.upper {
            all.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        }
        if self.digits {
            all.push_str("0123456789");
        }
        if self.symbols {
            all.push_str("!#$%&*+-.:=?@^_~");
        }
        all.chars()
            .filter(|c| !self.unambiguous || !"lI1O0o".contains(*c))
            .collect()
    }

    /// Bits of randomness in a password of `length` of these.
    pub fn bits(self, length: usize) -> f64 {
        let size = self.alphabet().len();
        if size < 2 {
            return 0.0;
        }
        length as f64 * (size as f64).log2()
    }
}

/// A random password of `length` (4 to 256) with each chosen kind of
/// character at least once.
pub fn password(length: usize, characters: Characters) -> Result<String> {
    let length = length.clamp(4, 256);
    let alphabet = characters.alphabet();
    if alphabet.len() < 2 {
        bail!("Choose at least one kind of character.");
    }
    let sets: Vec<Vec<char>> = [
        (characters.lower, "abcdefghijklmnopqrstuvwxyz"),
        (characters.upper, "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        (characters.digits, "0123456789"),
        (characters.symbols, "!#$%&*+-.:=?@^_~"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, set)| set.chars().filter(|c| alphabet.contains(c)).collect())
    .collect();
    // Drawn again until every kind is in (rarely more than once), rather
    // than placing one of each, which would make where they are guessable.
    for _ in 0..1000 {
        let picked: Vec<char> = (0..length)
            .map(|_| Ok(alphabet[uniform(alphabet.len())?]))
            .collect::<Result<_>>()?;
        if sets
            .iter()
            .all(|set| picked.iter().any(|c| set.contains(c)))
        {
            return Ok(picked.into_iter().collect());
        }
    }
    bail!("Could not make a password with every kind of character; make it longer.")
}

/// A random number below `bound`, every one equally likely.
fn uniform(bound: usize) -> Result<usize> {
    let bound = bound as u32;
    let zone = u32::MAX - (u32::MAX % bound);
    loop {
        let value = u32::from_le_bytes(random::<4>()?);
        if value < zone {
            return Ok((value % bound) as usize);
        }
    }
}

/// `bytes` random bytes, as hex or Base64.
pub fn random_key(bytes: usize, base64: bool) -> Result<String> {
    let mut key = vec![0u8; bytes.clamp(1, 1024)];
    getrandom::fill(&mut key).map_err(|err| anyhow::anyhow!("no randomness: {err}"))?;
    Ok(if base64 {
        STANDARD.encode(key)
    } else {
        ::hex::encode(key)
    })
}

/// A password of the given length (24 by default), or a key of that many
/// bytes (32 by default) in hex or Base64.
pub fn secrets(input: &Input) -> Result<String> {
    let length: Option<usize> = input.text.trim().parse().ok();
    match input.mode {
        0 => password(length.unwrap_or(24), Characters::default()),
        1 => random_key(length.unwrap_or(32), false),
        _ => random_key(length.unwrap_or(32), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_ids() {
        assert_eq!(
            hash(&Input::new("abc")).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash(&Input {
                mode: 2,
                ..Input::new("abc")
            })
            .unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        let all = hashes(b"abc");
        assert_eq!(all[1].hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
        let mac = hmac(&Input {
            second: "key".into(),
            ..Input::new("The quick brown fox jumps over the lazy dog")
        })
        .unwrap();
        assert_eq!(
            mac,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        let uuids = ids(&Input::new("3")).unwrap();
        let first = uuids.lines().next().unwrap();
        assert_eq!(uuids.lines().count(), 3);
        assert_eq!((first.len(), &first[14..15]), (36, "4"), "version 4");
        let ulid = ids(&Input {
            mode: 1,
            ..Input::new("1")
        })
        .unwrap();
        assert_eq!(ulid.len(), 26);
    }

    #[test]
    fn a_checksum_is_found_in_any_form() {
        let all = hashes(b"abc");
        let sha256 = &all[2];
        assert_eq!(
            matching(&sha256.hex.to_uppercase(), &all).unwrap().name,
            "SHA-256"
        );
        assert_eq!(
            matching(&format!("sha256:{}", sha256.hex), &all)
                .unwrap()
                .name,
            "SHA-256"
        );
        let sri = format!("sha384-{}", all[3].base64);
        assert_eq!(matching(&sri, &all).unwrap().name, "SHA-384");
        assert!(matching("deadbeef", &all).is_none());
        let path = std::env::temp_dir().join(format!("ph-utils-hash-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let (size, from_file) = file_hashes(&path).unwrap();
        assert_eq!((size, from_file), (3, all));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn webhook_signatures_are_checked_as_sent() {
        let mac = hmac_of(MacAlgorithm::Sha256, b"key", b"body");
        assert!(signature_matches(
            &format!("sha256={}", ::hex::encode(&mac)),
            &mac
        ));
        assert!(signature_matches(&STANDARD.encode(&mac), &mac));
        assert!(signature_matches(
            &format!("v1,{}", STANDARD.encode(&mac)),
            &mac
        ));
        assert!(!signature_matches("sha256=00", &mac));
        assert_eq!(hmac_of(MacAlgorithm::Sha1, b"key", b"body").len(), 20);
    }

    #[test]
    fn ids_say_when_they_were_made() {
        let v7 = new_id(IdKind::UuidV7).unwrap();
        let facts = read_id(&v7).unwrap();
        assert_eq!(facts.kind, "UUID v7");
        let made = facts.made.unwrap();
        assert!((jiff::Timestamp::now().as_second() - made.as_second()).abs() < 5);
        let ulid = read_id(&new_id(IdKind::Ulid).unwrap()).unwrap();
        assert!(ulid.made.is_some());
        // A known version 1: 1998-02-04, from RFC 9562's example.
        let v1 = read_id("C232AB00-9414-11EC-B3C8-9F6BDECED846").unwrap();
        assert_eq!(
            v1.made.unwrap().strftime("%Y-%m-%d").to_string(),
            "2022-02-22"
        );
        assert_eq!(
            read_id(&new_id(IdKind::UuidV4).unwrap()).unwrap().made,
            None
        );
        assert!(
            read_id("00000000-0000-0000-0000-000000000000")
                .unwrap()
                .kind
                .contains("nil")
        );
        assert!(read_id("hello").is_err());
    }

    #[test]
    fn passwords_have_every_kind_asked_for() {
        let characters = Characters {
            symbols: false,
            unambiguous: true,
            ..Characters::default()
        };
        for _ in 0..50 {
            let made = password(8, characters).unwrap();
            assert_eq!(made.chars().count(), 8);
            assert!(made.chars().any(|c| c.is_ascii_digit()));
            assert!(
                made.chars()
                    .all(|c| c.is_ascii_alphanumeric() && !"lI1O0o".contains(c))
            );
        }
        assert!(characters.bits(8) > 40.0);
        let none = Characters {
            lower: false,
            upper: false,
            digits: false,
            symbols: false,
            unambiguous: false,
        };
        assert!(password(8, none).is_err());
        assert_eq!(random_key(16, false).unwrap().len(), 32);
    }
}
