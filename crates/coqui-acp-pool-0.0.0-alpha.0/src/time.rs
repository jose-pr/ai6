//! The two ways a time is said in a refusal. The clock is the caller's: every time is an argument, in Unix seconds.

use alloc::format;
use alloc::string::String;

/// `in 2 h 10 min`, for a wait of `seconds` (rounded up to the minute; a wait that is over is `in 0 min`).
#[must_use]
pub fn after(seconds: i64) -> String {
    let minutes = (seconds.max(0) + 59) / 60;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("in {minutes} min"),
        (hours, 0) => format!("in {hours} h"),
        (hours, minutes) => format!("in {hours} h {minutes} min"),
    }
}

/// `2026-11-20 15:04 UTC` for a time in Unix seconds.
#[must_use]
pub fn utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // The proleptic Gregorian calendar from a day count (Howard Hinnant's `civil_from_days`).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        of_day / 3600,
        of_day % 3600 / 60
    )
}
