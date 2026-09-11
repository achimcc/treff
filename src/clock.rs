//! What time it is, where the forum stands.
//!
//! A timestamp in the database is Unix seconds and says nothing about a place.
//! The moment it is shown with an hour on it, it needs one: `09:41` is a lie
//! for everybody the offset does not fit, and the reader has no way to tell.
//! A bare date hid that question; a clock asks it out loud.
//!
//! The zone is resolved once, at startup, and from the instance rather than
//! from the reader: a forum for a closed circle is read in the circle's own
//! time, and a per-reader zone would need a per-reader setting nobody asked
//! for. `TZ` or `/etc/localtime` answers it without any configuration at all;
//! `timezone` in the configuration file overrides that where the machine
//! stands somewhere else than the people do.

use jiff::tz::TimeZone;
use std::sync::OnceLock;

static ZONE: OnceLock<TimeZone> = OnceLock::new();

/// A timestamp as the day and the hour, in `zone`.
///
/// Takes the zone rather than reading the global one, so that the conversion
/// can be tested against a named place instead of against whatever the machine
/// running the tests believes.
pub fn stamp(unix_seconds: i64, zone: &TimeZone) -> String {
    // Both steps can fail — a number far outside any calendar, a format the
    // formatter refuses — and neither may panic: this runs while a page is
    // being rendered. An unreadable timestamp becomes a word, the way the
    // bare date did before it.
    jiff::Timestamp::from_second(unix_seconds)
        .and_then(|t| {
            let zoned = t.to_zoned(zone.clone());
            jiff::fmt::strtime::format("%Y-%m-%d %H:%M", &zoned)
        })
        .unwrap_or_else(|_| String::from("unknown"))
}

/// The zone every page is rendered in.
///
/// Before [`install`] has run — in a test that never starts the program — this
/// is the machine's own zone. That is the same answer [`resolve`] gives for a
/// configuration file without a `timezone`, so nothing behaves one way in a
/// test and another in service.
pub fn zone() -> &'static TimeZone {
    ZONE.get_or_init(TimeZone::system)
}

/// The zone a configured name stands for, or the machine's own when no name is
/// configured.
///
/// An unknown name is an error and not a fallback: the caller stops the
/// program with it. UTC instead of `Europe/Berlin` is not a degraded forum,
/// it is a forum in which every timestamp is quietly wrong.
pub fn resolve(name: Option<&str>) -> Result<TimeZone, jiff::Error> {
    match name {
        Some(name) => TimeZone::get(name),
        None => Ok(TimeZone::system()),
    }
}

/// Fixes the zone for the life of the process. Called once, at startup; a
/// second call changes nothing, because a page rendered in one zone and the
/// next page in another would be worse than either.
pub fn install(zone: TimeZone) {
    let _ = ZONE.set(zone);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-01-15 09:41:00 UTC and 2026-07-15 09:41:00 UTC.
    const WINTER: i64 = 1_768_470_060;
    const SUMMER: i64 = 1_784_108_460;

    #[test]
    fn a_stamp_carries_the_day_and_the_hour() {
        let utc = TimeZone::UTC;
        assert_eq!(stamp(WINTER, &utc), "2026-01-15 09:41");
    }

    #[test]
    fn summer_time_is_not_an_hour_the_reader_has_to_add() {
        // The same offset all year would be right for half of it. Berlin is
        // UTC+1 in January and UTC+2 in July, and a fixed offset in the
        // configuration file would have to be corrected by hand twice a year —
        // which is the kind of manual step this project treats as a fault.
        let berlin = TimeZone::get("Europe/Berlin").expect("Europe/Berlin");
        assert_eq!(stamp(WINTER, &berlin), "2026-01-15 10:41");
        assert_eq!(stamp(SUMMER, &berlin), "2026-07-15 11:41");
    }
}
