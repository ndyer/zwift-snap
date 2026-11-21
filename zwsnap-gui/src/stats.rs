//! Formatting of the riding statistics shown on an account card.
//!
//! Pure functions with no I/O, kept apart from the widgets so the exact strings
//! (and the unit conversion) can be pinned by unit tests. Distances and elevations
//! follow the rider's own `use_metric` setting, so two accounts can show different
//! units side by side, exactly as the app does.

/// The rider's total distance, labelled and grouped: `Distance 1,000 km` (metric)
/// or `Distance 621 mi` (imperial), rounded to the nearest whole unit.
pub fn distance(meters: i64, use_metric: bool) -> String {
    if use_metric {
        format!(
            "Distance {} km",
            group_thousands((meters as f64 / 1000.0).round() as i64)
        )
    } else {
        format!(
            "Distance {} mi",
            group_thousands((meters as f64 / METERS_PER_MILE).round() as i64)
        )
    }
}

/// The rider's total elevation gain, labelled and grouped: `Elevation 10,000 m`
/// (metric) or `Elevation 8,018 ft` (imperial), both whole.
pub fn elevation(meters: i64, use_metric: bool) -> String {
    if use_metric {
        format!("Elevation {} m", group_thousands(meters))
    } else {
        format!(
            "Elevation {} ft",
            group_thousands((meters as f64 / METERS_PER_FOOT).round() as i64)
        )
    }
}

/// Total time ridden, labelled as whole days and hours: `Time 2d 15h`, or
/// `Time 13h` under a day. Minutes and seconds are dropped; the profile only
/// records whole minutes anyway.
pub fn duration(minutes: i64) -> String {
    let minutes = minutes.max(0);
    let hours = minutes / 60;
    if hours >= 24 {
        format!("Time {}d {}h", hours / 24, hours % 24)
    } else {
        format!("Time {hours}h")
    }
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

/// The level-progress fraction for a card's bar: the profile's `achievement_level`
/// remainder (`achievement_level % 100`) as a 0.0-0.99 value. Clamped so a bad
/// value cannot draw outside the bar.
///
/// ASSUMPTION: the remainder is a percentage toward the next level. It is what
/// zwift-offline's `achievement_level // 100` split leaves over and Zwift's HUD
/// draws a level bar, but it is unverified against a live profile.
pub fn level_fraction(progress: i64) -> f64 {
    progress.clamp(0, 99) as f64 / 100.0
}

/// Group a number with thousands separators: `10000` -> `10,000`. The sign is kept
/// so the helper is total (the card's stats are never negative, but a bad value
/// must not produce separator soup).
fn group_thousands(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if negative {
        out.push('-');
    }
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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
    fn distance_is_labelled_grouped_and_follows_the_riders_units() {
        assert_eq!(distance(233_400, true), "Distance 233 km");
        assert_eq!(distance(233_400, false), "Distance 145 mi");
        assert_eq!(distance(0, true), "Distance 0 km");
        assert_eq!(distance(1_234_567, true), "Distance 1,235 km");
    }

    #[test]
    fn elevation_is_labelled_grouped_and_follows_the_riders_units() {
        assert_eq!(elevation(2444, true), "Elevation 2,444 m");
        assert_eq!(elevation(2444, false), "Elevation 8,018 ft");
        assert_eq!(elevation(10_000, true), "Elevation 10,000 m");
    }

    #[test]
    fn duration_is_whole_days_and_hours() {
        assert_eq!(duration(834), "Time 13h");
        assert_eq!(duration(0), "Time 0h");
        assert_eq!(duration(59), "Time 0h");
        assert_eq!(duration(60), "Time 1h");
        assert_eq!(duration(6000), "Time 4d 4h");
        // A negative total (which the API should never send) clamps to zero.
        assert_eq!(duration(-5), "Time 0h");
    }

    #[test]
    fn group_thousands_separates_every_three_digits() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(10_000), "10,000");
        assert_eq!(group_thousands(1_000_000), "1,000,000");
        assert_eq!(group_thousands(-1234), "-1,234");
    }

    #[test]
    fn level_fraction_is_a_clamped_percentage() {
        assert_eq!(level_fraction(0), 0.0);
        assert_eq!(level_fraction(34), 0.34);
        assert_eq!(level_fraction(99), 0.99);
        assert_eq!(level_fraction(150), 0.99);
        assert_eq!(level_fraction(-5), 0.0);
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
