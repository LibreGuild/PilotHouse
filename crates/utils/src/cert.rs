//! X.509 certificates and QR codes.

use anyhow::{Result, anyhow, bail};
use sha2::Digest as _;
use x509_parser::prelude::*;

use crate::Input;

/// What a certificate says, for the dialog's card and the agents' text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    pub subject: String,
    /// The subject's common name (`CN=`), what it is usually called.
    pub common_name: Option<String>,
    pub issuer: String,
    pub issuer_name: Option<String>,
    /// Unix seconds.
    pub not_before: i64,
    pub not_after: i64,
    /// Its other names (DNS names, addresses).
    pub names: Vec<String>,
    pub key: String,
    pub signature: String,
    pub ca: bool,
    /// Signed by itself: the end of a chain (a root, or a test certificate).
    pub self_signed: bool,
    pub serial: String,
    pub sha256: String,
    pub sha1: String,
}

impl Certificate {
    /// Days until it expires; negative once it has.
    pub fn days_left(&self, now: i64) -> i64 {
        (self.not_after - now).div_euclid(86_400)
    }
}

/// The certificates in `bytes`: PEM text (one or a chain), or one in DER
/// (a `.cer` or `.der` file).
pub fn certificates(bytes: &[u8]) -> Result<Vec<Certificate>> {
    let mut found = Vec::new();
    let text = String::from_utf8_lossy(bytes);
    if text.contains("-----BEGIN") {
        for pem in Pem::iter_from_buffer(text.trim().as_bytes()) {
            let pem = pem.map_err(|err| anyhow!("Not a PEM block: {err}"))?;
            if pem.label != "CERTIFICATE" {
                continue;
            }
            let cert = pem
                .parse_x509()
                .map_err(|err| anyhow!("Not a valid certificate: {err}"))?;
            found.push(read(&cert, &pem.contents));
        }
        if found.is_empty() {
            bail!(
                "There is no certificate here (-----BEGIN CERTIFICATE-----); a key or request is not read."
            );
        }
        return Ok(found);
    }
    let (_, cert) = X509Certificate::from_der(bytes).map_err(|_| {
        anyhow!("Paste a certificate in PEM form (-----BEGIN CERTIFICATE-----), or open a .pem, .crt or .cer file.")
    })?;
    Ok(vec![read(&cert, bytes)])
}

fn read(cert: &X509Certificate<'_>, der: &[u8]) -> Certificate {
    let registry = x509_parser::objects::oid_registry();
    let name = |oid: &x509_parser::der_parser::Oid<'_>| {
        x509_parser::objects::oid2sn(oid, registry)
            .map(str::to_string)
            .unwrap_or_else(|_| oid.to_id_string())
    };
    let common = |name: &X509Name<'_>| {
        name.iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(str::to_string)
    };
    let validity = cert.validity();
    let key = cert.public_key();
    let key_text = match key.parsed() {
        Ok(parsed) if parsed.key_size() > 0 => {
            format!(
                "{} {} bits",
                name(&key.algorithm.algorithm),
                parsed.key_size()
            )
        }
        _ => name(&key.algorithm.algorithm),
    };
    let names = match cert.subject_alternative_name() {
        Ok(Some(san)) => {
            san.value
                .general_names
                .iter()
                .map(|general| match general {
                    GeneralName::DNSName(dns) => dns.to_string(),
                    GeneralName::IPAddress(bytes) => match bytes.len() {
                        4 => std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3])
                            .to_string(),
                        16 => <[u8; 16]>::try_from(*bytes)
                            .map(|octets| std::net::Ipv6Addr::from(octets).to_string())
                            .unwrap_or_else(|_| general.to_string()),
                        _ => general.to_string(),
                    },
                    other => other.to_string(),
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let ca = matches!(cert.basic_constraints(), Ok(Some(constraints)) if constraints.value.ca);
    let fingerprint = |digest: &[u8]| {
        digest
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    Certificate {
        subject: cert.subject().to_string(),
        common_name: common(cert.subject()),
        issuer: cert.issuer().to_string(),
        issuer_name: common(cert.issuer()),
        not_before: validity.not_before.timestamp(),
        not_after: validity.not_after.timestamp(),
        names,
        key: key_text,
        signature: name(&cert.signature_algorithm.algorithm),
        ca,
        self_signed: cert.subject() == cert.issuer(),
        serial: cert.raw_serial_as_string(),
        sha256: fingerprint(&sha2::Sha256::digest(der)),
        sha1: fingerprint(&sha1::Sha1::digest(der)),
    }
}

/// What each certificate of a PEM block (or a chain) says: who it is for,
/// who signed it, when it is valid, its key and names, and its fingerprint.
pub fn x509(input: &Input) -> Result<String> {
    let now = jiff::Timestamp::now().as_second();
    let described: Vec<String> = certificates(input.text.as_bytes())?
        .iter()
        .enumerate()
        .map(|(ix, cert)| {
            let state = if now < cert.not_before {
                "not valid yet".to_string()
            } else if now > cert.not_after {
                format!("expired {} days ago", -cert.days_left(now))
            } else {
                format!("valid, {} days left", cert.days_left(now))
            };
            [
                format!("Certificate {}", ix + 1),
                format!("Subject:    {}", cert.subject),
                format!("Issuer:     {}", cert.issuer),
                format!("Valid from: {}", day(cert.not_before)),
                format!("Valid to:   {} ({state})", day(cert.not_after)),
                format!(
                    "Names:      {}",
                    if cert.names.is_empty() {
                        "none".to_string()
                    } else {
                        cert.names.join(", ")
                    }
                ),
                format!("Key:        {}", cert.key),
                format!("Signature:  {}", cert.signature),
                format!("CA:         {}", if cert.ca { "yes" } else { "no" }),
                format!("Serial:     {}", cert.serial),
                format!("SHA-256:    {}", cert.sha256),
            ]
            .join("\n")
        })
        .collect();
    Ok(described.join("\n\n"))
}

/// A day and time in UTC.
pub fn day(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds)
        .map(|time| time.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| seconds.to_string())
}

/// A QR code's squares, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qr {
    /// Squares on a side.
    pub width: usize,
    pub dark: Vec<bool>,
    pub version: i16,
}

/// How much of a code can be damaged and still read: 7, 15, 25 or 30 %.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Correction {
    Low,
    #[default]
    Medium,
    Quartile,
    High,
}

/// The code for `text`.
pub fn qr_code(text: &str, correction: Correction) -> Result<Qr> {
    use qrcode::EcLevel;
    if text.is_empty() {
        bail!("Type the text or link to encode.");
    }
    let level = match correction {
        Correction::Low => EcLevel::L,
        Correction::Medium => EcLevel::M,
        Correction::Quartile => EcLevel::Q,
        Correction::High => EcLevel::H,
    };
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), level)
        .map_err(|err| anyhow!("Cannot make a QR code of this ({err}): it may be too long."))?;
    let version = match code.version() {
        qrcode::Version::Normal(n) | qrcode::Version::Micro(n) => n,
    };
    Ok(Qr {
        width: code.width(),
        dark: code
            .to_colors()
            .into_iter()
            .map(|color| color == qrcode::Color::Dark)
            .collect(),
        version,
    })
}

/// The code as a PNG: dark squares on white, `scale` pixels each, with the
/// four-square margin a reader needs.
pub fn qr_png(code: &Qr, scale: u32) -> Result<Vec<u8>> {
    const MARGIN: u32 = 4;
    let side = (code.width as u32 + MARGIN * 2) * scale;
    let picture = image::GrayImage::from_fn(side, side, |x, y| {
        let (column, row) = (x / scale, y / scale);
        let inside = |at: u32| at >= MARGIN && at < MARGIN + code.width as u32;
        let dark = inside(column)
            && inside(row)
            && code.dark[(row - MARGIN) as usize * code.width + (column - MARGIN) as usize];
        image::Luma([if dark { 0 } else { 255 }])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// The text as a QR code drawn with block characters, to scan from the
/// screen, dark or light.
pub fn qr(input: &Input) -> Result<String> {
    use qrcode::render::unicode::Dense1x2;
    if input.text.is_empty() {
        bail!("Type the text or link to encode.");
    }
    let code = qrcode::QrCode::new(input.text.as_bytes())
        .map_err(|err| anyhow!("Cannot make a QR code of this: {err}"))?;
    let (dark, light) = if input.mode == 0 {
        (Dense1x2::Light, Dense1x2::Dark)
    } else {
        (Dense1x2::Dark, Dense1x2::Light)
    };
    Ok(code
        .render::<Dense1x2>()
        .dark_color(dark)
        .light_color(light)
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-signed certificate for `example.test` (made for this test).
    const PEM: &str = include_str!("../tests/example.test.pem");

    #[test]
    fn describes_a_certificate() {
        let out = x509(&Input::new(PEM)).unwrap();
        assert!(out.contains("Subject:    CN=example.test"), "{out}");
        assert!(out.contains("example.test"));
        assert!(out.contains("SHA-256:    "));
        assert!(x509(&Input::new("hello")).is_err());
    }

    #[test]
    fn reads_der_and_chains() {
        let pem = x509_parser::pem::parse_x509_pem(PEM.as_bytes()).unwrap().1;
        let der = certificates(&pem.contents).unwrap();
        let from_pem = certificates(PEM.as_bytes()).unwrap();
        assert_eq!(der, from_pem);
        assert_eq!(der[0].common_name.as_deref(), Some("example.test"));
        assert!(der[0].self_signed);
        assert_eq!(der[0].sha1.split(':').count(), 20);
        let chain = format!("{PEM}\n{PEM}");
        assert_eq!(certificates(chain.as_bytes()).unwrap().len(), 2);
        assert!(
            certificates(b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----").is_err()
        );
    }

    #[test]
    fn makes_a_qr_picture() {
        let code = qr_code("https://example.test", Correction::Medium).unwrap();
        assert_eq!(code.dark.len(), code.width * code.width);
        assert!(code.version >= 1);
        let png = qr_png(&code, 4).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(qr_code("", Correction::Low).is_err());
    }

    #[test]
    fn draws_a_qr_code() {
        let out = qr(&Input::new("https://example.test")).unwrap();
        assert!(out.lines().count() > 10);
        assert!(out.contains('█') || out.contains('▀') || out.contains('▄'));
        assert!(qr(&Input::new("")).is_err());
    }
}
