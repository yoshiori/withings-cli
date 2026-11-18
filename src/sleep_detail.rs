//! Sleep segments (`v2/sleep` / `get`): one item per stretch of a single
//! sleep state, from Withings devices and from Apple Health alike.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use chrono::TimeZone;
use serde_json::{Map, Value};

use crate::intraday::local_time;

/// Withings sleep `state` values.
const STATES: &[(i64, &str)] = &[(0, "awake"), (1, "light"), (2, "deep"), (3, "rem")];

fn state_name(state: i64) -> String {
    STATES.iter().find(|(s, _)| *s == state).map(|(_, name)| name.to_string()).unwrap_or_else(|| format!("state_{state}"))
}

/// Segments from several (possibly overlapping) requests: duplicates
/// dropped, oldest first, `startdate`/`enddate`/`state` replaced by local
/// `start`/`end` and a state name. Other fields are kept as sent.
pub fn segments<Tz: TimeZone>(items: &[Value], tz: &Tz) -> Result<Vec<Value>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        let Value::Object(fields) = item else { continue };
        let time = |key: &str| fields.get(key).and_then(Value::as_i64).with_context(|| format!("Withings sent a sleep segment without `{key}`: {item}"));
        let (start, end) = (time("startdate")?, time("enddate")?);
        let state = fields.get("state").and_then(Value::as_i64);
        let model_id = fields.get("model_id").and_then(Value::as_i64);
        if !seen.insert((start, end, state, model_id)) {
            continue;
        }
        let local = |unix: i64| local_time(unix, tz).with_context(|| format!("sleep segment time {unix} is out of range"));
        let mut segment = Map::new();
        segment.insert("start".into(), local(start)?);
        segment.insert("end".into(), local(end)?);
        segment.insert("state".into(), state.map(state_name).map_or(Value::Null, Value::from));
        segment.extend(fields.iter().filter(|(k, _)| !matches!(k.as_str(), "startdate" | "enddate" | "state")).map(|(k, v)| (k.clone(), v.clone())));
        out.push(((start, model_id), Value::Object(segment)));
    }
    out.sort_by_key(|(key, _)| *key);
    Ok(out.into_iter().map(|(_, segment)| segment).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use serde_json::json;

    fn tokyo() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    #[test]
    fn names_the_state_and_shows_local_times() {
        let items = [json!({"startdate": 1794929776, "state": 2, "enddate": 1794930739, "model": "HealthKit step Apple Watch tracker", "hr": {"1794930010": 61}, "model_id": 1058})];
        assert_eq!(
            segments(&items, &tokyo()).unwrap(),
            [json!({"start": "2026-11-18T00:36:16+09:00", "end": "2026-11-18T00:52:19+09:00", "state": "deep", "model": "HealthKit step Apple Watch tracker", "hr": {"1794930010": 61}, "model_id": 1058})]
        );
        let unknown = segments(&[json!({"startdate": 0, "enddate": 1, "state": 9})], &tokyo()).unwrap();
        assert_eq!(unknown[0]["state"], "state_9");
    }

    #[test]
    fn drops_segments_returned_twice_and_sorts_them() {
        let later = json!({"startdate": 200, "enddate": 300, "state": 1, "model_id": 1058});
        let earlier = json!({"startdate": 100, "enddate": 200, "state": 3, "model_id": 1058});
        let other_source = json!({"startdate": 100, "enddate": 200, "state": 3, "model_id": 1059});
        let out = segments(&[later.clone(), earlier.clone(), later, other_source], &tokyo()).unwrap();
        let summary: Vec<_> = out.iter().map(|s| (s["state"].clone(), s["model_id"].clone())).collect();
        assert_eq!(summary, [(json!("rem"), json!(1058)), (json!("rem"), json!(1059)), (json!("light"), json!(1058))]);
    }

    #[test]
    fn rejects_a_segment_without_times() {
        let err = segments(&[json!({"state": 1})], &tokyo()).unwrap_err().to_string();
        assert!(err.contains("startdate"), "{err}");
    }
}
