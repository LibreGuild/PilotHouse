//! Unix times and dates, and what a cron line means.

use anyhow::{Context as _, Result, bail};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan as _, Zoned};

use crate::Input;

/// What a number was read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
    /// Not a number: a date, or nothing (now).
    Date,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Unit::Seconds => "seconds",
            Unit::Milliseconds => "milliseconds",
            Unit::Microseconds => "microseconds",
            Unit::Nanoseconds => "nanoseconds",
            Unit::Date => "a date",
        }
    }
}

/// The moment `text` names: a Unix time (its unit told by its size, as
/// logs and databases mix them), a date or date-time (local time unless it
/// says its zone), or now when empty.
pub fn moment(text: &str) -> Result<(Timestamp, Unit)> {
    let text = text.trim();
    if text.is_empty() {
        return Ok((Timestamp::now(), Unit::Date));
    }
    let digits: String = text.chars().filter(|c| *c != '_' && *c != ',').collect();
    if let Ok(number) = digits.parse::<i64>() {
        let size = number.unsigned_abs();
        return Ok(if size >= 100_000_000_000_000_000 {
            (
                Timestamp::from_nanosecond(number as i128)?,
                Unit::Nanoseconds,
            )
        } else if size >= 100_000_000_000_000 {
            (Timestamp::from_microsecond(number)?, Unit::Microseconds)
        } else if size >= 100_000_000_000 {
            (Timestamp::from_millisecond(number)?, Unit::Milliseconds)
        } else {
            (Timestamp::from_second(number)?, Unit::Seconds)
        });
    }
    if let Ok(number) = digits.parse::<f64>()
        && number.is_finite()
    {
        // Seconds with a fraction, as Python's time.time() prints them.
        let nanos = (number * 1e9).round() as i128;
        return Ok((Timestamp::from_nanosecond(nanos)?, Unit::Seconds));
    }
    if let Ok(at) = text.parse::<Timestamp>() {
        return Ok((at, Unit::Date));
    }
    if let Ok(zoned) = text.parse::<Zoned>() {
        return Ok((zoned.timestamp(), Unit::Date));
    }
    if let Ok(zoned) = jiff::fmt::rfc2822::parse(text) {
        return Ok((zoned.timestamp(), Unit::Date));
    }
    let civil: jiff::civil::DateTime = text
        .replace(' ', "T")
        .parse()
        .or_else(|_| {
            text.parse::<jiff::civil::Date>()
                .map(|date| date.at(0, 0, 0, 0))
        })
        .context("Give a Unix time, or a date like 2026-10-24 or 2026-10-24T09:00")?;
    Ok((civil.to_zoned(TimeZone::system())?.timestamp(), Unit::Date))
}

/// The time zone names this computer knows, in order.
pub fn zones() -> Vec<String> {
    let mut names: Vec<String> = jiff::tz::db()
        .available()
        .map(|name| name.to_string())
        .filter(|name| name.contains('/') || name == "UTC")
        .collect();
    names.sort();
    names.dedup();
    names
}

/// `at` in `zone` as a person reads it: `2026-10-24 09:00:00 CEST (+02:00)`.
pub fn in_zone(at: Timestamp, zone: &TimeZone) -> String {
    at.to_zoned(zone.clone())
        .strftime("%Y-%m-%d %H:%M:%S %Z (%:z)")
        .to_string()
}

/// `at` as HTTP headers and email write it (`Sat, 24 Oct 2026 07:00:00 GMT`).
pub fn http_date(at: Timestamp) -> String {
    at.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// How far `at` is from now, in words: "3 hours ago", "in 2 days", "now".
pub fn from_now(at: Timestamp) -> String {
    let seconds = Timestamp::now().as_second() - at.as_second();
    match seconds {
        -59..=59 => "now".into(),
        s if s > 0 => format!("{} ago", plain_duration(s)),
        s => format!("in {}", plain_duration(-s)),
    }
}

/// A unix time (seconds or milliseconds), a date, or now, shown in UTC,
/// local time and an optional zone.
pub fn convert(input: &Input) -> Result<String> {
    let (at, _) = moment(&input.text)?;
    let mut out = vec![
        format!("Unix seconds: {}", at.as_second()),
        format!("Unix milliseconds: {}", at.as_millisecond()),
        format!("UTC: {at}"),
        format!("Local: {}", in_zone(at, &TimeZone::system())),
    ];
    let zone = input.second.trim();
    if !zone.is_empty() {
        let tz = TimeZone::get(zone).with_context(|| format!("unknown time zone `{zone}`"))?;
        out.push(format!("{zone}: {}", in_zone(at, &tz)));
    }
    out.push(match from_now(at).as_str() {
        "now" => "That is now.".to_string(),
        words if words.starts_with("in ") => format!("That is {words}."),
        words => format!("That was {words}."),
    });
    Ok(out.join("\n"))
}

fn plain_duration(seconds: i64) -> String {
    // A day and some hours says more than "1 day" for what is 47 hours off.
    if (86_400..172_800).contains(&seconds) {
        let hours = (seconds - 86_400) / 3_600;
        return match hours {
            0 => "1 day".into(),
            1 => "1 day 1 hour".into(),
            hours => format!("1 day {hours} hours"),
        };
    }
    let (value, unit) = match seconds {
        0..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        86_400..5_184_000 => (seconds / 86_400, "day"),
        5_184_000..63_072_000 => (seconds / 2_629_746, "month"),
        _ => (seconds / 31_556_952, "year"),
    };
    format!("{value} {unit}{}", if value == 1 { "" } else { "s" })
}

/// Which clock a cron line is read on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Clock {
    #[default]
    Local,
    /// As GitHub Actions and most servers run them.
    Utc,
}

/// The next `count` times a cron line fires on `clock`, up to five years
/// ahead (a 29 February comes once in four).
pub fn cron_next(line: &str, count: usize, clock: Clock) -> Result<Vec<Zoned>> {
    let schedule = Cron::parse(line)?;
    let zone = match clock {
        Clock::Local => TimeZone::system(),
        Clock::Utc => TimeZone::UTC,
    };
    let mut at = Timestamp::now()
        .to_zoned(zone)
        .with()
        .second(0)
        .subsec_nanosecond(0)
        .build()?;
    let mut found = Vec::new();
    // A minute at a time; a day at a time where the day cannot match.
    let end = at.checked_add((5 * 366).days())?;
    while at < end && found.len() < count {
        if !schedule.day_matches(&at) || !schedule.fields[3][at.month() as usize] {
            at = at.checked_add(1.day())?.with().hour(0).minute(0).build()?;
            if schedule.matches(&at) {
                found.push(at.clone());
            }
            continue;
        }
        at = at.checked_add(1.minute())?;
        if schedule.matches(&at) {
            found.push(at.clone());
        }
    }
    if found.is_empty() {
        bail!("It never fires in the next five years.");
    }
    Ok(found)
}

/// The next five times a five-field cron line fires, in local time.
pub fn cron(input: &Input) -> Result<String> {
    let line = input.text.trim();
    let runs = cron_next(line, 5, Clock::Local)?;
    Ok(format!(
        "{}\nNext runs (local time):\n{}",
        cron_describe(line)?,
        runs.iter()
            .map(|at| at.strftime("%a %Y-%m-%d %H:%M").to_string())
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// A cron line said in words: "Every 15 minutes, 09:00 to 17:59, Monday
/// through Friday".
pub fn cron_describe(line: &str) -> Result<String> {
    let fields = normalized(line)?;
    let [minute, hour, day, month, weekday] = [0, 1, 2, 3, 4].map(|ix| fields[ix].as_str());
    let time = match (minute, hour) {
        ("*", "*") => "Every minute".to_string(),
        (minute, "*") => match minute.strip_prefix("*/") {
            Some(step) => format!("Every {step} minutes"),
            None => format!("At minute {} of every hour", list(minute, None)),
        },
        (minute, hour) if is_number(minute) && is_number(hour) => {
            format!("At {:0>2}:{:0>2}", hour, minute)
        }
        (minute, hour) if is_number(minute) => {
            let at = |hour: &str| format!("{hour:0>2}:{minute:0>2}");
            if let Some((range, step)) = hour.split_once('/') {
                let past = if minute == "0" {
                    "on the hour".to_string()
                } else {
                    format!("at minute {minute}")
                };
                let from = match range.split_once('-') {
                    Some((from, to)) => format!(" from {} through {}", at(from), at(to)),
                    None if range != "*" => format!(" from {}", at(range)),
                    None => String::new(),
                };
                format!("Every {step} hours{from}, {past}")
            } else if let Some((from, to)) = hour.split_once('-').filter(|_| !hour.contains(',')) {
                format!("Every hour from {} through {}", at(from), at(to))
            } else if hour.split(',').all(is_number) {
                let times: Vec<String> = hour.split(',').map(at).collect();
                format!("At {}", list(&times.join(","), None))
            } else {
                format!("At minute {minute} of {}", phrase(hour, "hour", None))
            }
        }
        ("*", hour) => format!("Every minute of {}", phrase(hour, "hour", None)),
        (minute, hour) => format!(
            "{} of {}",
            capitalized(&phrase(minute, "minute", None)),
            phrase(hour, "hour", None)
        ),
    };
    let mut parts = vec![time];
    match (day, weekday) {
        ("*", "*") => {}
        ("*", weekday) => parts.push(format!("on {}", list(weekday, Some(&WEEKDAYS)))),
        (day, "*") => parts.push(format!("on {}", phrase(day, "day", None))),
        (day, weekday) => parts.push(format!(
            "on {} or on {}",
            phrase(day, "day", None),
            list(weekday, Some(&WEEKDAYS))
        )),
    }
    if month != "*" {
        parts.push(format!("in {}", list(month, Some(&MONTHS))));
    }
    return Ok(parts.join(", "));

    fn is_number(text: &str) -> bool {
        !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
    }

    fn capitalized(text: &str) -> String {
        let mut chars = text.chars();
        chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect())
            .unwrap_or_default()
    }

    /// "every 2 hours from 9 through 17", "hours 9 through 17".
    fn phrase(text: &str, unit: &str, names: Option<&[&str]>) -> String {
        if let Some((range, step)) = text.split_once('/') {
            let from = match range {
                "*" => String::new(),
                range => format!(" from {}", list(range, names)),
            };
            return format!("every {step} {unit}s{from}");
        }
        if text.contains(',') || text.contains('-') {
            format!("{unit}s {}", list(text, names))
        } else {
            format!("{unit} {}", list(text, names))
        }
    }

    /// "Monday through Friday", "1, 15 and 30".
    fn list(text: &str, names: Option<&[&str]>) -> String {
        let name = |value: &str| match (names, value.parse::<usize>()) {
            (Some(names), Ok(ix)) => names.get(ix).copied().unwrap_or(value).to_string(),
            _ => value.to_string(),
        };
        let items: Vec<String> = text
            .split(',')
            .map(|item| match item.split_once('-') {
                Some((from, to)) => format!("{} through {}", name(from), name(to)),
                None => name(item),
            })
            .collect();
        match items.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    }
}

const WEEKDAYS: [&str; 8] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 13] = [
    "",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The five fields of a cron line, with `@daily` and friends spelled out,
/// month and day names as numbers and `?` as `*`.
pub fn normalized(line: &str) -> Result<[String; 5]> {
    let line = line.trim();
    let line = match line.to_ascii_lowercase().as_str() {
        "@yearly" | "@annually" => "0 0 1 1 *",
        "@monthly" => "0 0 1 * *",
        "@weekly" => "0 0 * * 0",
        "@daily" | "@midnight" => "0 0 * * *",
        "@hourly" => "0 * * * *",
        "@reboot" => bail!("@reboot runs when the computer starts, not at a time."),
        _ => line,
    };
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() == 6 {
        bail!(
            "This line has six fields; the first is probably seconds (Quartz, Spring). Cron's five are: minute hour day month weekday."
        );
    }
    if parts.len() != 5 {
        bail!("A cron line has five fields: minute hour day month weekday.");
    }
    let named = |text: &str, names: &[&str], first: usize| -> String {
        let mut text = text.to_ascii_uppercase();
        for (ix, name) in names.iter().enumerate() {
            text = text.replace(&name[..3].to_ascii_uppercase(), &(ix + first).to_string());
        }
        text
    };
    let unknown = |text: &str| {
        text.chars()
            .find(|c| c.is_ascii_alphabetic() || matches!(c, 'L' | 'W' | '#'))
    };
    let fields = [
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].replace('?', "*"),
        named(parts[3], &MONTHS[1..], 1),
        named(&parts[4].replace('?', "*"), &WEEKDAYS[..7], 0),
    ];
    for field in &fields {
        if let Some(c) = unknown(field) {
            bail!("`{field}`: `{c}` is not read here (L, W and # are Quartz's, not cron's).");
        }
    }
    Ok(fields)
}

/// Minute, hour, day of month, month, weekday: the values each allows.
struct Cron {
    fields: [Vec<bool>; 5],
    /// Both day fields were limited: a day matching either counts.
    either_day: bool,
}

impl Cron {
    fn parse(line: &str) -> Result<Self> {
        let parts = normalized(line)?;
        let names = ["minute", "hour", "day", "month", "weekday"];
        let ranges = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
        let mut fields: [Vec<bool>; 5] = Default::default();
        for (ix, (part, (low, high))) in parts.iter().zip(ranges).enumerate() {
            fields[ix] =
                field(part, low, high).with_context(|| format!("the {} (`{part}`)", names[ix]))?;
        }
        // 7 is Sunday too.
        if fields[4][7] {
            fields[4][0] = true;
        }
        Ok(Self {
            fields,
            either_day: parts[2] != "*" && parts[4] != "*",
        })
    }

    fn day_matches(&self, at: &Zoned) -> bool {
        let [_, _, day, _, weekday] = &self.fields;
        let day_ok = day[at.day() as usize];
        let weekday_ok = weekday[at.weekday().to_sunday_zero_offset() as usize];
        if self.either_day {
            day_ok || weekday_ok
        } else {
            day_ok && weekday_ok
        }
    }

    fn matches(&self, at: &Zoned) -> bool {
        let [minute, hour, _, month, _] = &self.fields;
        minute[at.minute() as usize]
            && hour[at.hour() as usize]
            && month[at.month() as usize]
            && self.day_matches(at)
    }
}

/// One field: `*`, `5`, `1-5`, `*/15`, `1-30/2`, and lists of those.
fn field(text: &str, low: usize, high: usize) -> Result<Vec<bool>> {
    let mut allowed = vec![false; high + 1];
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (range, step.parse::<usize>().context("a step is a number")?),
            None => (item, 1),
        };
        if step == 0 {
            bail!("a step of 0");
        }
        let (from, to) = match range {
            "*" => (low, high),
            range => match range.split_once('-') {
                Some((from, to)) => (
                    from.parse().context("not a number")?,
                    to.parse().context("not a number")?,
                ),
                None => {
                    let value: usize = range.parse().context("not a number")?;
                    (value, if item.contains('/') { high } else { value })
                }
            },
        };
        if from < low || to > high || from > to {
            bail!("{from}-{to} is outside {low}-{high}");
        }
        for value in (from..=to).step_by(step) {
            allowed[value] = true;
        }
    }
    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_times() {
        let out = convert(&Input {
            second: "UTC".into(),
            ..Input::new("1700000000")
        })
        .unwrap();
        assert!(
            out.contains("UTC: 2023-11-14T22:13:20Z")
                && out.contains("milliseconds: 1700000000000")
        );
        assert!(
            convert(&Input::new("1700000000000"))
                .unwrap()
                .contains("Unix seconds: 1700000000")
        );
        assert!(convert(&Input::new("2026-10-24")).is_ok());
        assert!(convert(&Input::new("soon")).is_err());
    }

    #[test]
    fn tells_units_and_forms_apart() {
        let second = |text: &str| moment(text).unwrap().0.as_second();
        assert_eq!(moment("1700000000123456").unwrap().1, Unit::Microseconds);
        assert_eq!(second("1700000000123456789"), 1_700_000_000);
        assert_eq!(second("1700000000.5"), 1_700_000_000);
        assert_eq!(second("1_700_000_000"), 1_700_000_000);
        assert_eq!(second("Tue, 14 Nov 2023 22:13:20 +0000"), 1_700_000_000);
        assert_eq!(second("2023-11-14T22:13:20Z"), 1_700_000_000);
        assert!(moment("2026-10-24 09:00").is_ok());
        assert_eq!(
            http_date(Timestamp::from_second(1_700_000_000).unwrap()),
            "Tue, 14 Nov 2023 22:13:20 GMT"
        );
        assert!(zones().iter().any(|zone| zone == "Europe/Berlin"));
    }

    #[test]
    fn reads_cron_lines() {
        let weekdays = Cron::parse("*/15 9-17 * * 1-5").unwrap();
        let monday_9_30 = jiff::civil::date(2026, 10, 26)
            .at(9, 30, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap();
        let sunday = jiff::civil::date(2026, 10, 25)
            .at(9, 30, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap();
        assert!(weekdays.matches(&monday_9_30) && !weekdays.matches(&sunday));
        assert!(
            Cron::parse("0 0 * * 7")
                .unwrap()
                .matches(&sunday.with().hour(0).minute(0).build().unwrap())
        );
        assert!(Cron::parse("61 * * * *").is_err());
        assert!(Cron::parse("* *").is_err());
        assert_eq!(cron(&Input::new("0 9 * * 1")).unwrap().lines().count(), 7);
        // Names, macros and Quartz's question mark.
        assert!(
            Cron::parse("0 9 ? JAN-MAR MON-FRI").unwrap().matches(
                &jiff::civil::date(2026, 1, 5)
                    .at(9, 0, 0, 0)
                    .to_zoned(TimeZone::UTC)
                    .unwrap()
            )
        );
        assert_eq!(cron_next("@daily", 3, Clock::Utc).unwrap().len(), 3);
        assert!(
            normalized("0 0 12 * * ?")
                .unwrap_err()
                .to_string()
                .contains("seconds")
        );
        assert!(normalized("0 0 L * *").is_err());
        // A line that fires rarely is still found quickly.
        assert_eq!(cron_next("0 0 29 2 *", 1, Clock::Utc).unwrap()[0].day(), 29);
    }

    #[test]
    fn says_cron_lines_in_words() {
        let said = |line: &str| cron_describe(line).unwrap();
        assert_eq!(said("* * * * *"), "Every minute");
        assert_eq!(said("*/15 * * * *"), "Every 15 minutes");
        assert_eq!(said("30 9 * * *"), "At 09:30");
        assert_eq!(said("0 9 * * 1-5"), "At 09:00, on Monday through Friday");
        assert_eq!(
            said("*/15 9-17 * * *"),
            "Every 15 minutes of hours 9 through 17"
        );
        assert_eq!(said("0 */2 * * *"), "Every 2 hours, on the hour");
        assert_eq!(said("0 9-17 * * *"), "Every hour from 09:00 through 17:00");
        assert_eq!(said("0 9,12,18 * * *"), "At 09:00, 12:00 and 18:00");
        assert_eq!(said("0 0 1 * *"), "At 00:00, on day 1");
        assert_eq!(said("@yearly"), "At 00:00, on day 1, in January");
        assert_eq!(said("0 12 * * SAT,SUN"), "At 12:00, on Saturday and Sunday");
    }
}
