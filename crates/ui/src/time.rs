//! Times shown to people.

/// "3 days ago" from a Unix time.
pub fn ago(time: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|now| now.as_secs() as i64)
        .unwrap_or(time);
    let seconds = (now - time).max(0);
    let (value, unit) = match seconds {
        0..60 => return "just now".into(),
        60..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        86_400..2_592_000 => (seconds / 86_400, "day"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    format!("{value} {unit}{} ago", if value == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_long_ago() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 120), "2 minutes ago");
        assert_eq!(ago(now - 3_600), "1 hour ago");
        assert_eq!(ago(now - 3 * 86_400), "3 days ago");
    }
}
