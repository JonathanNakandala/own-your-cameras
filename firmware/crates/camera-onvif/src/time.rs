//! ISO 8601 UTC timestamps and durations as ONVIF writes them (`2026-11-07T12:00:00Z`, `PT60S`). Pure.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Broken-down UTC time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Utc {
    pub fn of(t: SystemTime) -> Self {
        let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
        let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
        let rem = secs.rem_euclid(86_400) as u32;
        Self {
            year,
            month,
            day,
            hour: rem / 3600,
            minute: rem / 60 % 60,
            second: rem % 60,
        }
    }
}

pub fn format(t: SystemTime) -> String {
    let u = Utc::of(t);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        u.year, u.month, u.day, u.hour, u.minute, u.second
    )
}

/// `YYYY-MM-DDTHH:MM:SS[.fff](Z|±hh:mm)`; a missing zone is UTC.
pub fn parse(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let num = |a: usize, b: usize| -> Option<u32> {
        let part = s.get(a..b)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if s.get(4..5)? != "-" || s.get(7..8)? != "-" || !matches!(s.get(10..11)?, "T" | "t") {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = s.get(19..)?;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        rest = &frac[digits..];
    }
    let offset: i64 = match rest {
        "" | "Z" | "z" => 0,
        zone => {
            let sign = match zone.get(0..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let (h, m) = zone[1..].split_once(':')?;
            sign * (i64::from(h.parse::<u32>().ok()?) * 3600
                + i64::from(m.parse::<u32>().ok()?) * 60)
        }
    };
    let secs = days_from_civil(i64::from(year), month, day) * 86_400
        + i64::from(hour * 3600 + minute * 60 + second)
        - offset;
    Some(UNIX_EPOCH + Duration::from_secs(u64::try_from(secs).ok()?))
}

/// `PnDTnHnMnS` with optional fractional seconds; years and months are refused (no fixed length).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let rest = s.strip_prefix('P')?;
    let (days, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut total = 0f64;
    let mut any = false;
    let mut take = |part: &str, units: &[(char, f64)]| -> Option<()> {
        let mut number = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                number.push(c);
            } else {
                let (_, scale) = units.iter().find(|(u, _)| *u == c)?;
                total += number.parse::<f64>().ok()? * scale;
                number.clear();
                any = true;
            }
        }
        number.is_empty().then_some(())
    };
    take(days, &[('D', 86_400.0), ('W', 604_800.0)])?;
    take(time, &[('H', 3600.0), ('M', 60.0), ('S', 1.0)])?;
    if !any {
        return None;
    }
    // Hostile values (overflow, NaN) are refused, never a panic.
    Duration::try_from_secs_f64(total).ok()
}

pub fn format_duration(d: Duration) -> String {
    format!("PT{}S", d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip_and_zones_are_applied() {
        let t = UNIX_EPOCH + Duration::from_secs(1_794_052_800); // 2026-11-07T12:00:00Z
        assert_eq!(format(t), "2026-11-07T12:00:00Z");
        assert_eq!(parse("2026-11-07T12:00:00Z"), Some(t));
        assert_eq!(parse("2026-11-07T12:00:00.123Z"), Some(t));
        assert_eq!(parse("2026-11-07T14:00:00+02:00"), Some(t));
        assert_eq!(parse("2026-11-07T12:00:00"), Some(t));
        assert_eq!(format(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            format(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
        for bad in [
            "",
            "2026-13-01T00:00:00Z",
            "2026-11-07 12:00:00Z",
            "2026-11-07T12:00:00X",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn durations_parse_as_onvif_clients_send_them() {
        assert_eq!(parse_duration("PT60S"), Some(Duration::from_secs(60)));
        assert_eq!(parse_duration("PT10M"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("P1DT1H"), Some(Duration::from_secs(90_000)));
        assert_eq!(parse_duration("PT0.5S"), Some(Duration::from_millis(500)));
        for bad in [
            "",
            "P",
            "PT",
            "60S",
            "PT5X",
            "P1Y",
            "PT5",
            "PT99999999999999999999999S",
            "P99999999999999999999D",
        ] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
        assert_eq!(format_duration(Duration::from_secs(60)), "PT60S");
    }
}
