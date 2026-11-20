use jiff::Timestamp;
use jiff::tz::TimeZone;
use keryx_agent::schedule::{Cron, CronError};

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn zone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap()
}

/// The next `count` fires after `after`, each read in UTC.
fn fires(expression: &str, after: &str, zone_name: &str, count: usize) -> Vec<String> {
    let cron = Cron::parse(expression).unwrap();
    let zone = zone(zone_name);
    let mut time = at(after);
    let mut fire_list = Vec::new();
    for _ in 0..count {
        time = cron.next_after(time, &zone).unwrap();
        fire_list.push(time.to_string());
    }
    fire_list
}

#[test]
fn every_minute_fires_on_the_next_minute() {
    assert_eq!(
        fires("* * * * *", "2026-11-16T09:12:30Z", "UTC", 3),
        [
            "2026-11-16T09:13:00Z",
            "2026-11-16T09:14:00Z",
            "2026-11-16T09:15:00Z"
        ]
    );
}

#[test]
fn a_fire_is_strictly_after_the_time_given() {
    assert_eq!(
        fires("0 9 * * *", "2026-11-16T09:00:00Z", "UTC", 1),
        ["2026-11-17T09:00:00Z"]
    );
    assert_eq!(
        fires("0 9 * * *", "2026-11-16T08:59:59Z", "UTC", 1),
        ["2026-11-16T09:00:00Z"]
    );
}

#[test]
fn ranges_lists_and_steps() {
    assert_eq!(
        fires("*/15 9-10 * * *", "2026-11-16T09:50:00Z", "UTC", 4),
        [
            "2026-11-16T10:00:00Z",
            "2026-11-16T10:15:00Z",
            "2026-11-16T10:30:00Z",
            "2026-11-16T10:45:00Z"
        ]
    );
    assert_eq!(
        fires("5,10-20/5 0 * * *", "2026-11-16T00:00:00Z", "UTC", 4),
        [
            "2026-11-16T00:05:00Z",
            "2026-11-16T00:10:00Z",
            "2026-11-16T00:15:00Z",
            "2026-11-16T00:20:00Z"
        ]
    );
}

#[test]
fn names_in_any_case_and_seven_as_sunday() {
    // 2026-11-16 is a Monday.
    assert_eq!(
        fires("0 9 * * mon-Fri", "2026-11-20T10:00:00Z", "UTC", 1),
        ["2026-11-23T09:00:00Z"]
    );
    assert_eq!(
        Cron::parse("0 9 * * 7").unwrap(),
        Cron::parse("0 9 * * SUN").unwrap()
    );
    assert_eq!(
        Cron::parse("0 9 * * 5-7").unwrap(),
        Cron::parse("0 9 * * 0,5,6").unwrap()
    );
    assert_eq!(
        fires("0 0 1 jan,JUL *", "2026-11-16T00:00:00Z", "UTC", 2),
        ["2027-01-01T00:00:00Z", "2027-07-01T00:00:00Z"]
    );
}

#[test]
fn day_and_weekday_both_restricted_fire_on_either() {
    // The 1st of the month, and every Monday.
    assert_eq!(
        fires("0 9 1 * 1", "2026-11-24T00:00:00Z", "UTC", 3),
        [
            "2026-11-30T09:00:00Z",
            "2026-12-01T09:00:00Z",
            "2026-12-07T09:00:00Z"
        ]
    );
    // With one of them starred, both must hold: Mondays in every other day of the month.
    assert_eq!(
        fires("0 9 */2 * 1", "2026-11-16T10:00:00Z", "UTC", 1),
        ["2026-11-23T09:00:00Z"]
    );
}

#[test]
fn the_zone_decides_the_hour() {
    assert_eq!(
        fires("30 8 * * *", "2026-11-16T00:00:00Z", "Asia/Tehran", 1),
        ["2026-11-16T05:00:00Z"]
    );
}

#[test]
fn a_time_skipped_by_a_jump_forward_fires_once_at_the_jump() {
    // Europe/Berlin jumps from 02:00 to 03:00 on 2027-03-28, at 01:00 UTC.
    assert_eq!(
        fires("30 2 * * *", "2027-03-27T12:00:00Z", "Europe/Berlin", 2),
        ["2027-03-28T01:00:00Z", "2027-03-29T00:30:00Z"]
    );
    assert_eq!(
        fires("*/20 2 * * *", "2027-03-27T12:00:00Z", "Europe/Berlin", 2),
        ["2027-03-28T01:00:00Z", "2027-03-29T00:00:00Z"]
    );
}

#[test]
fn a_time_repeated_by_a_fall_back_fires_once_the_first_time() {
    // Europe/Berlin goes from 03:00 back to 02:00 on 2026-10-25, at 01:00 UTC.
    assert_eq!(
        fires("30 2 * * *", "2026-10-24T12:00:00Z", "Europe/Berlin", 2),
        ["2026-10-25T00:30:00Z", "2026-10-26T01:30:00Z"]
    );
    // Asked from inside the repeated hour, after its first pass, it waits for the next day.
    assert_eq!(
        fires("45 2 * * *", "2026-10-25T01:10:00Z", "Europe/Berlin", 1),
        ["2026-10-26T01:45:00Z"]
    );
}

#[test]
fn a_date_that_never_comes_never_fires() {
    let cron = Cron::parse("0 0 30 2 *").unwrap();
    assert_eq!(
        cron.next_after(at("2026-11-16T00:00:00Z"), &zone("UTC")),
        None
    );
}

#[test]
fn a_leap_day_fires() {
    assert_eq!(
        fires("0 0 29 2 *", "2026-11-16T00:00:00Z", "UTC", 1),
        ["2028-02-29T00:00:00Z"]
    );
}

#[test]
fn bad_expressions_are_refused_naming_their_field() {
    assert_eq!(Cron::parse("* * * *"), Err(CronError::FieldCount(4)));
    assert_eq!(Cron::parse("* * * * * *"), Err(CronError::FieldCount(6)));
    let refused = [
        ("60 * * * *", "minute"),
        ("* 24 * * *", "hour"),
        ("* * 0 * *", "day"),
        ("* * 32 * *", "day"),
        ("* * * 13 *", "month"),
        ("* * * JANUARY *", "month"),
        ("* * * * 8", "weekday"),
        ("* * * * MON-", "weekday"),
        ("*/0 * * * *", "minute"),
        ("*/x * * * *", "minute"),
        ("5/10 * * * *", "minute"),
        ("10-5 * * * *", "minute"),
        ("1,,2 * * * *", "minute"),
        ("@daily * * * *", "minute"),
    ];
    for (expression, field) in refused {
        match Cron::parse(expression) {
            Err(CronError::Field { field: named, text }) => {
                assert_eq!(named, field, "{expression}");
                let error = CronError::Field { field: named, text };
                assert!(error.to_string().contains(field));
            }
            other => panic!("{expression}: {other:?}"),
        }
    }
}
