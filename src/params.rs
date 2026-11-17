//! Turning command-line options into Withings request parameters.
//!
//! The v1 `measure` service takes Unix timestamps (`startdate`/`enddate`);
//! the v2 services (activity, sleep, workouts) take dates
//! (`startdateymd`/`enddateymd`). Both accept `lastupdate`.

use anyhow::{Result, bail};
use chrono::{NaiveDate, NaiveTime, TimeZone};

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

pub fn ymd_params(range: &Range) -> Params {
    let mut params = Params::new();
    if let Some(since) = range.since {
        params.push(("startdateymd".into(), since.to_string()));
    }
    if let Some(until) = range.until {
        params.push(("enddateymd".into(), until.to_string()));
    }
    push_lastupdate(&mut params, range);
    params
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
    fn ymd_and_lastupdate() {
        let range = Range { since: Some(day("2026-11-01")), until: None, lastupdate: Some(1_794_870_000) };
        assert_eq!(ymd_params(&range), pairs(&[("startdateymd", "2026-11-01"), ("lastupdate", "1794870000")]));
        assert_eq!(timestamp_params(&Range::default(), &FixedOffset::east_opt(0).unwrap()), pairs(&[]));
    }

    #[test]
    fn parses_raw_pairs() {
        let args = ["meastypes=1,6".to_string(), "category=1".to_string()];
        assert_eq!(parse_pairs(&args).unwrap(), pairs(&[("meastypes", "1,6"), ("category", "1")]));
        let err = parse_pairs(&["oops".to_string()]).unwrap_err().to_string();
        assert!(err.contains("oops") && err.contains("key=value"), "{err}");
    }
}
