//! Printing results as JSON, JSON lines or an aligned table.

use clap::ValueEnum;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum Format {
    /// One JSON array
    Json,
    /// One JSON object per line
    Jsonl,
    /// Aligned columns; nested objects become dotted columns
    Table,
}

pub fn render(items: &[Value], format: Format) -> String {
    match format {
        Format::Json => format!("{}\n", serde_json::to_string_pretty(items).expect("JSON values serialize")),
        Format::Jsonl => items.iter().map(|item| format!("{item}\n")).collect(),
        Format::Table => table(items),
    }
}

fn cell(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or_default();
            if f.fract() == 0.0 { format!("{f:.1}") } else { f.to_string() }
        }
        other => other.to_string(),
    }
}

fn table(items: &[Value]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let rows: Vec<Vec<(String, Value)>> = items.iter().map(flatten).collect();
    let mut columns: Vec<String> = Vec::new();
    for row in &rows {
        for (key, _) in row {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| columns.iter().map(|c| row.iter().find(|(k, _)| k == c).map(|(_, v)| cell(v)).unwrap_or_default()).collect())
        .collect();
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| cells.iter().map(|r| r[i].chars().count()).chain([c.chars().count()]).max().unwrap_or(0))
        .collect();
    let line = |values: Vec<&str>| {
        let padded: Vec<String> = values.iter().zip(&widths).map(|(v, w)| format!("{v:<w$}")).collect();
        format!("{}\n", padded.join("  ").trim_end())
    };
    let mut out = line(columns.iter().map(String::as_str).collect());
    for row in &cells {
        out.push_str(&line(row.iter().map(String::as_str).collect()));
    }
    out
}

/// Top-level keys in order, with nested objects expanded one level
/// (`data.total_sleep_time`).
pub fn flatten(item: &Value) -> Vec<(String, Value)> {
    let Value::Object(map) = item else { return vec![("value".into(), item.clone())] };
    let mut out = Vec::new();
    for (key, value) in map {
        match value {
            Value::Object(inner) => out.extend(inner.iter().map(|(k, v)| (format!("{key}.{k}"), v.clone()))),
            other => out.push((key.clone(), other.clone())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_and_jsonl() {
        let items = [json!({"a": 1}), json!({"a": 2})];
        assert_eq!(render(&items, Format::Json), "[\n  {\n    \"a\": 1\n  },\n  {\n    \"a\": 2\n  }\n]\n");
        assert_eq!(render(&items, Format::Jsonl), "{\"a\":1}\n{\"a\":2}\n");
        assert_eq!(render(&[], Format::Jsonl), "");
    }

    #[test]
    fn flattens_one_level_of_nesting() {
        let item = json!({"date": "2026-11-17", "data": {"total_sleep_time": 25200, "hr": {"min": 50}}, "tags": [1, 2]});
        let keys: Vec<_> = flatten(&item).into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(keys, ["date=\"2026-11-17\"", "data.total_sleep_time=25200", "data.hr={\"min\":50}", "tags=[1,2]"]);
    }

    #[test]
    fn table_aligns_the_union_of_columns() {
        let items = [json!({"date": "2026-11-16", "weight": 68.4}), json!({"date": "2026-11-17", "weight": 68.0, "fat_ratio": 18.5})];
        assert_eq!(
            render(&items, Format::Table),
            "date        weight  fat_ratio\n2026-11-16  68.4\n2026-11-17  68.0    18.5\n"
        );
        assert_eq!(render(&[], Format::Table), "");
    }
}
