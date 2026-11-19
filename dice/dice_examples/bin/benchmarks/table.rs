/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;

use json_value::OrderedValue;

/// A row of cells keyed by column name. Column order is tracked separately in `headers`.
type Row = HashMap<String, Cell>;

/// Unit of a numeric value, inferred from the JSON key suffix.
#[derive(Clone, Copy, Default, PartialEq)]
enum Unit {
    Seconds,
    Milliseconds,
    Bytes,
    #[default]
    None,
}

impl Unit {
    fn suffix(self) -> Option<&'static str> {
        match self {
            Unit::Seconds => Some("_secs"),
            Unit::Milliseconds => Some("_ms"),
            Unit::Bytes => Some("_bytes"),
            Unit::None => None,
        }
    }

    /// Infer the unit from the JSON key suffix. The benchmark emits keys
    /// like `duration_secs`, `cpu_user_ms`, `rss_anon_bytes`, `db_size_bytes` —
    /// the suffix determines how the value is formatted.
    fn from_key(key: &str) -> Self {
        for unit in [Unit::Seconds, Unit::Milliseconds, Unit::Bytes] {
            let suffix = unit.suffix().unwrap();
            if key.ends_with(suffix) || key.contains(&format!("{suffix}_")) {
                return unit;
            }
        }
        Unit::None
    }

    fn format(self, raw: f64) -> String {
        match self {
            Unit::Seconds => format!("{:.2}s", raw),
            Unit::Milliseconds => format!("{}ms", raw as u64),
            Unit::Bytes => human_bytes(raw as u64),
            Unit::None => human_number(raw as u64),
        }
    }

    /// Strip the unit suffix from a key (e.g. `duration_secs` → `duration`).
    fn strip_suffix(self, key: &str) -> &str {
        match self.suffix() {
            Some(suffix) => key.strip_suffix(suffix).unwrap_or(key),
            None => key,
        }
    }
}

/// A flattened JSON field with its key, display string, raw numeric value, and unit.
#[derive(Clone, Default)]
struct Cell {
    key: String,
    display: String,
    raw: Option<f64>,
}

/// Render a JSON object as a single-row markdown table with keys as column headers.
pub fn kv_table(json: &OrderedValue) -> String {
    let cells: Vec<(String, String)> = flatten_json(json)
        .into_iter()
        .filter(|c| c.key != "type")
        .map(|c| (strip_unit(&c.key), c.display))
        .collect();
    let widths: Vec<usize> = cells.iter().map(|(k, v)| k.len().max(v.len())).collect();
    let fmt = |items: &[&str]| -> String {
        let padded: Vec<String> = items
            .iter()
            .zip(&widths)
            .map(|(s, &w)| format!("{:<width$}", s, width = w))
            .collect();
        format!("| {} |\n", padded.join(" | "))
    };
    let headers: Vec<&str> = cells.iter().map(|(k, _)| k.as_str()).collect();
    let values: Vec<&str> = cells.iter().map(|(_, v)| v.as_str()).collect();
    let sep: Vec<String> = widths.iter().map(|&w| "-".repeat(w)).collect();
    let mut out = fmt(&headers);
    out.push_str(&format!("| {} |\n", sep.join(" | ")));
    out.push_str(&fmt(&values));
    out
}

/// Whether a column should show deltas instead of absolute values.
fn show_delta(key: &str) -> bool {
    Unit::from_key(key) == Unit::Bytes && key != "peak_rss_bytes" && !key.contains("_cum_")
}

/// Convert raw stage rows into formatted rows, for non-final rows
/// formatting deltas (ex. +14MB) and for the final row formatting absolute values.
fn format_deltas(headers: &[String], raw_values: &[Row]) -> Vec<Vec<String>> {
    let delta_cols: Vec<(usize, &str)> = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| show_delta(h))
        .map(|(i, h)| (i, h.as_str()))
        .collect();
    let mut prev: HashMap<&str, u64> = HashMap::new();
    let mut rows = Vec::new();
    for (i, raw_row) in raw_values.iter().enumerate() {
        let is_last = i == raw_values.len() - 1;
        let mut row: Vec<String> = headers
            .iter()
            .map(|h| {
                raw_row
                    .get(h)
                    .map(|c| c.display.clone())
                    .unwrap_or_default()
            })
            .collect();
        for &(col, key) in &delta_cols {
            if let Some(raw) = raw_row.get(key).and_then(|c| c.raw) {
                let raw = raw as u64;
                if is_last {
                    row[col] = human_bytes(raw);
                } else {
                    let p = prev.get(key).copied().unwrap_or(0);
                    let delta = raw as i64 - p as i64;
                    row[col] = if delta > 0 {
                        format!("+{}", human_bytes(delta as u64))
                    } else if delta < 0 {
                        format!("-{}", human_bytes((-delta) as u64))
                    } else {
                        "0B".to_owned()
                    };
                }
                prev.insert(key, raw);
            }
        }
        rows.push(row);
    }
    rows
}

/// How a column should be aggregated in the total row.
enum Aggregate {
    /// Sum across all stages (duration, CPU time).
    Sum,
    /// Max across all stages (peak RSS).
    Max,
    /// Use the last stage's value (jemalloc, rss, db size).
    Last,
}

fn aggregate_for(key: &str) -> Aggregate {
    match key {
        "duration_secs" | "cpu_user_ms" | "cpu_system_ms" => Aggregate::Sum,
        "peak_rss_bytes" => Aggregate::Max,
        _ => Aggregate::Last,
    }
}

/// Compute a total row by aggregating stage data.
fn compute_total_row(headers: &[String], raw_values: &[Row]) -> Row {
    let label = raw_values[0]
        .get("stage")
        .map(|c| c.display.as_str())
        .unwrap_or("");
    let total_label = match label.rsplit_once('.') {
        Some((prefix, _)) => format!("{}.total", prefix),
        None => "total".to_owned(),
    };

    fn raw_col<'a>(h: &'a str, raw_values: &'a [Row]) -> impl Iterator<Item = f64> + 'a {
        raw_values
            .iter()
            .filter_map(move |r| r.get(h).and_then(|c| c.raw))
    }

    headers
        .iter()
        .map(|h| {
            let cell = if h == "stage" {
                Cell {
                    key: h.clone(),
                    display: total_label.clone(),
                    raw: None,
                }
            } else {
                let unit = Unit::from_key(h);
                match aggregate_for(h) {
                    Aggregate::Sum => {
                        let val: f64 = raw_col(h, raw_values).sum();
                        Cell {
                            key: h.clone(),
                            display: unit.format(val),
                            raw: Some(val),
                        }
                    }
                    Aggregate::Max => {
                        let val = raw_col(h, raw_values).fold(0.0_f64, f64::max);
                        Cell {
                            key: h.clone(),
                            display: unit.format(val),
                            raw: Some(val),
                        }
                    }
                    Aggregate::Last => raw_values
                        .last()
                        .and_then(|r| r.get(h).cloned())
                        .unwrap_or_default(),
                }
            };
            (h.clone(), cell)
        })
        .collect()
}

/// Insert a cumulative (absolute value) column right before the given key.
/// The new column copies the raw absolute values before `format_deltas` converts the original to deltas.
fn insert_cum_col(key: &str, headers: &mut Vec<String>, raw_values: &mut [Row]) {
    let Some(pos) = headers.iter().position(|h| h == key) else {
        return;
    };
    let cum_key = key.replace("_bytes", "_cum_bytes");
    headers.insert(pos, cum_key.clone());
    for row in raw_values.iter_mut() {
        if let Some(cell) = row.get(key).cloned() {
            row.insert(
                cum_key.clone(),
                Cell {
                    key: cum_key.clone(),
                    ..cell
                },
            );
        }
    }
}

/// Render JSON lines as a multi-column markdown table.
/// Columns are discovered from the first line; nested objects are flattened.
pub fn stages_table(lines: &[&str]) -> anyhow::Result<String> {
    let mut headers: Vec<String> = Vec::new();
    let mut raw_values: Vec<Row> = Vec::new();

    for line in lines {
        let v: OrderedValue = serde_json::from_str(line)?;
        let flat = flatten_json(&v);
        if headers.is_empty() {
            headers = flat.iter().map(|c| c.key.clone()).collect();
        }
        let row: Row = flat.into_iter().map(|c| (c.key.clone(), c)).collect();
        raw_values.push(row);
    }

    // Insert cumulative column for rss_anon (absolute value, before deltas are applied).
    insert_cum_col("rss_anon_bytes", &mut headers, &mut raw_values);

    // Compute a total row from the stage data.
    if !raw_values.is_empty() {
        raw_values.push(compute_total_row(&headers, &raw_values));
    }

    let rows = format_deltas(&headers, &raw_values);

    let header_labels: Vec<String> = headers
        .iter()
        .map(|h| {
            let label = strip_unit(h);
            if show_delta(h) {
                format!("\u{0394} {}", label)
            } else {
                label
            }
        })
        .collect();
    let mut widths: Vec<usize> = header_labels.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let fmt_row = |cells: &[&str], widths: &[usize]| -> String {
        let padded: Vec<String> = cells
            .iter()
            .zip(widths)
            .map(|(c, &w)| format!("{:<width$}", c, width = w))
            .collect();
        format!("| {} |\n", padded.join(" | "))
    };

    let mut out = String::new();
    let header_refs: Vec<&str> = header_labels.iter().map(|s| s.as_str()).collect();
    out.push_str(&fmt_row(&header_refs, &widths));
    let sep: Vec<String> = widths.iter().map(|&w| "-".repeat(w)).collect();
    out.push_str(&format!("| {} |\n", sep.join(" | ")));
    for row in &rows {
        let refs: Vec<&str> = row.iter().map(|s| s.as_str()).collect();
        out.push_str(&fmt_row(&refs, &widths));
    }
    Ok(out)
}

/// Flatten a JSON object into `Cell`s with underscore-joined keys.
/// Values are formatted based on key suffix: `_bytes` → human_bytes,
/// `_secs` → seconds, `_ms` → milliseconds.
fn flatten_json(v: &OrderedValue) -> Vec<Cell> {
    let mut out = Vec::new();
    if let Some(obj) = v.as_object() {
        for (k, v) in obj {
            flatten_json_inner(k, v, &mut out);
        }
    }
    out
}

fn flatten_json_inner(prefix: &str, v: &OrderedValue, out: &mut Vec<Cell>) {
    match v {
        OrderedValue::Object(obj) => {
            for (k, v) in obj {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}_{}", prefix, k)
                };
                flatten_json_inner(&key, v, out);
            }
        }
        _ => {
            out.push(Cell {
                key: prefix.to_owned(),
                display: format_json_value(prefix, v),
                raw: v.as_f64(),
            });
        }
    }
}

fn format_json_value(key: &str, v: &OrderedValue) -> String {
    match v {
        OrderedValue::Number(n) => Unit::from_key(key).format(n.as_f64().unwrap_or(0.0)),
        OrderedValue::String(s) => s.clone(),
        OrderedValue::Null => "none".to_owned(),
        _ => format!("{}", v),
    }
}

fn human_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2}GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.0}MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0}KB", bytes as f64 / 1_000.0)
    } else {
        format!("{}B", bytes)
    }
}

fn human_number(n: u64) -> String {
    if n >= 1_000_000 && n.is_multiple_of(1_000_000) {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 && n.is_multiple_of(1_000) {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Strip unit suffixes from a key name and replace underscores with spaces.
/// Units are already shown in the formatted values.
fn strip_unit(key: &str) -> String {
    Unit::from_key(key)
        .strip_suffix(key)
        .replace("_cum", "")
        .replace('_', " ")
}
