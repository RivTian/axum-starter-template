//! Durations written as a whole number and a unit: `250ms`, `5s`, `2m`, `1h`.
//!
//! Configuration fields read them with [`crate::de::duration`] and write them with
//! [`serialize`].

use std::time::Duration;

use serde::Serializer;

/// What a duration must look like; configuration errors quote it.
pub const EXPECTED: &str = r#"a duration like "5s" or "250ms""#;

/// Parses `<digits><unit>` with the unit `h`, `m`, `s` or `ms`, and nothing else: no sign,
/// no fraction, no spaces.
#[must_use]
pub fn parse(text: &str) -> Option<Duration> {
    let digits = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(digits);
    if number.is_empty() {
        return None;
    }
    let value: u64 = number.parse().ok()?;
    let millis_per_unit = match unit {
        "h" => 3_600_000,
        "m" => 60_000,
        "s" => 1_000,
        "ms" => 1,
        _ => return None,
    };
    value
        .checked_mul(millis_per_unit)
        .map(Duration::from_millis)
}

/// Writes a duration in the largest unit that represents it exactly, whole milliseconds only.
#[must_use]
pub fn format(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis != 0 && millis.is_multiple_of(3_600_000) {
        format!("{}h", millis / 3_600_000)
    } else if millis != 0 && millis.is_multiple_of(60_000) {
        format!("{}m", millis / 60_000)
    } else if millis.is_multiple_of(1_000) {
        format!("{}s", millis / 1_000)
    } else {
        format!("{millis}ms")
    }
}

/// Serializes a duration as [`format()`] writes it.
///
/// # Errors
///
/// Whatever the serializer reports.
pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format(*duration))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{format, parse};

    #[test]
    fn parses_whole_numbers_with_a_unit() {
        assert_eq!(parse("250ms"), Some(Duration::from_millis(250)));
        assert_eq!(parse("5s"), Some(Duration::from_secs(5)));
        assert_eq!(parse("0s"), Some(Duration::ZERO));
        assert_eq!(parse("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse("1h"), Some(Duration::from_secs(3_600)));
    }

    #[test]
    fn rejects_everything_else() {
        for text in [
            "",
            "abc",
            "5",
            "s",
            "1.5s",
            "5 seconds",
            "-5s",
            " 5s",
            "5s ",
            "5S",
            "5sec",
            "99999999999999999999ms",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn formats_in_the_largest_exact_unit() {
        assert_eq!(format(Duration::from_millis(250)), "250ms");
        assert_eq!(format(Duration::from_millis(1_500)), "1500ms");
        assert_eq!(format(Duration::from_secs(5)), "5s");
        assert_eq!(format(Duration::ZERO), "0s");
        assert_eq!(format(Duration::from_secs(120)), "2m");
        assert_eq!(format(Duration::from_secs(7_200)), "2h");
    }
}
