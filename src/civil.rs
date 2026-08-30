//! Calendar arithmetic. Replaces `chrono` / `time`.
//!
//! `std` gives `SystemTime` — seconds since the epoch — and nothing else. No
//! dates, no formatting, no zones. This is Howard Hinnant's
//! `days_from_civil` / `civil_from_days`, exact in both directions across
//! the full proleptic Gregorian range.
//!
//! **No time zone database, and none is needed:** EXIF `DateTimeOriginal` is
//! local wall-clock time with no zone, so it is stored as-is and displayed
//! as-is. The honest cost is that a photo taken abroad sorts by the camera's
//! clock, not yours.

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64; // March-based month
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// The inverse: days since the epoch back to `(year, month, day)`.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Seconds since the epoch for a local wall-clock date and time.
pub fn timestamp(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    days_from_civil(y, mo, d) * 86400 + h as i64 * 3600 + mi as i64 * 60 + s as i64
}

/// `YYYY-MM-DD` for a timestamp, for grouping in the timeline.
pub fn date_string(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Parses EXIF's `"YYYY:MM:DD HH:MM:SS"`.
///
/// **Colons in the date, not dashes** — twenty bytes including the NUL.
/// Cameras also write all-zero dates when the clock was never set, and those
/// are rejected rather than becoming 1970.
pub fn parse_exif_datetime(s: &str) -> Option<i64> {
    let s = s.trim_end_matches('\0').trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.trim().parse::<i64>().ok() };

    let y = num(0, 4)?;
    let mo = num(5, 7)? as u32;
    let d = num(8, 10)? as u32;
    let h = num(11, 13)? as u32;
    let mi = num(14, 16)? as u32;
    let sec = num(17, 19)? as u32;

    if y < 1826 || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    if h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(timestamp(y, mo, d, h, mi, sec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_day_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn known_dates() {
        assert_eq!(days_from_civil(2000, 1, 1), 10957);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2026, 8, 18), 20683);
        assert_eq!(civil_from_days(20683), (2026, 8, 18));
    }

    #[test]
    fn leap_days_are_handled() {
        // 2000 is a leap year, 1900 is not, 2024 is.
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        // 1900-02-28 then 1900-03-01, one day apart.
        assert_eq!(days_from_civil(1900, 3, 1) - days_from_civil(1900, 2, 28), 1);
    }

    /// Exact both ways across a wide range — the whole point of the algorithm.
    #[test]
    fn round_trips_over_eight_centuries() {
        for z in (-300_000i64..300_000).step_by(7) {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z, "failed at {z} -> {y}-{m}-{d}");
        }
    }

    #[test]
    fn formats_dates() {
        assert_eq!(date_string(0), "1970-01-01");
        assert_eq!(date_string(1_786_882_193), "2026-08-16");
        // Negative timestamps floor correctly rather than truncating to 1970.
        assert_eq!(date_string(-1), "1969-12-31");
    }

    #[test]
    fn parses_exif_datetimes() {
        let ts = parse_exif_datetime("2026:08:18 14:30:05").unwrap();
        assert_eq!(date_string(ts), "2026-08-18");
        assert_eq!(ts % 86400, 14 * 3600 + 30 * 60 + 5);
    }

    #[test]
    fn tolerates_nul_padding_and_whitespace() {
        assert!(parse_exif_datetime("2026:08:18 14:30:05\0").is_some());
        assert!(parse_exif_datetime("  2026:08:18 14:30:05  ").is_some());
    }

    #[test]
    fn rejects_unset_and_malformed_clocks() {
        assert!(parse_exif_datetime("0000:00:00 00:00:00").is_none());
        assert!(parse_exif_datetime("").is_none());
        assert!(parse_exif_datetime("not a date at all").is_none());
        assert!(parse_exif_datetime("2026:13:01 00:00:00").is_none());
        assert!(parse_exif_datetime("2026:08:18 25:00:00").is_none());
        assert!(parse_exif_datetime("2026:08").is_none());
    }
}
