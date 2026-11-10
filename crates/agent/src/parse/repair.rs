//! Schema-guided argument repair (ARCHITECTURE.md 8.3, step 4).
//!
//! Only what the schema makes unambiguous is changed:
//!
//! - arguments double-encoded as a JSON string are decoded (also a nested object or array
//!   given as a string where the schema wants one);
//! - `"5"` becomes `5` and `"true"` becomes `true` where the schema demands that type;
//! - a `null` for an optional field is dropped;
//! - a single value is wrapped into a list where the schema wants an array.
//!
//! A schema position with `anyOf`, `oneOf`, `allOf`, `$ref`, or several possible types is
//! left alone, and so is anything deeper than [`MAX_DEPTH`].

use serde_json::{Map, Number, Value};

/// How deep into nested objects and arrays repair goes.
pub const MAX_DEPTH: usize = 8;

/// Repairs tool-call arguments against the tool's JSON Schema. Returns the repaired value,
/// which validation then checks; the result is not guaranteed to be an object (a string
/// that does not decode to one stays a string, and validation rejects it).
pub fn repair_arguments(args: Value, schema: &Value) -> Value {
    let args = match args {
        Value::Null => Value::Object(Map::new()),
        Value::String(text) => decode_object_string(text),
        other => other,
    };
    match args {
        Value::Object(map) => repair_object(map, schema, 0),
        other => other,
    }
}

/// Decodes arguments sent as a JSON string, at most twice (a doubly double-encoded object is
/// still unambiguous). Anything that does not end up as an object is returned unchanged.
fn decode_object_string(text: String) -> Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Value::Object(Map::new());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(Value::String(inner)) => match serde_json::from_str::<Value>(inner.trim()) {
            Ok(Value::Object(map)) => Value::Object(map),
            _ => Value::String(text),
        },
        _ => Value::String(text),
    }
}

/// The one type a schema position demands, when that is unambiguous.
fn single_type(schema: &Value) -> Option<&str> {
    let object = schema.as_object()?;
    if ["anyOf", "oneOf", "allOf", "$ref", "not"]
        .iter()
        .any(|key| object.contains_key(*key))
    {
        return None;
    }
    match object.get("type") {
        Some(Value::String(kind)) => Some(kind.as_str()),
        Some(Value::Array(kinds)) => {
            let mut non_null = kinds
                .iter()
                .filter_map(Value::as_str)
                .filter(|kind| *kind != "null");
            let first = non_null.next()?;
            non_null.next().is_none().then_some(first)
        }
        Some(_) => None,
        None if object.contains_key("properties") => Some("object"),
        None if object.contains_key("items") => Some("array"),
        None => None,
    }
}

/// True when the schema position accepts `null` explicitly.
fn allows_null(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return true;
    };
    if object.get("nullable") == Some(&Value::Bool(true)) {
        return true;
    }
    if let Some(Value::Array(values)) = object.get("enum")
        && values.contains(&Value::Null)
    {
        return true;
    }
    match object.get("type") {
        Some(Value::String(kind)) => kind == "null",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "null"),
        // No type constraint: null is as valid as anything else, so keep it.
        _ => single_type(schema).is_none() && !object.contains_key("properties"),
    }
}

fn repair_object(map: Map<String, Value>, schema: &Value, depth: usize) -> Value {
    let properties = schema.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = Map::new();
    for (key, value) in map {
        let property = properties.and_then(|props| props.get(&key));
        if value.is_null() && !required.contains(&key.as_str()) {
            // An optional field sent as null is the model's way of saying "not given".
            if property.is_none_or(|schema| !allows_null(schema)) {
                continue;
            }
        }
        let value = match property {
            Some(schema) => repair_value(value, schema, depth + 1),
            None => value,
        };
        out.insert(key, value);
    }
    Value::Object(out)
}

fn repair_value(value: Value, schema: &Value, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return value;
    }
    let Some(kind) = single_type(schema) else {
        return value;
    };
    match (kind, value) {
        ("object", Value::Object(map)) => repair_object(map, schema, depth),
        ("object", Value::String(text)) => match serde_json::from_str::<Value>(text.trim()) {
            Ok(Value::Object(map)) => repair_object(map, schema, depth),
            _ => Value::String(text),
        },
        ("array", Value::Array(items)) => repair_items(items, schema, depth),
        ("array", Value::String(text)) => match serde_json::from_str::<Value>(text.trim()) {
            Ok(Value::Array(items)) => repair_items(items, schema, depth),
            _ => repair_items(vec![Value::String(text)], schema, depth),
        },
        ("array", Value::Null) => Value::Null,
        ("array", single) => repair_items(vec![single], schema, depth),
        ("integer", Value::String(text)) => match text.trim().parse::<i64>() {
            Ok(number) => Value::Number(number.into()),
            Err(_) => match text.trim().parse::<u64>() {
                Ok(number) => Value::Number(number.into()),
                Err(_) => Value::String(text),
            },
        },
        ("number", Value::String(text)) => parse_number(&text).unwrap_or(Value::String(text)),
        ("boolean", Value::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(text),
        },
        (_, other) => other,
    }
}

fn repair_items(items: Vec<Value>, schema: &Value, depth: usize) -> Value {
    match schema.get("items").filter(|items| items.is_object()) {
        Some(item_schema) => Value::Array(
            items
                .into_iter()
                .map(|item| repair_value(item, item_schema, depth + 1))
                .collect(),
        ),
        None => Value::Array(items),
    }
}

fn parse_number(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(integer) = trimmed.parse::<i64>() {
        return Some(Value::Number(integer.into()));
    }
    let float: f64 = trimmed.parse().ok()?;
    // `parse` accepts "inf" and "NaN"; JSON numbers are finite.
    Number::from_f64(float).map(Value::Number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer"},
                "ratio": {"type": "number"},
                "recursive": {"type": "boolean"},
                "paths": {"type": "array", "items": {"type": "string"}},
                "limits": {"type": "array", "items": {"type": "integer"}},
                "options": {
                    "type": "object",
                    "properties": {"depth": {"type": "integer"}, "hidden": {"type": ["boolean", "null"]}}
                },
                "either": {"anyOf": [{"type": "integer"}, {"type": "string"}]},
                "maybe": {"type": ["integer", "string"]},
                "nullable": {"type": ["string", "null"]}
            },
            "required": ["path", "offset"]
        })
    }

    #[test]
    fn repair_table() {
        let cases: Vec<(&str, Value, Value)> = vec![
            (
                "double-encoded string",
                json!("{\"path\": \"a.txt\", \"offset\": 3}"),
                json!({"path": "a.txt", "offset": 3}),
            ),
            (
                "doubly double-encoded",
                Value::String(json!("{\"path\":\"a\",\"offset\":1}").to_string()),
                json!({"path": "a", "offset": 1}),
            ),
            ("null arguments", Value::Null, json!({})),
            ("empty string", json!(""), json!({})),
            (
                "numeric and boolean strings",
                json!({"path": "a", "offset": "5", "ratio": "0.5", "recursive": "true"}),
                json!({"path": "a", "offset": 5, "ratio": 0.5, "recursive": true}),
            ),
            (
                "capitalised boolean",
                json!({"path": "a", "offset": 1, "recursive": "False"}),
                json!({"path": "a", "offset": 1, "recursive": false}),
            ),
            (
                "optional null dropped, required null kept",
                json!({"path": null, "offset": 1, "ratio": null, "nullable": null}),
                json!({"path": null, "offset": 1, "nullable": null}),
            ),
            (
                "single value wrapped",
                json!({"path": "a", "offset": 1, "paths": "b.txt", "limits": "4"}),
                json!({"path": "a", "offset": 1, "paths": ["b.txt"], "limits": [4]}),
            ),
            (
                "stringified array and object",
                json!({"path": "a", "offset": 1, "paths": "[\"x\", \"y\"]", "options": "{\"depth\": \"2\"}"}),
                json!({"path": "a", "offset": 1, "paths": ["x", "y"], "options": {"depth": 2}}),
            ),
            (
                "nested optional null with null type kept",
                json!({"path": "a", "offset": 1, "options": {"hidden": null, "depth": null}}),
                json!({"path": "a", "offset": 1, "options": {"hidden": null}}),
            ),
            (
                "ambiguous positions untouched",
                json!({"path": "a", "offset": 1, "either": "5", "maybe": "5"}),
                json!({"path": "a", "offset": 1, "either": "5", "maybe": "5"}),
            ),
            (
                "non-numeric strings untouched",
                json!({"path": "a", "offset": "five", "ratio": "NaN", "recursive": "yes"}),
                json!({"path": "a", "offset": "five", "ratio": "NaN", "recursive": "yes"}),
            ),
            (
                "fractional integer untouched",
                json!({"path": "a", "offset": "5.5"}),
                json!({"path": "a", "offset": "5.5"}),
            ),
            (
                "numbers are not turned into strings",
                json!({"path": 5, "offset": 1}),
                json!({"path": 5, "offset": 1}),
            ),
            (
                "unknown keys kept",
                json!({"path": "a", "offset": 1, "extra": "x"}),
                json!({"path": "a", "offset": 1, "extra": "x"}),
            ),
            (
                "not an object stays as is",
                json!("plain words"),
                json!("plain words"),
            ),
            ("array arguments stay as is", json!([1, 2]), json!([1, 2])),
        ];
        for (name, input, expected) in cases {
            assert_eq!(repair_arguments(input, &schema()), expected, "{name}");
        }
    }

    #[test]
    fn depth_is_bounded() {
        let mut schema = json!({"type": "integer"});
        let mut value = json!("7");
        for _ in 0..(MAX_DEPTH + 4) {
            schema = json!({"type": "object", "properties": {"x": schema}});
            value = json!({ "x": value });
        }
        // Too deep to repair: the innermost string survives, and nothing panics.
        let repaired = repair_arguments(value.clone(), &schema);
        assert_eq!(repaired, value);
    }

    #[test]
    fn schema_without_type_but_with_properties_is_an_object() {
        let schema = json!({"properties": {"n": {"type": "integer"}}});
        assert_eq!(
            repair_arguments(json!({"n": "3"}), &schema),
            json!({"n": 3})
        );
    }
}
