//! Body measurements (`measure` / `getmeas`): measure type names and turning
//! measure groups into flat records.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use chrono::{DateTime, FixedOffset, TimeZone};
use serde::{Deserialize, Serialize};

/// Withings `meastype` ids with the names this CLI uses for them.
pub const TYPES: &[(i64, &str)] = &[
    (1, "weight"),
    (4, "height"),
    (5, "fat_free_mass"),
    (6, "fat_ratio"),
    (8, "fat_mass"),
    (9, "diastolic_bp"),
    (10, "systolic_bp"),
    (11, "heart_pulse"),
    (12, "temperature"),
    (54, "spo2"),
    (71, "body_temperature"),
    (73, "skin_temperature"),
    (76, "muscle_mass"),
    (77, "hydration"),
    (88, "bone_mass"),
    (91, "pulse_wave_velocity"),
    (123, "vo2max"),
];

const REAL_MEASUREMENT: i64 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct MeasureGroup {
    pub grpid: u64,
    pub date: i64,
    pub category: i64,
    pub measures: Vec<RawMeasure>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawMeasure {
    pub value: i64,
    #[serde(rename = "type")]
    pub kind: i64,
    pub unit: i32,
}

/// One measure group as printed: fixed fields, then one key per measure.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Record {
    pub date: DateTime<FixedOffset>,
    pub grpid: u64,
    /// "real" for a measurement, "goal" for a target typed in the app.
    pub category: &'static str,
    #[serde(flatten)]
    pub values: BTreeMap<String, f64>,
}

/// Name for a meastype id; unknown ids become `type_<id>`.
pub fn type_name(id: i64) -> String {
    TYPES.iter().find(|(t, _)| *t == id).map(|(_, name)| name.to_string()).unwrap_or_else(|| format!("type_{id}"))
}

/// Comma-separated names or ids, e.g. `weight,fat_ratio,76`.
pub fn parse_types(list: &str) -> Result<Vec<i64>> {
    list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(type_id).collect()
}

fn type_id(name: &str) -> Result<i64> {
    if let Ok(id) = name.parse() {
        return Ok(id);
    }
    match TYPES.iter().find(|(_, n)| *n == name) {
        Some((id, _)) => Ok(*id),
        None => {
            let known: Vec<_> = TYPES.iter().map(|(_, n)| *n).collect();
            bail!("unknown measure type '{name}'; use one of {} or a numeric meastype id", known.join(", "))
        }
    }
}

/// Withings sends integers with a power-of-ten exponent: 6845 with unit -2
/// is 68.45.
pub fn scaled(value: i64, unit: i32) -> f64 {
    let raw = value as f64 * 10f64.powi(unit);
    if unit < 0 {
        // Round to the precision Withings sent so 6845e-2 is 68.45, not 68.45000000000001.
        let factor = 10f64.powi(-unit);
        (raw * factor).round() / factor
    } else {
        raw
    }
}

/// Groups as records, oldest first.
pub fn to_records<Tz: TimeZone>(groups: &[MeasureGroup], tz: &Tz) -> Vec<Record> {
    let mut records: Vec<Record> = groups
        .iter()
        .filter_map(|g| {
            let date = tz.timestamp_opt(g.date, 0).single()?.fixed_offset();
            let values = g.measures.iter().map(|m| (type_name(m.kind), scaled(m.value, m.unit))).collect();
            let category = if g.category == REAL_MEASUREMENT { "real" } else { "goal" };
            Some(Record { date, grpid: g.grpid, category, values })
        })
        .collect();
    records.sort_by_key(|r| (r.date, r.grpid));
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokyo() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    fn group(grpid: u64, date: i64, category: i64, measures: &[(i64, i64, i32)]) -> MeasureGroup {
        MeasureGroup {
            grpid,
            date,
            category,
            measures: measures.iter().map(|&(kind, value, unit)| RawMeasure { value, kind, unit }).collect(),
        }
    }

    #[test]
    fn scales_by_the_unit_exponent() {
        assert_eq!(scaled(6845, -2), 68.45);
        assert_eq!(scaled(185, -1), 18.5);
        assert_eq!(scaled(7, 1), 70.0);
    }

    #[test]
    fn names_known_and_unknown_types() {
        assert_eq!(type_name(1), "weight");
        assert_eq!(type_name(76), "muscle_mass");
        assert_eq!(type_name(999), "type_999");
    }

    #[test]
    fn parses_names_and_ids() {
        assert_eq!(parse_types("weight, fat_ratio,76").unwrap(), [1, 6, 76]);
    }

    #[test]
    fn rejects_an_unknown_name_and_lists_the_known_ones() {
        let err = parse_types("weight,wieght").unwrap_err().to_string();
        assert!(err.contains("wieght") && err.contains("fat_ratio"), "{err}");
    }

    #[test]
    fn flattens_groups_into_named_values_oldest_first() {
        let groups = [
            group(2, 1_794_870_000, 1, &[(1, 6845, -2), (6, 185, -1), (999, 3, 0)]),
            group(1, 1_794_783_600, 2, &[(1, 6300, -2)]),
        ];
        let records = to_records(&groups, &tokyo());
        assert_eq!(records.iter().map(|r| r.grpid).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(records[0].category, "goal");
        let r = &records[1];
        assert_eq!((r.category, r.date.to_rfc3339()), ("real", "2026-11-17T08:00:00+09:00".to_string()));
        assert_eq!(
            serde_json::to_string(r).unwrap(),
            r#"{"date":"2026-11-17T08:00:00+09:00","grpid":2,"category":"real","fat_ratio":18.5,"type_999":3.0,"weight":68.45}"#
        );
    }
}
