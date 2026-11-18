//! Turning command-line options into Withings request parameters.
//!
//! The v1 `measure` service takes Unix timestamps (`startdate`/`enddate`);
//! the v2 services (activity, sleep, workouts) take dates
//! (`startdateymd`/`enddateymd`). Both accept `lastupdate`.

use anyhow::{Result, bail};
use chrono::{NaiveDate, NaiveTime, TimeDelta, TimeZone};

pub type Params = Vec<(String, String)>;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Range {
    pub since: Option<NaiveDate>,
    /// Inclusive: the whole day is covered.
    pub until: Option<NaiveDate>,
    /// Only items created or edited after this Unix time.
    pub lastupdate: Option<i64>,
}

/// `startdate` at the start of `since` and `enddate` at the end of `until`,
/// in the given time zone.
pub fn timestamp_params<Tz: TimeZone>(range: &Range, tz: &Tz) -> Params {
    let start_of = |day: NaiveDate| tz.from_local_datetime(&day.and_time(NaiveTime::MIN)).earliest().map(|t| t.timestamp());
    let mut params = Params::new();
    if let Some(start) = range.since.and_then(start_of) {
        params.push(("startdate".into(), start.to_string()));
    }
    if let Some(end) = range.until.and_then(|d| d.succ_opt()).and_then(start_of) {
        params.push(("enddate".into(), (end - 1).to_string()));
    }
    push_lastupdate(&mut params, range);
    params
}

fn push_lastupdate(params: &mut Params, range: &Range) {
    if let Some(lastupdate) = range.lastupdate {
        params.push(("lastupdate".into(), lastupdate.to_string()));
    }
}

/// Days covered when only one end (or neither) of the range is given.
pub const DEFAULT_SPAN_DAYS: i64 = 7;

/// The v2 services need both `startdateymd` and `enddateymd`, so a missing
/// end becomes today and a missing start covers DEFAULT_SPAN_DAYS. With only
/// `lastupdate`, no dates are sent.
pub fn ymd_params(range: &Range, today: NaiveDate) -> Params {
    let mut params = Params::new();
    let only_lastupdate = range.lastupdate.is_some() && range.since.is_none() && range.until.is_none();
    if !only_lastupdate {
        let (since, until) = day_span(range, today);
        params.push(("startdateymd".into(), since.to_string()));
        params.push(("enddateymd".into(), until.to_string()));
    }
    push_lastupdate(&mut params, range);
    params
}

/// First and last day of the range: a missing end is today and a missing
/// start covers DEFAULT_SPAN_DAYS.
fn day_span(range: &Range, today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let until = range.until.unwrap_or(today);
    let since = range.since.unwrap_or(until - TimeDelta::days(DEFAULT_SPAN_DAYS - 1));
    (since, until)
}

/// Inclusive `(startdate, enddate)` Unix times, one per day of the range
/// (defaults as in `ymd_params`). Day D starts at D 00:00 + `day_start`
/// local time and ends one second before the next day starts.
pub fn day_windows<Tz: TimeZone>(range: &Range, today: NaiveDate, tz: &Tz, day_start: TimeDelta) -> Vec<(i64, i64)> {
    let start_of = |day: NaiveDate| tz.from_local_datetime(&(day.and_time(NaiveTime::MIN) + day_start)).earliest().map(|t| t.timestamp());
    let (since, until) = day_span(range, today);
    since
        .iter_days()
        .take_while(|day| *day <= until)
        .filter_map(|day| Some((start_of(day)?, start_of(day.succ_opt()?)? - 1)))
        .collect()
}

/// `key=value` arguments of `withings raw`.
pub fn parse_pairs(args: &[String]) -> Result<Params> {
    args.iter()
        .map(|arg| match arg.split_once('=') {
            Some((key, value)) if !key.is_empty() => Ok((key.to_string(), value.to_string())),
            _ => bail!("'{arg}' is not a parameter; write each one as key=value"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn pairs(list: &[(&str, &str)]) -> Params {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn timestamps_cover_whole_local_days() {
        let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
        let range = Range { since: Some(day("2026-11-17")), until: Some(day("2026-11-17")), lastupdate: None };
        // 2026-11-17 00:00 and 2026-11-18 00:00 - 1s, +09:00
        assert_eq!(
            timestamp_params(&range, &tokyo),
            pairs(&[("startdate", "1794841200"), ("enddate", "1794927599")])
        );
    }

    #[test]
    fn ymd_fills_the_missing_end_of_the_range() {
        // The v2 services answer "Missing params" (or 501) unless both dates are sent.
        let today = day("2026-11-17");
        let since_only = Range { since: Some(day("2026-11-01")), ..Range::default() };
        assert_eq!(ymd_params(&since_only, today), pairs(&[("startdateymd", "2026-11-01"), ("enddateymd", "2026-11-17")]));
        let until_only = Range { until: Some(day("2026-11-10")), ..Range::default() };
        assert_eq!(ymd_params(&until_only, today), pairs(&[("startdateymd", "2026-11-04"), ("enddateymd", "2026-11-10")]));
        assert_eq!(ymd_params(&Range::default(), today), pairs(&[("startdateymd", "2026-11-11"), ("enddateymd", "2026-11-17")]));
    }

    #[test]
    fn ymd_with_only_lastupdate_sends_just_that() {
        let range = Range { lastupdate: Some(1_794_870_000), ..Range::default() };
        assert_eq!(ymd_params(&range, day("2026-11-17")), pairs(&[("lastupdate", "1794870000")]));
        assert_eq!(timestamp_params(&Range::default(), &FixedOffset::east_opt(0).unwrap()), pairs(&[]));
    }

    #[test]
    fn parses_raw_pairs() {
        let args = ["meastypes=1,6".to_string(), "category=1".to_string()];
        assert_eq!(parse_pairs(&args).unwrap(), pairs(&[("meastypes", "1,6"), ("category", "1")]));
        let err = parse_pairs(&["oops".to_string()]).unwrap_err().to_string();
        assert!(err.contains("oops") && err.contains("key=value"), "{err}");
    }

    #[test]
    fn day_windows_split_the_range_into_local_days() {
        let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
        let range = Range { since: Some(day("2026-11-17")), until: Some(day("2026-11-18")), lastupdate: None };
        // 2026-11-17 00:00 +09:00 is 1794841200; each window ends one second before the next.
        assert_eq!(
            day_windows(&range, day("2026-11-30"), &tokyo, TimeDelta::zero()),
            [(1794841200, 1794927599), (1794927600, 1795013999)]
        );
    }

    #[test]
    fn day_windows_can_start_before_midnight() {
        // For sleep a day is the night that ends on it: noon before to noon.
        let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
        let range = Range { since: Some(day("2026-11-17")), until: Some(day("2026-11-17")), lastupdate: None };
        assert_eq!(day_windows(&range, day("2026-11-30"), &tokyo, -TimeDelta::hours(12)), [(1794798000, 1794884399)]);
    }

    #[test]
    fn day_windows_default_to_the_last_seven_days() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let windows = day_windows(&Range::default(), day("2026-11-17"), &utc, TimeDelta::zero());
        assert_eq!(windows.len(), DEFAULT_SPAN_DAYS as usize);
        assert_eq!(windows.last().unwrap().1 + 1, utc.from_utc_datetime(&day("2026-11-18").and_time(NaiveTime::MIN)).timestamp());
    }
}
