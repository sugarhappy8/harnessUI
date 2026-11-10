//! Tool input schemas (ARCHITECTURE.md 8.3 steps 1 and 5, 9.1).
//!
//! Schemas from MCP servers are untrusted input. Each tool's schema is compiled once,
//! when the tool is registered, with a validator that cannot fetch anything (jsonschema is
//! built without network or file retrieval and runs offline). Before compiling, a schema
//! is refused if it refers to any external document (a `$ref` that does not start with
//! `#`), is nested too deeply or is too large. `pattern` matching uses a backtracking
//! limit so a hostile regular expression cannot stall validation.
//!
//! llama-server turns each offered schema into a grammar, and one schema it cannot
//! convert fails the whole request. [`simplify_for_llama`] therefore produces the copy
//! that is sent to the model; arguments are validated against the full schema here.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use jsonschema::error::{TypeKind, ValidationErrorKind};
use serde_json::{Map, Value, json};

/// Deepest JSON nesting (objects and arrays) accepted in a tool schema: about 32 levels
/// of nested subschemas.
pub const MAX_SCHEMA_DEPTH: usize = 64;

/// Largest serialised tool schema accepted.
pub const MAX_SCHEMA_BYTES: usize = 256 * 1024;

/// Deepest subschema nesting kept in the copy sent to llama-server.
pub const LLAMA_MAX_DEPTH: usize = 12;

/// Largest serialised copy sent to llama-server. A larger copy is cut back to the
/// top-level fields (see [`simplify_for_llama`]).
pub const LLAMA_MAX_BYTES: usize = 64 * 1024;

/// Most `$ref` expansions while inlining one schema for llama-server.
const MAX_INLINES: usize = 256;

/// Bytes of referenced schemas copied in by inlining before later references are left
/// unconstrained, so a large definition referenced many times cannot multiply the copy.
const MAX_INLINED_BYTES: usize = 4 * LLAMA_MAX_BYTES;

/// Largest `minLength`, `minItems` or `minProperties` kept in the llama-server copy.
/// llama.cpp expands a minimum count into a grammar repetition and refuses the grammar
/// past a fixed limit (2000 in b12110), which fails the whole request.
const LLAMA_MAX_MIN_COUNT: u64 = 1000;

/// Largest magnitude of a numeric bound kept in the llama-server copy (2^53). llama.cpp
/// cannot build its integer-range grammar for bounds such as `1e20`.
const LLAMA_MAX_BOUND: f64 = 9_007_199_254_740_992.0;

/// Longest `description` or `title` kept in the llama-server copy, in characters.
const LLAMA_MAX_TEXT_CHARS: usize = 1000;

/// The type names JSON Schema defines; llama.cpp refuses any other.
const JSON_TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "object", "array", "null",
];

/// Most messages [`CompiledSchema::validate`] returns (a summary line is added after).
const MAX_MESSAGES: usize = 8;

/// Errors counted before validation stops looking for more.
const MAX_ERRORS_SCANNED: usize = 1000;

/// Longest single validation message.
const MAX_MESSAGE_CHARS: usize = 240;

/// Backtracking steps allowed per `pattern` match.
const REGEX_BACKTRACK_LIMIT: usize = 100_000;

/// Keywords that never carry a subschema; their values are copied as data and never
/// searched for references.
const DATA_KEYWORDS: &[&str] = &["enum", "const", "default", "examples"];

/// Keywords whose value is one subschema (or, for `items`, possibly a list of them).
const SCHEMA_KEYWORDS: &[&str] = &[
    "items",
    "additionalItems",
    "additionalProperties",
    "unevaluatedItems",
    "unevaluatedProperties",
    "contains",
    "not",
    "if",
    "then",
    "else",
    "propertyNames",
];

/// Keywords whose value is a list of subschemas.
const SCHEMA_LIST_KEYWORDS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];

/// Keywords whose value maps names to subschemas.
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "dependentSchemas",
    "$defs",
    "definitions",
];

/// Keywords removed from the llama-server copy: llama.cpp cannot convert some regular
/// expressions and formats, references and identifiers are resolved here instead, and
/// the rest constrain nothing llama.cpp can express.
const LLAMA_DROPPED: &[&str] = &[
    "pattern",
    "patternProperties",
    "propertyNames",
    "format",
    "dependencies",
    "dependentRequired",
    "dependentSchemas",
    "unevaluatedItems",
    "unevaluatedProperties",
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$dynamicRef",
    "$recursiveRef",
    "$recursiveAnchor",
    "$defs",
    "definitions",
    "$comment",
    "$vocabulary",
    "contentEncoding",
    "contentMediaType",
    "contentSchema",
];

/// Why a tool schema was refused. The tool is registered as disabled with this reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    /// The schema is not a JSON object.
    #[error("the input schema is not a JSON object")]
    NotAnObject,
    /// Larger than [`MAX_SCHEMA_BYTES`].
    #[error("the input schema is larger than {} KiB", MAX_SCHEMA_BYTES / 1024)]
    TooLarge,
    /// Nested deeper than [`MAX_SCHEMA_DEPTH`].
    #[error("the input schema is nested more than {MAX_SCHEMA_DEPTH} levels deep")]
    TooDeep,
    /// A reference to another document, which is never fetched.
    #[error(
        "the input schema refers to an external document ({0}); only references inside the schema are allowed"
    )]
    ExternalRef(String),
    /// The validator could not compile it.
    #[error("the input schema is invalid: {0}")]
    Invalid(String),
}

/// A tool schema compiled once for validation, plus its llama-server copy.
#[derive(Clone)]
pub struct CompiledSchema {
    validator: Arc<jsonschema::Validator>,
    llama: Arc<Value>,
}

impl fmt::Debug for CompiledSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledSchema").finish_non_exhaustive()
    }
}

impl CompiledSchema {
    /// Checks and compiles `schema`. See [`SchemaError`] for what is refused.
    pub fn compile(schema: &Value) -> Result<Self, SchemaError> {
        check(schema)?;
        let validator = jsonschema::options()
            .offline()
            .should_validate_formats(false)
            .with_pattern_options(
                jsonschema::PatternOptions::fancy_regex().backtrack_limit(REGEX_BACKTRACK_LIMIT),
            )
            .build(schema)
            .map_err(|e| SchemaError::Invalid(policy::sanitize::one_line(&e.to_string(), 200)))?;
        Ok(Self {
            validator: Arc::new(validator),
            llama: Arc::new(simplify_for_llama(schema)),
        })
    }

    /// Validates tool arguments. On failure, returns short messages that name each bad
    /// field and what was expected, e.g. `count: expected integer, got string`.
    pub fn validate(&self, args: &Value) -> Result<(), Vec<String>> {
        let mut messages = Vec::new();
        let mut total = 0usize;
        for error in self.validator.iter_errors(args) {
            total += 1;
            if messages.len() < MAX_MESSAGES {
                messages.push(describe_error(&error));
            }
            if total >= MAX_ERRORS_SCANNED {
                break;
            }
        }
        if total > MAX_MESSAGES {
            messages.push(format!("…and {} more problems", total - MAX_MESSAGES));
        }
        if messages.is_empty() {
            Ok(())
        } else {
            Err(messages)
        }
    }

    /// The simplified copy offered to llama-server ([`simplify_for_llama`]).
    pub fn llama(&self) -> &Value {
        &self.llama
    }
}

/// Size, depth and external-reference checks done before compiling.
fn check(schema: &Value) -> Result<(), SchemaError> {
    if !schema.is_object() {
        return Err(SchemaError::NotAnObject);
    }
    if json_depth(schema, 0) > MAX_SCHEMA_DEPTH {
        return Err(SchemaError::TooDeep);
    }
    let size = serde_json::to_vec(schema)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    if size > MAX_SCHEMA_BYTES {
        return Err(SchemaError::TooLarge);
    }
    if let Some(reference) = find_external_ref(schema) {
        return Err(SchemaError::ExternalRef(policy::sanitize::one_line(
            &reference, 120,
        )));
    }
    Ok(())
}

/// Nesting depth of objects and arrays, counting stops once past the limit.
fn json_depth(value: &Value, depth: usize) -> usize {
    if depth > MAX_SCHEMA_DEPTH {
        return depth;
    }
    match value {
        Value::Object(map) => map
            .values()
            .map(|v| json_depth(v, depth + 1))
            .max()
            .unwrap_or(depth + 1),
        Value::Array(items) => items
            .iter()
            .map(|v| json_depth(v, depth + 1))
            .max()
            .unwrap_or(depth + 1),
        _ => depth,
    }
}

/// The first reference that leaves the document: a `$ref`, `$dynamicRef` or
/// `$recursiveRef` whose value does not start with `#`.
///
/// `value` is read as a schema. Values of data keywords (`enum`, `const`, `default`,
/// `examples`) are not searched, and in maps of subschemas (`properties`, `$defs`,
/// `dependencies`…) the keys are names, not keywords: a property named `enum` is still
/// searched.
fn find_external_ref(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                let keyword = key.as_str();
                if matches!(keyword, "$ref" | "$dynamicRef" | "$recursiveRef")
                    && let Value::String(target) = v
                {
                    if !target.starts_with('#') {
                        return Some(target.clone());
                    }
                    continue;
                }
                if DATA_KEYWORDS.contains(&keyword) {
                    continue;
                }
                let found = match v {
                    Value::Object(subschemas)
                        if SCHEMA_MAP_KEYWORDS.contains(&keyword) || keyword == "dependencies" =>
                    {
                        subschemas.values().find_map(find_external_ref)
                    }
                    other => find_external_ref(other),
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(find_external_ref),
        _ => None,
    }
}

fn describe_error(error: &jsonschema::ValidationError<'_>) -> String {
    let path = field_path(error.instance_path().as_str());
    let message = match error.kind() {
        ValidationErrorKind::Required { property } => {
            let name = property
                .as_str()
                .map_or_else(|| property.to_string(), str::to_owned);
            format!("{path}: missing required field \"{name}\"")
        }
        ValidationErrorKind::Type { kind } => {
            let expected = match kind {
                TypeKind::Single(t) => t.to_string(),
                TypeKind::Multiple(set) => set
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(" or "),
            };
            format!(
                "{path}: expected {expected}, got {}",
                json_type(error.instance())
            )
        }
        ValidationErrorKind::AdditionalProperties { unexpected }
        | ValidationErrorKind::UnevaluatedProperties { unexpected } => {
            let list = unexpected
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{path}: unexpected field(s) {list}")
        }
        _ => format!("{path}: {}", error.masked()),
    };
    policy::sanitize::one_line(&message, MAX_MESSAGE_CHARS)
}

/// `/a/0/b` → `a[0].b`; the root is called `arguments`.
fn field_path(pointer: &str) -> String {
    if pointer.is_empty() {
        return "arguments".into();
    }
    let mut out = String::new();
    for segment in pointer.trim_start_matches('/').split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        if !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()) {
            out.push_str(&format!("[{segment}]"));
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(&segment);
        }
    }
    out
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The copy of a tool schema sent to llama-server (8.3 step 1). One schema llama-server
/// cannot turn into a grammar fails every request it is offered in, so the copy keeps
/// only what llama.cpp is known to convert:
///
/// - local references (`#/$defs/...`, `#/definitions/...`, any `#/...` pointer) are
///   inlined, with recursion, count and copied bytes bounded; remote, recursive and
///   unresolvable references become unconstrained;
/// - `pattern`, `format` and keywords llama.cpp cannot express are dropped, and nesting
///   is cut at [`LLAMA_MAX_DEPTH`];
/// - boolean subschemas become `{}` (except `additionalProperties: true/false`);
/// - values llama.cpp b12110 was seen to refuse are dropped: empty `enum`, `anyOf`,
///   `oneOf`, `allOf` or `type` lists, unknown type names, counts that are not
///   non-negative integers, a minimum count above 1000, a minimum count above the maximum
///   (with `minLength` it can exhaust llama-server's memory), numeric bounds beyond 2^53;
/// - `description` and `title` texts have invisible characters escaped and are capped;
/// - the top level is always an object schema;
/// - a copy larger than [`LLAMA_MAX_BYTES`] is cut back to the top-level fields with their
///   types and descriptions, and if that is still too large, to an empty object schema.
///
/// Everything dropped here is still enforced: arguments are validated against the full
/// schema before a call.
pub fn simplify_for_llama(schema: &Value) -> Value {
    let mut simplifier = Simplifier {
        root: schema,
        budget: MAX_INLINES,
        bytes_left: MAX_INLINED_BYTES,
        sizes: HashMap::new(),
        stack: Vec::new(),
    };
    let full = object_root(simplifier.node(schema, 0));
    if fits_llama(&full) {
        return full;
    }
    let top = object_root(top_level_only(schema));
    if fits_llama(&top) {
        return top;
    }
    json!({"type": "object", "properties": {}})
}

fn fits_llama(value: &Value) -> bool {
    serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= LLAMA_MAX_BYTES)
}

/// Tool arguments are always an object, and llama-server wants an object schema at the
/// top with a `properties` map.
fn object_root(value: Value) -> Value {
    let Value::Object(mut map) = value else {
        return json!({"type": "object", "properties": {}});
    };
    map.insert("type".into(), json!("object"));
    if !map.get("properties").is_some_and(Value::is_object) {
        map.insert("properties".into(), json!({}));
    }
    Value::Object(map)
}

/// The top-level fields with their types and descriptions only, and the required list:
/// the fallback for a schema whose simplified copy is too large.
fn top_level_only(schema: &Value) -> Value {
    let mut properties = Map::new();
    if let Some(Value::Object(fields)) = schema.get("properties") {
        for (name, field) in fields {
            let kept = match field {
                Value::Object(map) => shallow(map),
                _ => Map::new(),
            };
            properties.insert(name.clone(), Value::Object(kept));
        }
    }
    let mut out = Map::new();
    out.insert("properties".into(), Value::Object(properties));
    if let Some(required) = schema.get("required") {
        out.insert("required".into(), required.clone());
    }
    repair(&mut out);
    Value::Object(out)
}

struct Simplifier<'a> {
    root: &'a Value,
    /// `$ref` expansions left.
    budget: usize,
    /// Bytes of referenced schemas that may still be copied in.
    bytes_left: usize,
    /// Serialised size of each reference target seen, so each is measured once.
    sizes: HashMap<String, usize>,
    /// References being expanded, to stop recursion.
    stack: Vec<String>,
}

impl Simplifier<'_> {
    fn node(&mut self, value: &Value, depth: usize) -> Value {
        // Boolean schemas (`true`, `false`) and anything malformed become unconstrained:
        // llama.cpp accepts only object schemas in these places.
        let Value::Object(map) = value else {
            return json!({});
        };
        if depth >= LLAMA_MAX_DEPTH {
            return Value::Object(shallow(map));
        }
        if let Some(Value::String(reference)) = map.get("$ref") {
            let reference = reference.clone();
            return self.inline(&reference, map, depth);
        }
        let mut out = Map::new();
        for (key, v) in map {
            let k = key.as_str();
            if LLAMA_DROPPED.contains(&k) || k == "$ref" {
                continue;
            }
            let simplified = if k == "additionalProperties" && v.is_boolean() {
                v.clone()
            } else if matches!(k, "description" | "title") {
                match v {
                    Value::String(text) => Value::String(llama_text(text)),
                    _ => continue,
                }
            } else if SCHEMA_MAP_KEYWORDS.contains(&k) {
                match v {
                    Value::Object(props) => Value::Object(
                        props
                            .iter()
                            .map(|(name, sub)| (name.clone(), self.node(sub, depth + 1)))
                            .collect(),
                    ),
                    _ => continue,
                }
            } else if SCHEMA_LIST_KEYWORDS.contains(&k) {
                match v {
                    Value::Array(items) => {
                        Value::Array(items.iter().map(|sub| self.node(sub, depth + 1)).collect())
                    }
                    _ => continue,
                }
            } else if SCHEMA_KEYWORDS.contains(&k) {
                match v {
                    Value::Array(items) => {
                        Value::Array(items.iter().map(|sub| self.node(sub, depth + 1)).collect())
                    }
                    other => self.node(other, depth + 1),
                }
            } else {
                v.clone()
            };
            out.insert(key.clone(), simplified);
        }
        repair(&mut out);
        Value::Object(out)
    }

    /// Replaces `{"$ref": r, ...siblings}` with the referenced schema, siblings on top.
    fn inline(&mut self, reference: &str, map: &Map<String, Value>, depth: usize) -> Value {
        let mut siblings = map.clone();
        siblings.remove("$ref");
        let siblings = self.node(&Value::Object(siblings), depth);
        let target = if reference.starts_with('#')
            && self.budget > 0
            && !self.stack.iter().any(|r| r == reference)
        {
            resolve_pointer(self.root, reference)
        } else {
            None
        };
        let Some(target) = target else {
            // Remote, recursive, over budget or not found: leave it unconstrained.
            return siblings;
        };
        self.budget -= 1;
        let size = *self
            .sizes
            .entry(reference.to_owned())
            .or_insert_with(|| serde_json::to_vec(target).map_or(usize::MAX, |v| v.len()));
        if size > self.bytes_left {
            // Copying it in again would make the copy too large: unconstrained.
            return siblings;
        }
        self.bytes_left -= size;
        self.stack.push(reference.to_owned());
        let inlined = self.node(target, depth + 1);
        self.stack.pop();
        match (inlined, siblings) {
            (Value::Object(mut base), Value::Object(extra)) => {
                for (k, v) in extra {
                    base.insert(k, v);
                }
                // The siblings may contradict the target (a `maxLength` below its
                // `minLength`).
                repair(&mut base);
                Value::Object(base)
            }
            (inlined, Value::Object(extra)) if extra.is_empty() => inlined,
            (_, siblings) => siblings,
        }
    }
}

/// Drops what llama.cpp refuses in an otherwise valid schema; each of these fails the
/// whole chat request (checked against llama-server b12110). See [`simplify_for_llama`].
fn repair(out: &mut Map<String, Value>) {
    for key in ["enum", "anyOf", "oneOf", "allOf", "prefixItems"] {
        if out
            .get(key)
            .is_some_and(|v| v.as_array().is_none_or(Vec::is_empty))
        {
            out.remove(key);
        }
    }
    if out
        .get("items")
        .is_some_and(|v| v.as_array().is_some_and(Vec::is_empty))
    {
        out.remove("items");
    }
    match out.get("type") {
        None => {}
        Some(Value::String(name)) if JSON_TYPES.contains(&name.as_str()) => {}
        Some(Value::Array(names)) => {
            let known: Vec<Value> = names
                .iter()
                .filter(|n| n.as_str().is_some_and(|n| JSON_TYPES.contains(&n)))
                .cloned()
                .collect();
            if known.is_empty() {
                out.remove("type");
            } else {
                out.insert("type".into(), Value::Array(known));
            }
        }
        Some(_) => {
            out.remove("type");
        }
    }
    for (min_key, max_key) in [
        ("minLength", "maxLength"),
        ("minItems", "maxItems"),
        ("minProperties", "maxProperties"),
    ] {
        if out
            .get(min_key)
            .is_some_and(|v| v.as_u64().is_none_or(|n| n > LLAMA_MAX_MIN_COUNT))
        {
            out.remove(min_key);
        }
        if out.get(max_key).is_some_and(|v| v.as_u64().is_none()) {
            out.remove(max_key);
        }
        if let (Some(low), Some(high)) = (
            out.get(min_key).and_then(Value::as_u64),
            out.get(max_key).and_then(Value::as_u64),
        ) && low > high
        {
            out.remove(min_key);
            out.remove(max_key);
        }
    }
    for key in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
        if out.get(key).is_some_and(|v| {
            v.as_f64()
                .is_none_or(|n| !n.is_finite() || n.abs() > LLAMA_MAX_BOUND)
        }) {
            out.remove(key);
        }
    }
    if let Some(required) = out.get("required") {
        let names: Vec<Value> = required
            .as_array()
            .map(|names| names.iter().filter(|n| n.is_string()).cloned().collect())
            .unwrap_or_default();
        if names.is_empty() {
            out.remove("required");
        } else {
            out.insert("required".into(), Value::Array(names));
        }
    }
}

/// A `description` or `title` for the model: invisible characters escaped (13.5), cut to
/// [`LLAMA_MAX_TEXT_CHARS`].
fn llama_text(text: &str) -> String {
    let visible = policy::sanitize::visible(text);
    if visible.chars().count() <= LLAMA_MAX_TEXT_CHARS {
        return visible;
    }
    let cut: String = visible.chars().take(LLAMA_MAX_TEXT_CHARS - 1).collect();
    format!("{cut}…")
}

/// What is kept of a schema below the depth limit: its type and description.
fn shallow(map: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(t) = map.get("type") {
        out.insert("type".into(), t.clone());
    }
    if let Some(Value::String(d)) = map.get("description") {
        out.insert("description".into(), Value::String(llama_text(d)));
    }
    repair(&mut out);
    out
}

/// Resolves a `#/...` JSON pointer (percent-encoded per RFC 3986) against `root`.
fn resolve_pointer<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let fragment = percent_decode(reference.strip_prefix('#')?)?;
    if fragment.is_empty() {
        return Some(root);
    }
    if !fragment.starts_with('/') {
        // A plain-name fragment ($anchor) is not followed.
        return None;
    }
    root.pointer(&fragment)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Canonical JSON: object keys sorted at every level, no whitespace. Used for grant
/// scopes (the exact arguments) and fingerprints, independent of how serde_json orders
/// maps in this build.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                if let Some(v) = map.get(*key) {
                    write_canonical(v, out);
                }
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// `value` with object keys sorted at every level, for pretty-printing in a fixed order.
pub fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                if let Some(v) = map.get(key) {
                    out.insert(key.clone(), sorted(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(depth: usize) -> Value {
        let mut schema = json!({"type": "string"});
        for _ in 0..depth {
            schema = json!({"type": "object", "properties": {"x": schema}});
        }
        schema
    }

    #[test]
    fn rejects_external_references() {
        for reference in [
            "https://example.com/schema.json",
            "file:///etc/passwd",
            "other.json#/defs/x",
            "/abs/path.json",
        ] {
            let schema = json!({
                "type": "object",
                "properties": {"a": {"$ref": reference}}
            });
            assert_eq!(
                CompiledSchema::compile(&schema).unwrap_err(),
                SchemaError::ExternalRef(reference.to_owned())
            );
        }
        let dynamic = json!({"type": "object", "$dynamicRef": "https://x/y"});
        assert!(matches!(
            CompiledSchema::compile(&dynamic),
            Err(SchemaError::ExternalRef(_))
        ));
        // Local references compile, and "$ref" inside data keywords is data.
        let local = json!({
            "type": "object",
            "$defs": {"name": {"type": "string"}},
            "properties": {
                "a": {"$ref": "#/$defs/name"},
                "b": {"enum": [{"$ref": "https://not-a-ref"}]},
                "c": {"default": {"$ref": "http://x"}}
            }
        });
        CompiledSchema::compile(&local).unwrap();
    }

    #[test]
    fn rejects_deep_large_and_non_object_schemas() {
        assert_eq!(
            CompiledSchema::compile(&nested(40)).unwrap_err(),
            SchemaError::TooDeep
        );
        CompiledSchema::compile(&nested(20)).unwrap();
        let mut props = Map::new();
        for i in 0..20_000 {
            props.insert(
                format!("field_{i}"),
                json!({"type": "string", "description": "padding"}),
            );
        }
        let big = json!({"type": "object", "properties": props});
        assert_eq!(
            CompiledSchema::compile(&big).unwrap_err(),
            SchemaError::TooLarge
        );
        assert_eq!(
            CompiledSchema::compile(&json!(true)).unwrap_err(),
            SchemaError::NotAnObject
        );
        assert!(matches!(
            CompiledSchema::compile(&json!({"type": 12})),
            Err(SchemaError::Invalid(_))
        ));
        // An unknown meta-schema would need fetching: refused, not fetched.
        assert!(
            CompiledSchema::compile(
                &json!({"$schema": "https://example.com/meta", "type": "object"})
            )
            .is_err()
        );
    }

    #[test]
    fn validation_messages_name_fields_and_expectations() {
        let schema = CompiledSchema::compile(&json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer"},
                "mode": {"enum": ["fast", "slow"]},
                "items": {"type": "array", "items": {"type": "string"}},
                "nested": {"type": "object", "properties": {"flag": {"type": "boolean"}}}
            },
            "required": ["count", "path"],
            "additionalProperties": false
        }))
        .unwrap();
        assert_eq!(
            schema.validate(&json!({"count": 1, "path": "x"})),
            Err(vec!["arguments: unexpected field(s) \"path\"".to_owned()])
        );
        let errors = schema
            .validate(&json!({
                "count": "abc",
                "mode": "medium",
                "items": ["a", 2],
                "nested": {"flag": "yes"},
                "extra": 1
            }))
            .unwrap_err();
        let joined = errors.join("\n");
        assert!(
            joined.contains("count: expected integer, got string"),
            "{joined}"
        );
        assert!(
            joined.contains("missing required field \"path\""),
            "{joined}"
        );
        assert!(
            joined.contains("items[1]: expected string, got integer"),
            "{joined}"
        );
        assert!(
            joined.contains("nested.flag: expected boolean, got string"),
            "{joined}"
        );
        assert!(joined.contains("mode:"), "{joined}");
        assert!(joined.contains("\"extra\""), "{joined}");
        // Instance values are masked in generic messages.
        assert!(!joined.contains("medium"), "{joined}");
        let valid = json!({"count": 3, "path": "p"});
        let ok_schema = CompiledSchema::compile(&json!({
            "type": "object",
            "properties": {"count": {"type": "integer"}, "path": {"type": "string"}},
            "required": ["count", "path"]
        }))
        .unwrap();
        assert_eq!(ok_schema.validate(&valid), Ok(()));
    }

    #[test]
    fn many_errors_are_summarised() {
        let schema = CompiledSchema::compile(&json!({
            "type": "object",
            "additionalProperties": {"type": "integer"}
        }))
        .unwrap();
        let mut args = Map::new();
        for i in 0..20 {
            args.insert(format!("k{i:02}"), json!("x"));
        }
        let errors = schema.validate(&Value::Object(args)).unwrap_err();
        assert_eq!(errors.len(), MAX_MESSAGES + 1);
        assert_eq!(errors.last().unwrap(), "…and 12 more problems");
    }

    #[test]
    fn hostile_patterns_cannot_stall_validation() {
        let schema = CompiledSchema::compile(&json!({
            "type": "object",
            "properties": {"s": {"type": "string", "pattern": "^(a+)+$"}}
        }))
        .unwrap();
        let start = std::time::Instant::now();
        let input = format!("{}b", "a".repeat(40));
        let _ = schema.validate(&json!({"s": input}));
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn simplify_inlines_local_definitions_and_drops_risky_keywords() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "definitions": {"path": {"type": "string", "pattern": "^/", "format": "uri"}},
            "$defs": {"mode": {"enum": ["a", "b"]}},
            "properties": {
                "p": {"$ref": "#/definitions/path", "description": "where"},
                "m": {"$ref": "#/$defs/mode"},
                "r": {"$ref": "https://example.com/remote.json", "description": "remote"},
                "q": {"$ref": "#/definitions/missing"},
                "list": {"type": "array", "items": {"$ref": "#/definitions/path"}},
                "pp": {"type": "object", "patternProperties": {"^x": {"type": "string"}}}
            },
            "required": ["p"]
        });
        let simple = simplify_for_llama(&schema);
        assert_eq!(
            simple,
            json!({
                "type": "object",
                "properties": {
                    "p": {"type": "string", "description": "where"},
                    "m": {"enum": ["a", "b"]},
                    "r": {"description": "remote"},
                    "q": {},
                    "list": {"type": "array", "items": {"type": "string"}},
                    "pp": {"type": "object"}
                },
                "required": ["p"]
            })
        );
    }

    #[test]
    fn simplify_bounds_recursion_and_depth() {
        let recursive = json!({
            "type": "object",
            "$defs": {"node": {
                "type": "object",
                "properties": {"children": {"type": "array", "items": {"$ref": "#/$defs/node"}}}
            }},
            "properties": {"root": {"$ref": "#/$defs/node"}}
        });
        let simple = simplify_for_llama(&recursive);
        // One level is inlined; the recursive reference becomes unconstrained.
        assert_eq!(
            simple["properties"]["root"]["properties"]["children"]["items"],
            json!({})
        );
        let deep = simplify_for_llama(&nested(30));
        assert!(
            json_depth(&deep, 0) <= 2 * LLAMA_MAX_DEPTH + 2,
            "{}",
            json_depth(&deep, 0)
        );
        // A self-reference to the root expands once.
        let tree = json!({"type": "object", "properties": {"sub": {"$ref": "#"}}});
        let simple = simplify_for_llama(&tree);
        assert_eq!(simple["properties"]["sub"]["properties"]["sub"], json!({}));
        // Not an object schema at the top: replaced by an empty object schema.
        assert_eq!(
            simplify_for_llama(&json!(1)),
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            simplify_for_llama(&json!({})),
            json!({"type": "object", "properties": {}})
        );
    }

    #[test]
    fn simplify_handles_percent_encoded_pointers() {
        let schema = json!({
            "type": "object",
            "$defs": {"a b": {"type": "integer"}, "c/d": {"type": "boolean"}},
            "properties": {
                "x": {"$ref": "#/$defs/a%20b"},
                "y": {"$ref": "#/$defs/c~1d"}
            }
        });
        let simple = simplify_for_llama(&schema);
        assert_eq!(simple["properties"]["x"], json!({"type": "integer"}));
        assert_eq!(simple["properties"]["y"], json!({"type": "boolean"}));
    }

    #[test]
    fn property_names_never_hide_an_external_reference() {
        // A property named like a data keyword is still a schema and is searched.
        for name in ["enum", "const", "default", "examples"] {
            let schema = json!({
                "type": "object",
                "properties": {name: {"$ref": "https://evil.example/s.json"}}
            });
            assert_eq!(
                CompiledSchema::compile(&schema).unwrap_err(),
                SchemaError::ExternalRef("https://evil.example/s.json".into()),
                "{name}"
            );
        }
        let in_dependencies = json!({
            "type": "object",
            "dependencies": {"enum": {"$ref": "file:///etc/x.json"}}
        });
        assert!(matches!(
            CompiledSchema::compile(&in_dependencies),
            Err(SchemaError::ExternalRef(_))
        ));
        // A property named "$ref" is a field, not a reference.
        let field = json!({"type": "object", "properties": {"$ref": {"type": "string"}}});
        CompiledSchema::compile(&field).unwrap();
    }

    /// Each case failed a whole chat request on llama-server b12110 before it was
    /// simplified (see `tests/llama_schemas.rs` for the check against the real server).
    #[test]
    fn simplify_removes_what_llama_server_refuses() {
        let p = |field: Value| {
            simplify_for_llama(&json!({"type": "object", "properties": {"f": field}}))["properties"]
                ["f"]
                .clone()
        };
        // Boolean subschemas, except additionalProperties.
        assert_eq!(p(json!(true)), json!({}));
        assert_eq!(p(json!(false)), json!({}));
        assert_eq!(
            p(json!({"type": "array", "items": true})),
            json!({"type": "array", "items": {}})
        );
        assert_eq!(
            p(json!({"oneOf": [true, {"type": "string"}]})),
            json!({"oneOf": [{}, {"type": "string"}]})
        );
        assert_eq!(
            p(json!({"type": "object", "additionalProperties": false})),
            json!({"type": "object", "additionalProperties": false})
        );
        // Empty lists and unknown types.
        assert_eq!(p(json!({"enum": []})), json!({}));
        assert_eq!(p(json!({"anyOf": []})), json!({}));
        assert_eq!(p(json!({"type": []})), json!({}));
        assert_eq!(p(json!({"type": "foo"})), json!({}));
        assert_eq!(
            p(json!({"type": ["string", "foo", "null"]})),
            json!({"type": ["string", "null"]})
        );
        // Counts: large minimums, minimum above maximum, non-integers.
        assert_eq!(
            p(json!({"type": "string", "minLength": 5000, "maxLength": 9000})),
            json!({"type": "string", "maxLength": 9000})
        );
        assert_eq!(
            p(json!({"type": "string", "minLength": 3, "maxLength": 2})),
            json!({"type": "string"})
        );
        assert_eq!(
            p(json!({"type": "array", "minItems": -1, "maxItems": 2.5})),
            json!({"type": "array"})
        );
        assert_eq!(
            p(json!({"type": "string", "minLength": 2, "maxLength": 1_000_000_000})),
            json!({"type": "string", "minLength": 2, "maxLength": 1_000_000_000})
        );
        // Numeric bounds llama.cpp cannot represent.
        assert_eq!(
            p(json!({"type": "integer", "minimum": 5, "maximum": 1e20})),
            json!({"type": "integer", "minimum": 5})
        );
        assert_eq!(
            p(json!({"type": "integer", "exclusiveMinimum": true})),
            json!({"type": "integer"})
        );
        // A malformed required list.
        assert_eq!(
            simplify_for_llama(&json!({"type": "object", "required": [1, "a"]}))["required"],
            json!(["a"])
        );
        // Unknown references in any form are gone.
        assert_eq!(p(json!({"$ref": "#/$defs/missing"})), json!({}));
        assert_eq!(p(json!({"$ref": "#anchor"})), json!({}));
        assert_eq!(
            p(json!({"type": "object", "dependencies": {"x": {"$ref": "#/nope"}}})),
            json!({"type": "object"})
        );
    }

    #[test]
    fn simplify_repairs_a_reference_contradicted_by_its_siblings() {
        let schema = json!({
            "type": "object",
            "$defs": {"name": {"type": "string", "minLength": 5}},
            "properties": {"n": {"$ref": "#/$defs/name", "maxLength": 2}}
        });
        assert_eq!(
            simplify_for_llama(&schema)["properties"]["n"],
            json!({"type": "string"})
        );
    }

    #[test]
    fn the_top_level_is_always_an_object_schema() {
        assert_eq!(
            simplify_for_llama(&json!({"type": "string", "description": "odd"})),
            json!({"type": "object", "description": "odd", "properties": {}})
        );
        assert_eq!(
            simplify_for_llama(&json!({"type": "object", "properties": []})),
            json!({"type": "object", "properties": {}})
        );
    }

    #[test]
    fn descriptions_are_made_visible_and_capped() {
        let schema = json!({
            "type": "object",
            "description": "hidden\u{202E}text",
            "properties": {"a": {"type": "string", "description": "x".repeat(5000)}}
        });
        let simple = simplify_for_llama(&schema);
        assert_eq!(simple["description"], json!("hidden\\u{202E}text"));
        let long = simple["properties"]["a"]["description"].as_str().unwrap();
        assert_eq!(long.chars().count(), LLAMA_MAX_TEXT_CHARS);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn the_copy_stays_small() {
        // A large definition referenced from many fields: inlining stops once the copied
        // bytes run out, and the result fits.
        let mut big = Map::new();
        for i in 0..400 {
            big.insert(
                format!("k{i}"),
                json!({"type": "string", "description": "padding padding"}),
            );
        }
        let mut fields = Map::new();
        for i in 0..200 {
            fields.insert(format!("f{i}"), json!({"$ref": "#/$defs/big"}));
        }
        let schema = json!({
            "type": "object",
            "$defs": {"big": {"type": "object", "properties": big}},
            "properties": fields
        });
        let simple = simplify_for_llama(&schema);
        assert!(fits_llama(&simple));
        assert_eq!(simple["properties"].as_object().unwrap().len(), 200);
        // Too many top-level fields even when cut back: an empty object schema.
        let mut huge = Map::new();
        for i in 0..5000 {
            huge.insert(
                format!("field_{i}"),
                json!({"type": "string", "description": "d".repeat(40)}),
            );
        }
        let simple = simplify_for_llama(&json!({"type": "object", "properties": huge}));
        assert_eq!(simple, json!({"type": "object", "properties": {}}));
        // In between: the top-level fields with their types.
        let mut many = Map::new();
        for i in 0..300 {
            many.insert(
                format!("field_{i}"),
                json!({"type": "object", "properties": {"x": {"type": "string", "description": "y".repeat(300)}}}),
            );
        }
        let simple = simplify_for_llama(
            &json!({"type": "object", "properties": many, "required": ["field_1"]}),
        );
        assert_eq!(simple["properties"]["field_7"], json!({"type": "object"}));
        assert_eq!(simple["required"], json!(["field_1"]));
        assert!(fits_llama(&simple));
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let value = json!({"b": 1, "a": {"d": [3, {"z": 1, "y": 2}], "c": "x"}});
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":"x","d":[3,{"y":2,"z":1}]},"b":1}"#
        );
        assert_eq!(canonical_json(&json!("a\"b")), r#""a\"b""#);
        assert_eq!(
            serde_json::to_string(&sorted(&value)).unwrap(),
            canonical_json(&value)
        );
    }

    #[test]
    fn field_paths_read_naturally() {
        assert_eq!(field_path(""), "arguments");
        assert_eq!(field_path("/a/0/b"), "a[0].b");
        assert_eq!(field_path("/a~1b"), "a/b");
    }
}
