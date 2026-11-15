//! Formatting of the riding statistics shown on an account card.
//!
//! Pure functions with no I/O, kept apart from the widgets so the exact strings
//! (and the unit conversion) can be pinned by unit tests. Distances and elevations
//! follow the rider's own `use_metric` setting, so two accounts can show different
//! units side by side, exactly as the app does.

/// The rider's total distance, in kilometres (one decimal) or miles (one decimal).
pub fn distance(meters: i64, use_metric: bool) -> String {
    if use_metric {
        format!("{:.1} km", meters as f64 / 1000.0)
    } else {
        format!("{:.1} mi", meters as f64 / METERS_PER_MILE)
    }
}

/// The rider's total elevation gain, in metres or feet, both as whole numbers.
pub fn elevation(meters: i64, use_metric: bool) -> String {
    if use_metric {
        format!("{meters} m")
    } else {
        format!("{} ft", (meters as f64 / METERS_PER_FOOT).round() as i64)
    }
}

/// Total time ridden, as a zero-padded `HH:MM:SS`.
///
/// The profile only records whole minutes, so the seconds field is always `00`;
/// the `HH:MM:SS` shape is kept anyway so the card matches the app's chip.
pub fn duration(minutes: i64) -> String {
    let minutes = minutes.max(0);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    format!("{hours:03}:{minutes:02}:00")
}

/// The rider's weight, in kilograms (one decimal) or pounds (whole).
pub fn weight(grams: i64, use_metric: bool) -> String {
    if use_metric {
        format!("{:.1} kg", grams as f64 / 1000.0)
    } else {
        format!("{} lb", (grams as f64 / GRAMS_PER_POUND).round() as i64)
    }
}

/// Up to two uppercase initials from a first and last name, for the avatar
/// fallback. Falls back to the single name, and to `?` when there is neither.
pub fn initials(first: Option<&str>, last: Option<&str>) -> String {
    let pick = |name: Option<&str>| {
        name.and_then(|name| {
            name.split_whitespace()
                .next()
                .and_then(|word| word.chars().next())
        })
    };
    let mut initials = String::new();
    if let Some(letter) = pick(first).or_else(|| pick(last)) {
        initials.extend(letter.to_uppercase());
    }
    if let Some(letter) = pick(last).filter(|_| pick(first).is_some()) {
        initials.extend(letter.to_uppercase());
    }
    if initials.is_empty() {
        "?".to_owned()
    } else {
        initials
    }
}

/// A transfer rate in bytes per second, as decimal megabytes (10⁶, how network
/// speeds are quoted) to one decimal place. Used by both the debug log and the
/// download progress bar, so the two always read the same.
pub fn rate(bytes_per_second: f64) -> String {
    format!("{:.1} MB/s", bytes_per_second / 1_000_000.0)
}

/// The caption for a download's progress bar: the completion percentage, and the
/// transfer rate when the downloader has one to show (`None` on the per-file
/// ticks, which carry no fresh measurement).
pub fn transfer_progress(percent: u8, rate: Option<f64>) -> String {
    match rate {
        Some(bytes_per_second) => {
            format!("{}% · {}", percent.min(99), self::rate(bytes_per_second))
        }
        None => format!("{}%", percent.min(99)),
    }
}

/// Metres in a statute mile.
const METERS_PER_MILE: f64 = 1609.344;

/// Metres in a foot.
const METERS_PER_FOOT: f64 = 0.3048;

/// Grams in a pound.
const GRAMS_PER_POUND: f64 = 453.592_37;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_follows_the_riders_units() {
        assert_eq!(distance(233_400, true), "233.4 km");
        assert_eq!(distance(233_400, false), "145.0 mi");
        assert_eq!(distance(0, true), "0.0 km");
    }

    #[test]
    fn elevation_follows_the_riders_units() {
        assert_eq!(elevation(2444, true), "2444 m");
        assert_eq!(elevation(2444, false), "8018 ft");
    }

    #[test]
    fn duration_is_zero_padded_hours_minutes_seconds() {
        assert_eq!(duration(834), "013:54:00");
        assert_eq!(duration(0), "000:00:00");
        assert_eq!(duration(59), "000:59:00");
        assert_eq!(duration(60), "001:00:00");
        assert_eq!(duration(6000), "100:00:00");
        // A negative total (which the API should never send) clamps to zero.
        assert_eq!(duration(-5), "000:00:00");
    }

    #[test]
    fn weight_follows_the_riders_units() {
        assert_eq!(weight(70_000, true), "70.0 kg");
        assert_eq!(weight(70_000, false), "154 lb");
    }

    #[test]
    fn initials_come_from_both_names() {
        assert_eq!(initials(Some("Sara"), Some("Campbell")), "SC");
        assert_eq!(initials(Some("sara"), Some("campbell")), "SC");
        assert_eq!(initials(Some("Sara"), None), "S");
        assert_eq!(initials(None, Some("Campbell")), "C");
        assert_eq!(initials(None, None), "?");
        assert_eq!(initials(Some(""), Some("")), "?");
        assert_eq!(initials(Some("  "), Some(" Campbell ")), "C");
    }

    #[test]
    fn rate_is_decimal_megabytes_per_second() {
        assert_eq!(rate(0.0), "0.0 MB/s");
        assert_eq!(rate(12_340_000.0), "12.3 MB/s");
        assert_eq!(rate(1_000_000.0), "1.0 MB/s");
        assert_eq!(rate(999_999.0), "1.0 MB/s");
    }

    #[test]
    fn transfer_progress_appends_the_rate_only_when_known() {
        assert_eq!(transfer_progress(42, None), "42%");
        assert_eq!(transfer_progress(42, Some(12_340_000.0)), "42% · 12.3 MB/s");
        // The percentage is capped below 100 until the task's own Done tick.
        assert_eq!(transfer_progress(150, None), "99%");
    }
}
