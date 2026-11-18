//! Minute-level activity (`v2/measure` / `getintradayactivity`). Withings
//! sends `series` as an object keyed by Unix time; this turns it into rows.

use anyhow::{Context, Result, bail};
use chrono::TimeZone;
use serde_json::{Map, Value};

/// A Unix time as a local RFC 3339 string, the same form `measure` prints.
pub fn local_time<Tz: TimeZone>(unix: i64, tz: &Tz) -> Option<Value> {
    serde_json::to_value(tz.timestamp_opt(unix, 0).single()?.fixed_offset()).ok()
}

/// Rows `{time, steps, heart_rate, ...}` of one response body, oldest first.
pub fn rows<Tz: TimeZone>(body: &Value, tz: &Tz) -> Result<Vec<Value>> {
    let series = match body.get("series") {
        Some(Value::Object(series)) => series,
        // An empty answer comes as {} or [].
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) if items.is_empty() => return Ok(Vec::new()),
        Some(other) => bail!("Withings sent intraday `series` in an unexpected shape: {}", other.to_string().chars().take(200).collect::<String>()),
    };
    let mut rows = Vec::with_capacity(series.len());
    for (key, fields) in series {
        let unix: i64 = key.parse().with_context(|| format!("Withings sent an intraday key '{key}' that is not a Unix time"))?;
        let time = local_time(unix, tz).with_context(|| format!("intraday time {unix} is out of range"))?;
        let mut row = Map::new();
        row.insert("time".into(), time);
        if let Value::Object(fields) = fields {
            row.extend(fields.clone());
        }
        rows.push((unix, Value::Object(row)));
    }
    rows.sort_by_key(|(unix, _)| *unix);
    Ok(rows.into_iter().map(|(_, row)| row).collect())
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
    fn turns_the_series_object_into_rows() {
        let body = json!({"series": {
            "1794930060": {"heart_rate": 63, "model": "HealthKit other step tracker", "model_id": 1059},
            "1794930000": {"steps": 12, "distance": 9.5, "model_id": 1059}
        }});
        assert_eq!(
            rows(&body, &tokyo()).unwrap(),
            [
                json!({"time": "2026-11-18T00:40:00+09:00", "steps": 12, "distance": 9.5, "model_id": 1059}),
                json!({"time": "2026-11-18T00:41:00+09:00", "heart_rate": 63, "model": "HealthKit other step tracker", "model_id": 1059}),
            ]
        );
    }

    #[test]
    fn an_empty_series_has_no_rows() {
        assert!(rows(&json!({"series": {}}), &tokyo()).unwrap().is_empty());
        assert!(rows(&json!({"series": []}), &tokyo()).unwrap().is_empty());
        assert!(rows(&json!({}), &tokyo()).unwrap().is_empty());
    }

    #[test]
    fn rejects_a_key_that_is_not_a_time() {
        let err = rows(&json!({"series": {"soon": {"steps": 1}}}), &tokyo()).unwrap_err().to_string();
        assert!(err.contains("soon"), "{err}");
    }
}
