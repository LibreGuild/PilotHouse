//! Hashes, HMAC, and random ids (UUID v4, ULID).

use anyhow::Result;
use base64::Engine as _;
use hmac::{KeyInit as _, Mac as _};
use sha2::Digest as _;

use crate::Input;

pub fn hash(input: &Input) -> Result<String> {
    let bytes = input.text.as_bytes();
    Ok(match input.mode {
        0 => ::hex::encode(sha2::Sha256::digest(bytes)),
        1 => ::hex::encode(sha2::Sha512::digest(bytes)),
        _ => ::hex::encode(md5::Md5::digest(bytes)),
    })
}

pub fn hmac(input: &Input) -> Result<String> {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(input.second.as_bytes())?;
    mac.update(input.text.as_bytes());
    let bytes = mac.finalize().into_bytes();
    Ok(if input.mode == 0 {
        ::hex::encode(bytes)
    } else {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    })
}

/// `count` new UUIDs (v4) or ULIDs, one per line (5 by default, 100 at most).
pub fn ids(input: &Input) -> Result<String> {
    let count = input
        .text
        .trim()
        .parse::<usize>()
        .unwrap_or(5)
        .clamp(1, 100);
    let ids: Result<Vec<String>> = (0..count)
        .map(|_| if input.mode == 0 { uuid_v4() } else { ulid() })
        .collect();
    Ok(ids?.join("\n"))
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|err| anyhow::anyhow!("no randomness: {err}"))?;
    Ok(bytes)
}

fn uuid_v4() -> Result<String> {
    let mut bytes = random::<16>()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// 48 bits of milliseconds then 80 random bits, in Crockford's Base32.
fn ulid() -> Result<String> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let random = u128::from_be_bytes({
        let mut bytes = [0u8; 16];
        bytes[6..].copy_from_slice(&random::<10>()?);
        bytes
    });
    let value = (millis << 80) | random;
    Ok((0..26)
        .rev()
        .map(|ix| ALPHABET[((value >> (ix * 5)) & 0x1f) as usize] as char)
        .collect())
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
}
