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

/// How long ago `millis` was, as lists say it: *now*, *5m*, *3h*, *Tue*,
/// *Mar 4*.
pub fn age(millis: i64) -> String {
    let Ok(at) = jiff::Timestamp::from_millisecond(millis) else {
        return String::new();
    };
    let now = jiff::Timestamp::now();
    let minutes = (now.as_second() - at.as_second()) / 60;
    match minutes {
        ..1 => "now".into(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        1440..10080 => at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%a")
            .to_string(),
        _ => at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%b %-d")
            .to_string(),
    }
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
