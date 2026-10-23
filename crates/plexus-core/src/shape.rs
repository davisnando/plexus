//! Result shaping for meta-mode `call_tool`: keep only selected JSON fields
//! and cap the size, so a 60 KB Jira search doesn't flood the client context.

use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Map, Value};

/// One step of a field path: `issues[].fields.summary` is
/// `Key(issues) Each Key(fields) Key(summary)`.
#[derive(Debug, PartialEq)]
enum Segment {
    Key(String),
    Each,
}

fn parse_path(path: &str) -> Vec<Segment> {
    let mut out = vec![];
    for part in path.split('.').filter(|p| !p.is_empty()) {
        let mut key = part;
        let mut each = 0;
        while let Some(rest) = key.strip_suffix("[]") {
            key = rest;
            each += 1;
        }
        if !key.is_empty() {
            out.push(Segment::Key(key.to_string()));
        }
        out.extend((0..each).map(|_| Segment::Each));
    }
    out
}

/// The parts of `value` reachable through `path`, keeping their structure.
/// Arrays keep their length (non-matching elements become `null`) so that
/// projections of several paths line up when merged.
fn project(value: &Value, path: &[Segment]) -> Option<Value> {
    let Some((first, rest)) = path.split_first() else {
        return Some(value.clone());
    };
    match (first, value) {
        (Segment::Key(k), Value::Object(o)) => {
            let inner = project(o.get(k)?, rest)?;
            Some(Value::Object(Map::from_iter([(k.clone(), inner)])))
        }
        // Be lenient: `issues.key` on an array means `issues[].key`.
        (Segment::Key(_), Value::Array(items)) | (Segment::Each, Value::Array(items)) => {
            let path = if matches!(first, Segment::Each) {
                rest
            } else {
                path
            };
            let projected: Vec<Value> = items
                .iter()
                .map(|v| project(v, path).unwrap_or(Value::Null))
                .collect();
            projected
                .iter()
                .any(|v| !v.is_null())
                .then_some(Value::Array(projected))
        }
        _ => None,
    }
}

fn merge(into: &mut Value, other: Value) {
    match (into, other) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(&k) {
                    Some(existing) => merge(existing, v),
                    None => {
                        a.insert(k, v);
                    }
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, v) in b.into_iter().enumerate() {
                match a.get_mut(i) {
                    Some(existing) => merge(existing, v),
                    None => a.push(v),
                }
            }
        }
        (slot @ Value::Null, v) => *slot = v,
        (_, Value::Null) => {}
        (slot, v) => *slot = v,
    }
}

/// Keep only `paths` of `value`; `None` when none of them matched.
pub fn select(value: &Value, paths: &[String]) -> Option<Value> {
    let mut out: Option<Value> = None;
    for path in paths {
        if let Some(p) = project(value, &parse_path(path)) {
            match &mut out {
                Some(acc) => merge(acc, p),
                None => out = Some(p),
            }
        }
    }
    out
}

#[derive(Debug, Default, Clone)]
pub struct Shape {
    /// Field paths to keep from JSON results, e.g. `issues[].key`.
    pub fields: Vec<String>,
    /// Cap on the total text length; 0 means no cap.
    pub max_chars: usize,
}

/// Apply `shape` to a tool result. Text blocks that parse as JSON are
/// projected onto `fields`; then all text is capped at `max_chars` with a note
/// saying how much was cut. Structured content is dropped once the text has
/// been reshaped, since it would carry the full payload again.
pub fn apply(mut result: CallToolResult, shape: &Shape) -> CallToolResult {
    if result.content.is_empty()
        && let Some(structured) = result.structured_content.take()
    {
        result.content.push(ContentBlock::text(
            serde_json::to_string_pretty(&structured).unwrap_or_default(),
        ));
    }
    let mut reshaped = false;
    if !shape.fields.is_empty() {
        let mut unmatched = false;
        for block in &mut result.content {
            let Some(text) = block.as_text().map(|t| t.text.clone()) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let selected = select(&json, &shape.fields);
            unmatched |= selected.is_none();
            let selected = selected.unwrap_or(Value::Null);
            *block =
                ContentBlock::text(serde_json::to_string_pretty(&selected).unwrap_or_default());
            reshaped = true;
        }
        if unmatched {
            result.content.push(ContentBlock::text(format!(
                "[plexus: none of the fields {:?} matched; call again without `fields` to see the full result]",
                shape.fields
            )));
        }
    }
    if shape.max_chars > 0 {
        let total: usize = result
            .content
            .iter()
            .filter_map(|b| b.as_text().map(|t| t.text.chars().count()))
            .sum();
        if total > shape.max_chars {
            let mut budget = shape.max_chars;
            for block in &mut result.content {
                let Some(text) = block.as_text().map(|t| t.text.clone()) else {
                    continue;
                };
                let len = text.chars().count();
                if len > budget {
                    *block = ContentBlock::text(text.chars().take(budget).collect::<String>());
                }
                budget = budget.saturating_sub(len);
            }
            result
                .content
                .retain(|b| b.as_text().is_none_or(|t| !t.text.is_empty()));
            result.content.push(ContentBlock::text(format!(
                "[plexus: truncated, showing {} of {total} characters. Pass `fields` to select \
                 what you need, or raise `max_chars` (0 = no limit).]",
                shape.max_chars
            )));
            reshaped = true;
        }
    }
    if reshaped {
        result.structured_content = None;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issues() -> Value {
        json!({
            "total": 2,
            "issues": [
                {"key": "A-1", "fields": {"summary": "One", "status": {"name": "Open"}, "body": "…"}},
                {"key": "A-2", "fields": {"summary": "Two", "status": {"name": "Done"}, "body": "…"}}
            ]
        })
    }

    #[test]
    fn selects_nested_fields_across_arrays() {
        let paths = vec![
            "total".to_string(),
            "issues[].key".to_string(),
            "issues[].fields.status.name".to_string(),
        ];
        assert_eq!(
            select(&issues(), &paths).unwrap(),
            json!({
                "total": 2,
                "issues": [
                    {"key": "A-1", "fields": {"status": {"name": "Open"}}},
                    {"key": "A-2", "fields": {"status": {"name": "Done"}}}
                ]
            })
        );
    }

    #[test]
    fn implicit_iteration_and_root_arrays() {
        let paths = vec!["issues.key".to_string()];
        assert_eq!(
            select(&issues(), &paths).unwrap(),
            json!({"issues": [{"key": "A-1"}, {"key": "A-2"}]})
        );
        let root = json!([{"id": 1, "x": 0}, {"id": 2}]);
        assert_eq!(
            select(&root, &["[].id".into()]).unwrap(),
            json!([{"id": 1}, {"id": 2}])
        );
    }

    #[test]
    fn missing_paths_select_nothing() {
        assert_eq!(select(&issues(), &["nope".into()]), None);
    }

    #[test]
    fn truncates_with_a_note() {
        let r = CallToolResult::success(vec![ContentBlock::text("x".repeat(100))]);
        let r = apply(
            r,
            &Shape {
                fields: vec![],
                max_chars: 10,
            },
        );
        assert_eq!(r.content[0].as_text().unwrap().text, "x".repeat(10));
        assert!(
            r.content[1]
                .as_text()
                .unwrap()
                .text
                .contains("showing 10 of 100")
        );
    }

    #[test]
    fn projects_json_text_blocks() {
        let r = CallToolResult::success(vec![ContentBlock::text(issues().to_string())]);
        let r = apply(
            r,
            &Shape {
                fields: vec!["issues[].key".into()],
                max_chars: 0,
            },
        );
        let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(v, json!({"issues": [{"key": "A-1"}, {"key": "A-2"}]}));
    }
}
