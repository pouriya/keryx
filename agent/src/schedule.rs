//! Schedules: `at`, `every` and `cron`, with keryx-agent's own cron parser, and their fires.

use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, TimeZone};

/// How far ahead `Cron::next_after` looks: nine years, past any leap-day and weekday cycle that can
/// come around, so an expression that can never match, such as `0 0 30 2 *`, ends.
const SEARCH_DAYS: i64 = 9 * 366;

/// A cron expression's five fields, each a bit per allowed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minute: u64,
    hour: u64,
    day: u64,
    month: u64,
    /// Sunday is bit 0. A 7 written as Sunday is folded onto it.
    weekday: u64,
    /// Whether the day or weekday field starts with `*`. Vixie cron's rule: when either does, both
    /// must match; when neither does, either matching is enough.
    day_star: bool,
    weekday_star: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CronError {
    #[error("cron has five fields, minute hour day month weekday, and this has {0}")]
    FieldCount(usize),
    #[error("cron's {field} field cannot read {text:?}")]
    Field { field: &'static str, text: String },
}

const MONTH_NAMES: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const WEEKDAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

impl Cron {
    /// Reads `minute hour day month weekday`, each a `*`, a number, a range `a-b`, either of those
    /// with a step `/n`, or a list of them joined by commas. Months and weekdays also take their
    /// three-letter English names in any case, and weekday 7 is Sunday as 0 is. No `@` aliases.
    pub fn parse(text: &str) -> Result<Self, CronError> {
        let field_list: Vec<&str> = text.split_whitespace().collect();
        if field_list.len() != 5 {
            return Err(CronError::FieldCount(field_list.len()));
        }
        // name, lowest, highest, the names that stand for numbers from `lowest`
        let shape: [(&'static str, u64, u64, &[&str]); 5] = [
            ("minute", 0, 59, &[]),
            ("hour", 0, 23, &[]),
            ("day", 1, 31, &[]),
            ("month", 1, 12, &MONTH_NAMES),
            ("weekday", 0, 7, &WEEKDAY_NAMES),
        ];
        let mut mask_list = [0u64; 5];
        for (index, (field, lowest, highest, name_list)) in shape.into_iter().enumerate() {
            let text = field_list[index];
            let refused = || CronError::Field {
                field,
                text: text.to_string(),
            };
            let mut mask = 0u64;
            for item in text.split(',') {
                let (range, step) = match item.split_once('/') {
                    Some((range, step)) => match step.parse::<u64>() {
                        Ok(step) if step > 0 => (range, step),
                        _ => return Err(refused()),
                    },
                    None => (item, 1),
                };
                let (start, end) = if range == "*" {
                    (lowest, highest)
                } else {
                    let (start, end) = match range.split_once('-') {
                        Some((start, end)) => (start, Some(end)),
                        None => (range, None),
                    };
                    let mut bound_list = [0u64; 2];
                    for (slot, bound) in [Some(start), end].into_iter().enumerate() {
                        let Some(bound) = bound else {
                            bound_list[1] = bound_list[0];
                            continue;
                        };
                        let mut value = None;
                        for (offset, name) in name_list.iter().enumerate() {
                            if bound.eq_ignore_ascii_case(name) {
                                value = Some(lowest + offset as u64);
                            }
                        }
                        if value.is_none()
                            && let Ok(number) = bound.parse::<u64>()
                        {
                            value = Some(number);
                        }
                        match value {
                            Some(value) if (lowest..=highest).contains(&value) => {
                                bound_list[slot] = value
                            }
                            _ => return Err(refused()),
                        }
                    }
                    // A step needs a range to walk: `5/10` is refused, as Vixie cron does.
                    if end.is_none() && step != 1 {
                        return Err(refused());
                    }
                    (bound_list[0], bound_list[1])
                };
                if start > end {
                    return Err(refused());
                }
                let mut value = start;
                while value <= end {
                    mask |= 1 << value;
                    value += step;
                }
            }
            mask_list[index] = mask;
        }
        let mut weekday = mask_list[4];
        if weekday & (1 << 7) != 0 {
            weekday = (weekday & !(1 << 7)) | 1;
        }
        Ok(Self {
            minute: mask_list[0],
            hour: mask_list[1],
            day: mask_list[2],
            month: mask_list[3],
            weekday,
            day_star: field_list[2].starts_with('*'),
            weekday_star: field_list[4].starts_with('*'),
        })
    }

    /// The first fire strictly after `after`, in `zone`, or `None` if nothing matches within nine
    /// years. A time a clock change skips fires at the jump itself, and a time a fall-back repeats
    /// fires once, the first time.
    pub fn next_after(&self, after: Timestamp, zone: &TimeZone) -> Option<Timestamp> {
        let local = zone.to_datetime(after);
        let mut date: Date = local.date();
        // The first candidate minute after `after`'s own.
        let mut from_hour = local.hour() as u64;
        let mut from_minute = local.minute() as u64 + 1;
        for _ in 0..SEARCH_DAYS {
            let month_ok = self.month & (1 << date.month()) != 0;
            let day_ok = self.day & (1 << date.day()) != 0;
            let weekday_ok = self.weekday & (1 << date.weekday().to_sunday_zero_offset()) != 0;
            let date_ok = if self.day_star || self.weekday_star {
                day_ok && weekday_ok
            } else {
                day_ok || weekday_ok
            };
            if month_ok && date_ok {
                for hour in from_hour..24 {
                    if self.hour & (1 << hour) == 0 {
                        continue;
                    }
                    let first_minute = if hour == from_hour { from_minute } else { 0 };
                    for minute in first_minute..60 {
                        if self.minute & (1 << minute) == 0 {
                            continue;
                        }
                        let civil = DateTime::from_parts(
                            date,
                            jiff::civil::time(hour as i8, minute as i8, 0, 0),
                        );
                        let instant = match zone.to_ambiguous_timestamp(civil).offset() {
                            AmbiguousOffset::Unambiguous { offset } => {
                                offset.to_timestamp(civil).ok()?
                            }
                            // Skipped by a jump forward: fires at the jump, the first transition
                            // after this time read with the offset after it.
                            AmbiguousOffset::Gap { after: later, .. } => {
                                let before_jump = later.to_timestamp(civil).ok()?;
                                zone.following(before_jump).next()?.timestamp()
                            }
                            // Repeated by a fall-back: the first time only.
                            AmbiguousOffset::Fold { before, .. } => {
                                before.to_timestamp(civil).ok()?
                            }
                        };
                        if instant > after {
                            return Some(instant);
                        }
                    }
                }
            }
            date = date.tomorrow().ok()?;
            from_hour = 0;
            from_minute = 0;
        }
        None
    }
}
