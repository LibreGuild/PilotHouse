//! X.509 certificates and QR codes.

use anyhow::{Result, anyhow, bail};
use sha2::Digest as _;
use x509_parser::prelude::*;

use crate::Input;

/// What each certificate of a PEM block (or a chain) says: who it is for,
/// who signed it, when it is valid, its key and names, and its fingerprint.
pub fn x509(input: &Input) -> Result<String> {
    let mut described = Vec::new();
    for pem in Pem::iter_from_buffer(input.text.trim().as_bytes()) {
        let pem = pem.map_err(|err| anyhow!("Not a PEM block: {err}"))?;
        if pem.label != "CERTIFICATE" {
            continue;
        }
        let cert = pem
            .parse_x509()
            .map_err(|err| anyhow!("Not a valid certificate: {err}"))?;
        described.push(describe(described.len() + 1, &cert, &pem.contents));
    }
    if described.is_empty() {
        bail!("Paste a certificate in PEM form (-----BEGIN CERTIFICATE-----).");
    }
    Ok(described.join("\n\n"))
}

fn describe(number: usize, cert: &X509Certificate<'_>, der: &[u8]) -> String {
    let registry = x509_parser::objects::oid_registry();
    let name = |oid: &x509_parser::der_parser::Oid<'_>| {
        x509_parser::objects::oid2sn(oid, registry)
            .map(str::to_string)
            .unwrap_or_else(|_| oid.to_id_string())
    };
    let validity = cert.validity();
    let (from, to) = (
        validity.not_before.timestamp(),
        validity.not_after.timestamp(),
    );
    let now = jiff::Timestamp::now().as_second();
    let state = if now < from {
        "not valid yet".to_string()
    } else if now > to {
        format!("expired {} days ago", (now - to) / 86_400)
    } else {
        format!("valid, {} days left", (to - now) / 86_400)
    };
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
        Ok(Some(san)) => san
            .value
            .general_names
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        _ => "none".to_string(),
    };
    let ca = match cert.basic_constraints() {
        Ok(Some(constraints)) if constraints.value.ca => "yes",
        _ => "no",
    };
    let fingerprint = sha2::Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    [
        format!("Certificate {number}"),
        format!("Subject:    {}", cert.subject()),
        format!("Issuer:     {}", cert.issuer()),
        format!("Valid from: {}", day(from)),
        format!("Valid to:   {} ({state})", day(to)),
        format!("Names:      {names}"),
        format!("Key:        {key_text}"),
        format!("Signature:  {}", name(&cert.signature_algorithm.algorithm)),
        format!("CA:         {ca}"),
        format!("Serial:     {}", cert.raw_serial_as_string()),
        format!("SHA-256:    {fingerprint}"),
    ]
    .join("\n")
}

fn day(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds)
        .map(|time| time.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| seconds.to_string())
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
    fn draws_a_qr_code() {
        let out = qr(&Input::new("https://example.test")).unwrap();
        assert!(out.lines().count() > 10);
        assert!(out.contains('█') || out.contains('▀') || out.contains('▄'));
        assert!(qr(&Input::new("")).is_err());
    }
}
