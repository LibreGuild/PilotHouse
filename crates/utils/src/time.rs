//! Unix times and dates, and what a cron line means.

use anyhow::{Context as _, Result, bail};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan as _, Zoned};

use crate::Input;

/// A Unix time (seconds or milliseconds, told apart by size), a date, or
/// now, shown in UTC, local time and an optional zone.
pub fn convert(input: &Input) -> Result<String> {
    let text = input.text.trim();
    let at = if text.is_empty() {
        Timestamp::now()
    } else if let Ok(number) = text.parse::<i64>() {
        if number.abs() > 100_000_000_000 {
            Timestamp::from_millisecond(number)?
        } else {
            Timestamp::from_second(number)?
        }
    } else if let Ok(at) = text.parse::<Timestamp>() {
        at
    } else {
        // A date or date-time without a zone: local time.
        let civil: jiff::civil::DateTime = text
            .parse()
            .or_else(|_| {
                text.parse::<jiff::civil::Date>()
                    .map(|date| date.at(0, 0, 0, 0))
            })
            .context("Give a Unix time, or a date like 2026-10-24 or 2026-10-24T09:00")?;
        civil.to_zoned(TimeZone::system())?.timestamp()
    };
    let mut out = vec![
        format!("Unix seconds: {}", at.as_second()),
        format!("Unix milliseconds: {}", at.as_millisecond()),
        format!("UTC: {at}"),
        format!(
            "Local: {}",
            at.to_zoned(TimeZone::system())
                .strftime("%Y-%m-%d %H:%M:%S %Z (%:z)")
        ),
    ];
    let zone = input.second.trim();
    if !zone.is_empty() {
        let tz = TimeZone::get(zone).with_context(|| format!("unknown time zone `{zone}`"))?;
        out.push(format!(
            "{zone}: {}",
            at.to_zoned(tz).strftime("%Y-%m-%d %H:%M:%S %Z (%:z)")
        ));
    }
    let seconds = Timestamp::now().as_second() - at.as_second();
    out.push(match seconds {
        -59..=59 => "That is now.".into(),
        s if s > 0 => format!("That was {} ago.", plain_duration(s)),
        s => format!("That is in {}.", plain_duration(-s)),
    });
    Ok(out.join("\n"))
}

fn plain_duration(seconds: i64) -> String {
    let (value, unit) = match seconds {
        0..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    format!("{value} {unit}{}", if value == 1 { "" } else { "s" })
}

/// The next five times a five-field cron line fires, in local time.
pub fn cron(input: &Input) -> Result<String> {
    let schedule = Cron::parse(input.text.trim())?;
    let mut at = Zoned::now().with().second(0).subsec_nanosecond(0).build()?;
    let mut found = Vec::new();
    // A minute at a time, up to a year ahead.
    for _ in 0..(366 * 24 * 60) {
        at = at.checked_add(1.minute())?;
        if schedule.matches(&at) {
            found.push(at.strftime("%a %Y-%m-%d %H:%M").to_string());
            if found.len() == 5 {
                break;
            }
        }
    }
    if found.is_empty() {
        bail!("It never fires in the next year.");
    }
    Ok(format!("Next runs (local time):\n{}", found.join("\n")))
}

/// Minute, hour, day of month, month, weekday: the values each allows.
struct Cron {
    fields: [Vec<bool>; 5],
    /// Both day fields were limited: a day matching either counts.
    either_day: bool,
}

impl Cron {
    fn parse(line: &str) -> Result<Self> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 5 {
            bail!("A cron line has five fields: minute hour day month weekday.");
        }
        let ranges = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
        let mut fields: [Vec<bool>; 5] = Default::default();
        for (ix, (part, (low, high))) in parts.iter().zip(ranges).enumerate() {
            fields[ix] =
                field(part, low, high).with_context(|| format!("field {} (`{part}`)", ix + 1))?;
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

    fn matches(&self, at: &Zoned) -> bool {
        let [minute, hour, day, month, weekday] = &self.fields;
        let day_ok = day[at.day() as usize];
        let weekday_ok = weekday[at.weekday().to_sunday_zero_offset() as usize];
        minute[at.minute() as usize]
            && hour[at.hour() as usize]
            && month[at.month() as usize]
            && if self.either_day {
                day_ok || weekday_ok
            } else {
                day_ok && weekday_ok
            }
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
                Some((from, to)) => (from.parse()?, to.parse()?),
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
        assert!(cron(&Input::new("0 9 * * 1")).unwrap().lines().count() == 6);
    }
}
