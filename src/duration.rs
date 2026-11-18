//! `std::time::Duration` in configuration files, written as `500ms`, `30s`,
//! `1h30m` or `90days`: one or more parts, each an integer and a unit, with
//! optional whitespace between and around them. A day is 24 hours and a week
//! 7 days; months, years, fractions and signs are refused, because their
//! length is either unclear or never needed here. Printing writes the largest
//! units first (`1h30m`, `90d`, `0s`), and the text parses back to the same
//! duration.
//!
//! Use it on a field as `#[serde(with = "crate::duration")]`.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serializer};

const NANOS_PER_SEC: u128 = 1_000_000_000;

/// Unit labels and their length in nanoseconds. Longer labels come first, so
/// that `ms` is never read as `m`.
const UNITS: &[(&str, u128)] = &[
    ("nanoseconds", 1),
    ("nanosecond", 1),
    ("ns", 1),
    ("microseconds", 1_000),
    ("microsecond", 1_000),
    ("us", 1_000),
    ("µs", 1_000),
    ("milliseconds", 1_000_000),
    ("millisecond", 1_000_000),
    ("ms", 1_000_000),
    ("seconds", NANOS_PER_SEC),
    ("second", NANOS_PER_SEC),
    ("secs", NANOS_PER_SEC),
    ("sec", NANOS_PER_SEC),
    ("s", NANOS_PER_SEC),
    ("minutes", 60 * NANOS_PER_SEC),
    ("minute", 60 * NANOS_PER_SEC),
    ("mins", 60 * NANOS_PER_SEC),
    ("min", 60 * NANOS_PER_SEC),
    ("m", 60 * NANOS_PER_SEC),
    ("hours", 3_600 * NANOS_PER_SEC),
    ("hour", 3_600 * NANOS_PER_SEC),
    ("hrs", 3_600 * NANOS_PER_SEC),
    ("hr", 3_600 * NANOS_PER_SEC),
    ("h", 3_600 * NANOS_PER_SEC),
    ("days", 86_400 * NANOS_PER_SEC),
    ("day", 86_400 * NANOS_PER_SEC),
    ("d", 86_400 * NANOS_PER_SEC),
    ("weeks", 604_800 * NANOS_PER_SEC),
    ("week", 604_800 * NANOS_PER_SEC),
    ("w", 604_800 * NANOS_PER_SEC),
];

/// The units [`format()`] writes, largest first.
const PRINTED: &[(&str, u128)] = &[
    ("d", 86_400 * NANOS_PER_SEC),
    ("h", 3_600 * NANOS_PER_SEC),
    ("m", 60 * NANOS_PER_SEC),
    ("s", NANOS_PER_SEC),
    ("ms", 1_000_000),
    ("us", 1_000),
    ("ns", 1),
];

/// A duration that could not be read, with the text that was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    input: String,
    reason: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid duration {:?}: {}; write it as e.g. 500ms, 30s, 5m, 1h30m or 7days",
            self.input, self.reason
        )
    }
}

impl std::error::Error for ParseError {}

/// Read a duration such as `500ms`, `30s`, `1h 30m` or `90days`.
///
/// # Errors
/// [`ParseError`] for empty text, a part without a number or a unit, an
/// unknown unit, or a total longer than [`Duration::MAX`].
pub fn parse(input: &str) -> Result<Duration, ParseError> {
    let fail = |reason| ParseError {
        input: input.to_owned(),
        reason,
    };
    let mut rest = input.trim_start();
    if rest.trim_end().is_empty() {
        return Err(fail("it is empty"));
    }
    let mut total: u128 = 0;
    while !rest.is_empty() {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return Err(fail("each part must start with a whole number"));
        }
        let (number, after) = rest.split_at(digits);
        let number: u128 = number
            .parse()
            .map_err(|_| fail("the number is too large"))?;
        let after = after.trim_start();
        let label_len = after.len() - after.trim_start_matches(char::is_alphabetic).len();
        let (label, after) = after.split_at(label_len);
        if label.is_empty() {
            return Err(fail("each number needs a unit"));
        }
        let Some(&(_, nanos)) = UNITS.iter().find(|(name, _)| *name == label) else {
            return Err(fail("the unit is not one of ns, us, ms, s, m, h, d or w"));
        };
        total = number
            .checked_mul(nanos)
            .and_then(|part| total.checked_add(part))
            .ok_or_else(|| fail("it is too long"))?;
        rest = after.trim_start();
    }
    let secs = u64::try_from(total / NANOS_PER_SEC).map_err(|_| fail("it is too long"))?;
    let nanos = u32::try_from(total % NANOS_PER_SEC).map_err(|_| fail("it is too long"))?;
    Ok(Duration::new(secs, nanos))
}

/// Write a duration with the largest units first, such as `1h30m`, `90d`,
/// `1s500ms` or `0s`; [`parse`] reads it back to the same value.
#[must_use]
pub fn format(duration: Duration) -> String {
    let mut left = duration.as_nanos();
    if left == 0 {
        return "0s".to_owned();
    }
    let mut out = String::new();
    for &(label, nanos) in PRINTED {
        let count = left / nanos;
        if count > 0 {
            out.push_str(&count.to_string());
            out.push_str(label);
            left %= nanos;
        }
    }
    out
}

/// Serialize as [`format()`] writes it.
///
/// # Errors
/// Whatever the serializer returns.
pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format(*duration))
}

/// Deserialize from text that [`parse`] reads.
///
/// # Errors
/// A custom error naming the text when it is not a duration.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
    let text = String::deserialize(deserializer)?;
    parse(&text).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1;
    const S: u64 = 1_000;
    const M: u64 = 60 * S;
    const H: u64 = 60 * M;
    const D: u64 = 24 * H;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn reads_every_form_the_configs_use() {
        for (text, want) in [
            ("500ms", ms(500 * MS)),
            ("1500ms", ms(1_500 * MS)),
            ("0s", Duration::ZERO),
            ("1s", ms(S)),
            ("30s", ms(30 * S)),
            ("60s", ms(60 * S)),
            ("5m", ms(5 * M)),
            ("15m", ms(15 * M)),
            ("1h", ms(H)),
            ("24h", ms(24 * H)),
            ("7days", ms(7 * D)),
            ("30days", ms(30 * D)),
            ("90days", ms(90 * D)),
        ] {
            assert_eq!(parse(text), Ok(want), "{text}");
        }
    }

    #[test]
    fn reads_long_labels_spaces_and_several_parts() {
        for (text, want) in [
            ("1h 30m", ms(H + 30 * M)),
            ("1h30m", ms(H + 30 * M)),
            ("  2 minutes ", ms(2 * M)),
            ("1 day 2 hours", ms(D + 2 * H)),
            ("2w", ms(14 * D)),
            ("1 week", ms(7 * D)),
            ("3 secs", ms(3 * S)),
            ("10min", ms(10 * M)),
            ("2hrs", ms(2 * H)),
            ("250us", Duration::from_micros(250)),
            ("250µs", Duration::from_micros(250)),
            ("7ns", Duration::from_nanos(7)),
        ] {
            assert_eq!(parse(text), Ok(want), "{text}");
        }
    }

    #[test]
    fn refuses_what_has_no_clear_length() {
        for text in [
            "",
            "   ",
            "s",
            "10",
            "1.5s",
            "-1s",
            "+1s",
            "1month",
            "2y",
            "1 fortnight",
            "1h x",
            "1h,30m",
            "99999999999999999999999999999999999999999w",
        ] {
            let err = parse(text);
            assert!(err.is_err(), "{text} parsed as {err:?}");
        }
    }

    #[test]
    fn the_error_names_the_input() {
        let err = parse("1month")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("\"1month\""), "{err}");
    }

    #[test]
    fn prints_largest_units_first_and_reads_back() {
        for (duration, text) in [
            (Duration::ZERO, "0s"),
            (ms(500), "500ms"),
            (ms(1_500), "1s500ms"),
            (ms(30 * S), "30s"),
            (ms(H + 30 * M), "1h30m"),
            (ms(90 * D), "90d"),
            (Duration::new(3_661, 1_001), "1h1m1s1us1ns"),
        ] {
            assert_eq!(format(duration), text);
            assert_eq!(parse(text), Ok(duration), "{text}");
        }
    }

    #[test]
    fn reads_a_yaml_field_and_refuses_a_bad_one() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Settings {
            #[serde(with = "super")]
            timeout: Duration,
        }
        let printed = format(ms(2 * M + 5 * S));
        let read: Result<Settings, _> = serde_saphyr::from_str(&format!("timeout: {printed}\n"));
        assert_eq!(read.ok().map(|s| s.timeout), Some(ms(2 * M + 5 * S)));
        let bad: Result<Settings, _> = serde_saphyr::from_str("timeout: 3 months\n");
        let err = bad.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("3 months"), "{err}");
    }
}
