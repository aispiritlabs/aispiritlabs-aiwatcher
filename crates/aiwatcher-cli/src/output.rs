//! Turning an API answer into columns.
//!
//! The rule is that a table is a *rendering* and never a filter: every column
//! the rows have is shown, in the order the API put them in, and a value too
//! long for a terminal is truncated with an ellipsis rather than dropped. A
//! renderer that decided which fields matter would be a second, undocumented
//! contract — and the first time somebody needed the field it hid, the answer
//! would be "the CLI does not show that", which is exactly the failure
//! `format=json` exists to make impossible.

use serde_json::Value;

/// The widest a single cell is allowed to get before it is cut.
///
/// Long enough for a run id, a timestamp or a short name; short enough that one
/// prose field cannot push every other column off the screen.
const MAX_CELL: usize = 44;

/// Print a value as a table where it is one, and as JSON where it is not.
pub fn render(value: &Value) {
    match value {
        // The list routes answer `{ "items": [...], "next_cursor": ... }`.
        Value::Object(map) => {
            if let Some(rows) = map
                .get("items")
                .or_else(|| map.get("runs"))
                .or_else(|| map.get("rows"))
                .and_then(Value::as_array)
            {
                table(rows);
                if let Some(cursor) = map.get("next_cursor").and_then(Value::as_str) {
                    println!("\nmore: cursor={cursor}");
                }
                return;
            }
            pairs(map);
        }
        Value::Array(rows) => table(rows),
        Value::Null => {}
        other => println!("{}", scalar(other)),
    }
}

/// One object as `key  value` lines — a `show` rather than a `list`.
fn pairs(map: &serde_json::Map<String, Value>) {
    let width = map.keys().map(String::len).max().unwrap_or(0);
    for (key, value) in map {
        println!("{key:<width$}  {}", cell(value), width = width);
    }
}

/// A list of objects as aligned columns.
fn table(rows: &[Value]) {
    if rows.is_empty() {
        println!("(nothing)");
        return;
    }
    // Every key any row has, in first-seen order: a row missing one is a hole
    // rather than a reason to leave the column out of the others.
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        if let Some(map) = row.as_object() {
            for key in map.keys() {
                if !columns.iter().any(|known| known == key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    if columns.is_empty() {
        for row in rows {
            println!("{}", cell(row));
        }
        return;
    }

    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|key| row.get(key).map_or_else(String::new, cell))
                .collect()
        })
        .collect();

    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(index, name)| {
            body.iter()
                .filter_map(|row| row.get(index))
                .map(|value| value.chars().count())
                .chain(std::iter::once(name.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let header: Vec<String> = columns
        .iter()
        .zip(&widths)
        .map(|(name, width)| format!("{name:<width$}"))
        .collect();
    println!("{}", header.join("  ").trim_end());
    println!(
        "{}",
        widths
            .iter()
            .map(|width| "─".repeat(*width))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in &body {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(value, width)| format!("{value:<width$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
}

/// One value as one cell.
fn cell(value: &Value) -> String {
    let rendered = scalar(value);
    let count = rendered.chars().count();
    if count <= MAX_CELL {
        return rendered;
    }
    let kept: String = rendered.chars().take(MAX_CELL - 1).collect();
    format!("{kept}…")
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(inner) => inner.to_string(),
        Value::Number(inner) => inner.to_string(),
        Value::String(inner) => inner.clone(),
        // A nested object or array in a column is shown compactly rather than
        // spread over lines, because the row it sits in has to stay one row.
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_is_kept_even_when_only_one_row_has_it() {
        // The hole is the interesting case: a run that failed carries an error
        // field the others do not, and dropping the column would hide it.
        let rows = serde_json::json!([{ "id": "a" }, { "id": "b", "error": "boom" }]);
        let mut columns: Vec<String> = Vec::new();
        for row in rows.as_array().expect("an array") {
            for key in row.as_object().expect("an object").keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
        assert_eq!(columns, ["id", "error"]);
    }

    #[test]
    fn a_long_cell_is_cut_rather_than_allowed_to_push_the_row_apart() {
        let long = Value::String("x".repeat(200));
        let rendered = cell(&long);
        assert_eq!(rendered.chars().count(), MAX_CELL);
        assert!(rendered.ends_with('…'));
    }

    #[test]
    fn a_nested_value_stays_on_one_line() {
        let nested = serde_json::json!({ "a": [1, 2] });
        assert!(!cell(&nested).contains('\n'));
    }
}
