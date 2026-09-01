//! Flyte's type system, in the two directions this adapter needs it.
//!
//! Outwards: a form's JSON values become a `LiteralMap` bound to the types the
//! launch plan itself declares. Inwards: a declared interface becomes
//! [`EngineParameter`]s a form can render, and a default becomes something a
//! field can be pre-filled with.
//!
//! ## Why the types come from the engine and not from the caller
//!
//! A caller sends `{"since": "2026-08-01T00:00:00Z"}` — a JSON string, with no
//! indication of whether the workflow wants a timestamp, a string, or an
//! optional one. Binding therefore reads the launch plan's own
//! `expected_inputs` *at launch time* and encodes against that. Two things
//! follow, and both are deliberate: a panel rendering a stale interface fails
//! with a named error instead of sending a mistyped literal, and an input the
//! entity does not declare is refused rather than dropped. An orchestrator
//! that silently ignores an unknown field turns a typo in a filter into a run
//! over everything.
//!
//! ## Why the JSON is hand-built rather than generated from the protos
//!
//! `flyteidl` is a large protobuf surface and this adapter uses six messages
//! of it. Pulling in `prost`, `tonic` and the IDL crate to reach `/api/v1/…`
//! over the gateway that already speaks JSON would put a code generator and a
//! gRPC stack in the build for types that are a hundred lines by hand — and
//! the gateway is a stable, documented contract. The cost is that field names
//! live here as strings, which is what [`field`] and the round-trip tests are
//! for.

use std::collections::BTreeMap;

use aiwatcher_core::engine::{EngineParameter, LaunchError, ParameterKind};
use serde_json::{Map, Value, json};

/// Read a protobuf field by its `snake_case` name, accepting `camelCase` too.
///
/// grpc-gateway's JSON is protobuf JSON, where a field has both spellings: the
/// original name and the lowerCamelCase one, and which appears on the wire
/// depends on how the gateway was built. Reading only one spelling is the bug
/// that shows up as an interface with no parameters against somebody else's
/// deployment.
pub(crate) fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    if let Some(found) = value.get(name) {
        return Some(found);
    }
    if !name.contains('_') {
        return None;
    }
    let mut camel = String::with_capacity(name.len());
    let mut upper = false;
    for character in name.chars() {
        if character == '_' {
            upper = true;
        } else if upper {
            camel.extend(character.to_uppercase());
            upper = false;
        } else {
            camel.push(character);
        }
    }
    value.get(camel)
}

/// A string field, or empty.
pub(crate) fn text(value: &Value, name: &str) -> String {
    field(value, name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// One declared input: its type, and whether the caller has to supply it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParameterSpec {
    /// The `LiteralType` as the engine stated it. Kept whole rather than
    /// reduced to a [`ParameterKind`], because encoding needs the parts the
    /// kind throws away — a collection's element type, a union's variants.
    pub(crate) literal_type: Value,
    pub(crate) required: bool,
    pub(crate) default: Option<Value>,
    pub(crate) description: String,
}

/// The declared inputs of one launchable entity.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Interface {
    parameters: BTreeMap<String, ParameterSpec>,
}

impl Interface {
    /// Read `closure.expected_inputs` — a `ParameterMap`.
    pub(crate) fn read(expected_inputs: Option<&Value>) -> Self {
        let Some(map) = expected_inputs
            .and_then(|inputs| field(inputs, "parameters"))
            .and_then(Value::as_object)
        else {
            return Self::default();
        };
        let parameters = map
            .iter()
            .map(|(name, parameter)| {
                let variable = field(parameter, "var");
                let literal_type = variable
                    .and_then(|variable| field(variable, "type"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let default = field(parameter, "default").cloned();
                (
                    name.clone(),
                    ParameterSpec {
                        literal_type,
                        // `Parameter` is a oneof: a default or a required
                        // flag, never both. A parameter with a default is
                        // optional however the flag reads.
                        required: default.is_none()
                            && field(parameter, "required")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        default,
                        description: variable
                            .map(|variable| text(variable, "description"))
                            .unwrap_or_default(),
                    },
                )
            })
            .collect();
        Self { parameters }
    }

    /// What a form should render, in a stable order.
    pub(crate) fn describe(&self) -> Vec<EngineParameter> {
        self.parameters
            .iter()
            .map(|(name, spec)| {
                let (kind, type_name, enum_values) = describe_type(&spec.literal_type);
                EngineParameter {
                    name: name.clone(),
                    kind,
                    required: spec.required,
                    default: spec.default.as_ref().and_then(literal_to_json),
                    description: spec.description.clone(),
                    type_name,
                    enum_values,
                }
            })
            .collect()
    }

    /// Bind supplied values to the declared types, as a `LiteralMap`.
    ///
    /// An empty result is correct and common: a launch plan whose inputs all
    /// carry defaults is started by sending no literals at all.
    pub(crate) fn bind(
        &self,
        workflow: &str,
        inputs: &BTreeMap<String, Value>,
    ) -> Result<Value, LaunchError> {
        let mut literals = Map::new();
        for (name, value) in inputs {
            let Some(spec) = self.parameters.get(name) else {
                return Err(LaunchError::UnknownInput {
                    workflow: workflow.to_owned(),
                    parameter: name.clone(),
                });
            };
            // An optional input left blank by a form arrives as null or "".
            // Omitting it is what lets the engine apply its own default;
            // encoding it would override that default with emptiness.
            if is_blank(value) && !spec.required {
                continue;
            }
            literals.insert(name.clone(), encode(name, &spec.literal_type, value)?);
        }
        for (name, spec) in &self.parameters {
            if spec.required && !literals.contains_key(name) {
                return Err(LaunchError::MissingInput {
                    parameter: name.clone(),
                });
            }
        }
        Ok(json!({ "literals": literals }))
    }
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        _ => false,
    }
}

/// The `SimpleType` of a literal type, as a name, whichever spelling the
/// gateway used. Protobuf JSON may render an enum as its name or as its
/// number, and both appear in the wild.
fn simple_type(literal_type: &Value) -> Option<&'static str> {
    const NUMBERED: [&str; 10] = [
        "NONE", "INTEGER", "FLOAT", "STRING", "BOOLEAN", "DATETIME", "DURATION", "BINARY", "ERROR",
        "STRUCT",
    ];
    let simple = field(literal_type, "simple")?;
    if let Some(name) = simple.as_str() {
        return NUMBERED.iter().find(|known| **known == name).copied();
    }
    let index = usize::try_from(simple.as_u64()?).ok()?;
    NUMBERED.get(index).copied()
}

/// How a form should render one declared type, and what to call it.
fn describe_type(literal_type: &Value) -> (ParameterKind, String, Vec<String>) {
    if let Some(simple) = simple_type(literal_type) {
        let kind = match simple {
            "INTEGER" => ParameterKind::Integer,
            "FLOAT" => ParameterKind::Float,
            "BOOLEAN" => ParameterKind::Boolean,
            "DATETIME" => ParameterKind::Datetime,
            "DURATION" => ParameterKind::Duration,
            "STRING" => ParameterKind::String,
            // NONE, BINARY, ERROR and STRUCT are all "send JSON and let the
            // engine decide"; the name beside the field is what tells them
            // apart.
            _ => ParameterKind::Json,
        };
        return (kind, simple.to_ascii_lowercase(), Vec::new());
    }
    if let Some(values) = field(literal_type, "enum_type").and_then(|enumeration| {
        field(enumeration, "values")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
    }) {
        return (ParameterKind::Enum, "enum".to_owned(), values);
    }
    if let Some(element) = field(literal_type, "collection_type") {
        let (_, element_name, _) = describe_type(element);
        return (
            ParameterKind::Collection,
            format!("list[{element_name}]"),
            Vec::new(),
        );
    }
    if let Some(element) = field(literal_type, "map_value_type") {
        let (_, element_name, _) = describe_type(element);
        return (
            ParameterKind::Map,
            format!("map[str, {element_name}]"),
            Vec::new(),
        );
    }
    if let Some(variants) = field(literal_type, "union_type")
        .and_then(|union| field(union, "variants"))
        .and_then(Value::as_array)
    {
        // `Optional[T]` is a union of T and NONE, and it is by far the most
        // common union in a real launch plan. Rendering it as T with the
        // required flag already off is the difference between a form somebody
        // can fill in and a JSON box.
        let described: Vec<_> = variants.iter().map(describe_type).collect();
        let mut concrete = described
            .iter()
            .filter(|(_, name, _)| name != "none")
            .cloned();
        if let (Some(single), None) = (concrete.next(), concrete.next()) {
            let (kind, name, values) = single;
            return (kind, format!("optional[{name}]"), values);
        }
        let names: Vec<_> = described.into_iter().map(|(_, name, _)| name).collect();
        return (
            ParameterKind::Json,
            format!("union[{}]", names.join(", ")),
            Vec::new(),
        );
    }
    if field(literal_type, "blob").is_some() {
        return (ParameterKind::String, "blob".to_owned(), Vec::new());
    }
    if field(literal_type, "structured_dataset_type").is_some() {
        return (
            ParameterKind::String,
            "structured_dataset".to_owned(),
            Vec::new(),
        );
    }
    if field(literal_type, "schema").is_some() {
        return (ParameterKind::String, "schema".to_owned(), Vec::new());
    }
    (ParameterKind::Json, String::new(), Vec::new())
}

fn wrong(parameter: &str, expected: &str, value: &Value) -> LaunchError {
    LaunchError::WrongType {
        parameter: parameter.to_owned(),
        expected: expected.to_owned(),
        // The value, not its type name: "expects a timestamp and got
        // \"yesterday\"" says what to fix, and `String` does not.
        got: match value {
            Value::String(text) => format!("{text:?}"),
            other => other.to_string(),
        },
    }
}

/// One JSON value as a `Literal` of the declared type.
fn encode(parameter: &str, literal_type: &Value, value: &Value) -> Result<Value, LaunchError> {
    if let Some(simple) = simple_type(literal_type) {
        return encode_simple(parameter, simple, value);
    }
    if let Some(enumeration) = field(literal_type, "enum_type") {
        let permitted: Vec<&str> = field(enumeration, "values")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let chosen = value
            .as_str()
            .ok_or_else(|| wrong(parameter, "one of the declared values", value))?;
        if !permitted.contains(&chosen) {
            return Err(wrong(
                parameter,
                &format!("one of [{}]", permitted.join(", ")),
                value,
            ));
        }
        return Ok(json!({ "scalar": { "primitive": { "string_value": chosen } } }));
    }
    if let Some(element) = field(literal_type, "collection_type") {
        let items = value
            .as_array()
            .ok_or_else(|| wrong(parameter, "a list", value))?;
        let literals = items
            .iter()
            .map(|item| encode(parameter, element, item))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(json!({ "collection": { "literals": literals } }));
    }
    if let Some(element) = field(literal_type, "map_value_type") {
        let entries = value
            .as_object()
            .ok_or_else(|| wrong(parameter, "an object", value))?;
        let mut literals = Map::new();
        for (key, item) in entries {
            literals.insert(key.clone(), encode(parameter, element, item)?);
        }
        return Ok(json!({ "map": { "literals": literals } }));
    }
    if let Some(variants) = field(literal_type, "union_type")
        .and_then(|union| field(union, "variants"))
        .and_then(Value::as_array)
    {
        // First variant that accepts the value wins, and NONE is tried first
        // for a null so `Optional[str]` does not turn a blank field into the
        // string "null".
        let ordered = variants.iter().filter(|variant| {
            value.is_null() == (simple_type(variant) == Some("NONE"))
                || simple_type(variant) != Some("NONE")
        });
        for variant in ordered {
            if let Ok(encoded) = encode(parameter, variant, value) {
                return Ok(json!({ "scalar": { "union": { "value": encoded, "type": variant } } }));
            }
        }
        return Err(wrong(parameter, "one of the union's variants", value));
    }
    if let Some(blob_type) = field(literal_type, "blob") {
        let uri = value
            .as_str()
            .ok_or_else(|| wrong(parameter, "a URI", value))?;
        return Ok(json!({
            "scalar": { "blob": { "metadata": { "type": blob_type }, "uri": uri } }
        }));
    }
    if let Some(dataset_type) = field(literal_type, "structured_dataset_type") {
        let uri = value
            .as_str()
            .ok_or_else(|| wrong(parameter, "a URI", value))?;
        return Ok(json!({
            "scalar": {
                "structured_dataset": {
                    "uri": uri,
                    "metadata": { "structured_dataset_type": dataset_type },
                }
            }
        }));
    }
    // A type this adapter does not model. Sending the value as a struct is a
    // guess the engine gets to refuse with its own message, which is more
    // useful than refusing it here on the strength of not recognising a field
    // name.
    Ok(json!({ "scalar": { "generic": value } }))
}

fn encode_simple(parameter: &str, simple: &str, value: &Value) -> Result<Value, LaunchError> {
    let primitive = |inner: Value| json!({ "scalar": { "primitive": inner } });
    match simple {
        "STRING" => {
            let text = value
                .as_str()
                .ok_or_else(|| wrong(parameter, "a string", value))?;
            Ok(primitive(json!({ "string_value": text })))
        }
        "INTEGER" => {
            let number = as_integer(value).ok_or_else(|| wrong(parameter, "an integer", value))?;
            // int64 is a *string* in protobuf JSON. Sending it as a number
            // works until the value is bigger than 2^53, at which point the
            // gateway's parse fails on a value the caller typed correctly.
            Ok(primitive(json!({ "integer": number.to_string() })))
        }
        "FLOAT" => {
            let number = as_float(value).ok_or_else(|| wrong(parameter, "a number", value))?;
            Ok(primitive(json!({ "float_value": number })))
        }
        "BOOLEAN" => {
            let flag = match value {
                Value::Bool(flag) => Some(*flag),
                Value::String(text) => text.parse().ok(),
                _ => None,
            }
            .ok_or_else(|| wrong(parameter, "true or false", value))?;
            Ok(primitive(json!({ "boolean": flag })))
        }
        "DATETIME" => {
            let text = value
                .as_str()
                .ok_or_else(|| wrong(parameter, "an RFC 3339 timestamp", value))?;
            let parsed =
                time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                    .map_err(|_| wrong(parameter, "an RFC 3339 timestamp", value))?;
            let normalised = parsed
                .to_offset(time::UtcOffset::UTC)
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| wrong(parameter, "an RFC 3339 timestamp", value))?;
            Ok(primitive(json!({ "datetime": normalised })))
        }
        "DURATION" => {
            let seconds =
                duration_seconds(value).ok_or_else(|| wrong(parameter, "a duration", value))?;
            Ok(primitive(json!({ "duration": format!("{seconds}s") })))
        }
        "NONE" => Ok(json!({ "scalar": { "none_type": {} } })),
        // STRUCT, BINARY and ERROR: pass the JSON through as a struct. A form
        // cannot produce a meaningful binary literal, and refusing the whole
        // launch because one optional input is a blob type would be worse than
        // letting the engine say so.
        _ => Ok(json!({ "scalar": { "generic": value } })),
    }
}

fn as_integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        // A form field is a string even when it holds a number.
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn as_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Seconds, from a number or from `900s` / `15m` / `24h` / `7d`.
///
/// The unit suffixes are here because a duration input is almost always a
/// window somebody is typing by hand, and `604800` is a number people get
/// wrong. Protobuf JSON wants seconds with an `s`, which is what comes back.
fn duration_seconds(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(number) = value.as_f64() {
        #[allow(clippy::cast_possible_truncation)]
        return Some(number as i64);
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    let (digits, multiplier) = match text.as_bytes().last()? {
        b's' => (&text[..text.len() - 1], 1),
        b'm' => (&text[..text.len() - 1], 60),
        b'h' => (&text[..text.len() - 1], 3_600),
        b'd' => (&text[..text.len() - 1], 86_400),
        _ => (text, 1),
    };
    digits
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
}

/// A `Literal` back as plain JSON, for showing a default in a form.
///
/// Best effort and lossy on purpose: this feeds a placeholder, never a launch.
/// Anything unrecognised comes back as `None`, which renders as "no default"
/// rather than as a wrong one.
pub(crate) fn literal_to_json(literal: &Value) -> Option<Value> {
    if let Some(scalar) = field(literal, "scalar") {
        if let Some(primitive) = field(scalar, "primitive") {
            for name in [
                "string_value",
                "boolean",
                "float_value",
                "datetime",
                "duration",
            ] {
                if let Some(found) = field(primitive, name) {
                    return Some(found.clone());
                }
            }
            if let Some(integer) = field(primitive, "integer") {
                // int64 arrives as a string; a form field wants the number.
                return integer
                    .as_str()
                    .and_then(|text| text.parse::<i64>().ok())
                    .map(Value::from)
                    .or_else(|| Some(integer.clone()));
            }
            return None;
        }
        if let Some(generic) = field(scalar, "generic") {
            return Some(generic.clone());
        }
        if field(scalar, "none_type").is_some() {
            return Some(Value::Null);
        }
        if let Some(blob) = field(scalar, "blob") {
            return field(blob, "uri").cloned();
        }
        if let Some(dataset) = field(scalar, "structured_dataset") {
            return field(dataset, "uri").cloned();
        }
        if let Some(union) = field(scalar, "union") {
            return field(union, "value").and_then(literal_to_json);
        }
        return None;
    }
    if let Some(items) = field(literal, "collection")
        .and_then(|collection| field(collection, "literals"))
        .and_then(Value::as_array)
    {
        return Some(Value::Array(
            items.iter().filter_map(literal_to_json).collect(),
        ));
    }
    if let Some(entries) = field(literal, "map")
        .and_then(|map| field(map, "literals"))
        .and_then(Value::as_object)
    {
        return Some(Value::Object(
            entries
                .iter()
                .filter_map(|(key, item)| literal_to_json(item).map(|item| (key.clone(), item)))
                .collect(),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface() -> Interface {
        Interface::read(Some(&json!({
            "parameters": {
                "dataset": { "var": { "type": { "simple": "STRING" }, "description": "Where to write" }, "required": true },
                "since": { "var": { "type": { "simple": "DATETIME" } }, "required": true },
                "window": { "var": { "type": { "simple": "DURATION" } }, "default": { "scalar": { "primitive": { "duration": "3600s" } } } },
                "limit": { "var": { "type": { "simple": "INTEGER" } }, "default": { "scalar": { "primitive": { "integer": "500" } } } },
                "agents": { "var": { "type": { "collectionType": { "simple": "STRING" } } }, "required": false },
                "split": { "var": { "type": { "enum_type": { "values": ["train", "holdout"] } } }, "required": false },
                "note": { "var": { "type": { "union_type": { "variants": [{ "simple": "STRING" }, { "simple": "NONE" }] } } }, "required": false }
            }
        })))
    }

    #[test]
    fn a_field_is_found_under_either_protobuf_spelling() {
        // The gateway may emit `expected_inputs` or `expectedInputs` depending
        // on how it was built, and reading only one is an interface that comes
        // back empty against somebody else's Flyte.
        let camel = json!({ "expectedInputs": { "parameters": {} } });
        let snake = json!({ "expected_inputs": { "parameters": {} } });
        assert!(field(&camel, "expected_inputs").is_some());
        assert!(field(&snake, "expected_inputs").is_some());
        assert!(field(&snake, "missing").is_none());
    }

    #[test]
    fn a_parameter_with_a_default_is_optional_however_the_flag_reads() {
        let interface = interface();
        let described: BTreeMap<_, _> = interface
            .describe()
            .into_iter()
            .map(|parameter| (parameter.name.clone(), parameter))
            .collect();
        assert!(described["dataset"].required);
        assert!(
            !described["window"].required,
            "a default is what makes an input optional"
        );
        assert_eq!(described["limit"].default, Some(json!(500)));
        assert_eq!(described["window"].default, Some(json!("3600s")));
        assert_eq!(described["dataset"].description, "Where to write");
    }

    #[test]
    fn every_declared_type_reaches_the_form_as_something_it_can_render() {
        let described: BTreeMap<_, _> = interface()
            .describe()
            .into_iter()
            .map(|parameter| (parameter.name.clone(), parameter))
            .collect();
        assert_eq!(described["since"].kind, ParameterKind::Datetime);
        assert_eq!(described["window"].kind, ParameterKind::Duration);
        assert_eq!(described["agents"].kind, ParameterKind::Collection);
        assert_eq!(described["agents"].type_name, "list[string]");
        assert_eq!(described["split"].kind, ParameterKind::Enum);
        assert_eq!(described["split"].enum_values, ["train", "holdout"]);
        // Optional[str] is a union, and rendering it as a JSON box would be
        // the single most common way this form becomes unusable.
        assert_eq!(described["note"].kind, ParameterKind::String);
        assert_eq!(described["note"].type_name, "optional[string]");
    }

    #[test]
    fn binding_encodes_each_value_as_the_declared_type() {
        let inputs = BTreeMap::from([
            ("dataset".to_owned(), json!("s3://curated/houses")),
            ("since".to_owned(), json!("2026-08-01T00:00:00Z")),
            ("window".to_owned(), json!("24h")),
            ("limit".to_owned(), json!("500")),
            ("agents".to_owned(), json!(["planner", "importer"])),
            ("split".to_owned(), json!("holdout")),
        ]);
        let bound = interface().bind("lp:p:d:n:v", &inputs).expect("binds");
        let literals = &bound["literals"];
        assert_eq!(
            literals["dataset"]["scalar"]["primitive"]["string_value"],
            "s3://curated/houses"
        );
        assert_eq!(
            literals["since"]["scalar"]["primitive"]["datetime"],
            "2026-08-01T00:00:00Z"
        );
        // A hand-typed window, as protobuf JSON's seconds-with-an-s.
        assert_eq!(
            literals["window"]["scalar"]["primitive"]["duration"],
            "86400s"
        );
        // int64 is a string in protobuf JSON, and a form sends a string
        // anyway; both have to land as a quoted integer.
        assert_eq!(literals["limit"]["scalar"]["primitive"]["integer"], "500");
        assert_eq!(
            literals["agents"]["collection"]["literals"][1]["scalar"]["primitive"]["string_value"],
            "importer"
        );
        assert_eq!(
            literals["split"]["scalar"]["primitive"]["string_value"],
            "holdout"
        );
    }

    #[test]
    fn an_input_the_entity_does_not_declare_is_refused() {
        // Not dropped. An orchestrator that ignores an unknown field turns a
        // typo in a filter into a run over everything.
        let inputs = BTreeMap::from([
            ("dataset".to_owned(), json!("x")),
            ("since".to_owned(), json!("2026-08-01T00:00:00Z")),
            ("windwo".to_owned(), json!("24h")),
        ]);
        assert_eq!(
            interface().bind("lp:p:d:n:v", &inputs),
            Err(LaunchError::UnknownInput {
                workflow: "lp:p:d:n:v".to_owned(),
                parameter: "windwo".to_owned(),
            })
        );
    }

    #[test]
    fn a_required_input_left_out_is_refused_before_the_request_leaves() {
        let inputs = BTreeMap::from([("dataset".to_owned(), json!("x"))]);
        assert_eq!(
            interface().bind("lp:p:d:n:v", &inputs),
            Err(LaunchError::MissingInput {
                parameter: "since".to_owned()
            })
        );
    }

    #[test]
    fn a_blank_optional_input_is_omitted_so_the_engines_default_survives() {
        // Sending an empty string would override the launch plan's default
        // with emptiness, which is the opposite of leaving a field alone.
        let inputs = BTreeMap::from([
            ("dataset".to_owned(), json!("x")),
            ("since".to_owned(), json!("2026-08-01T00:00:00Z")),
            ("window".to_owned(), json!("")),
            ("note".to_owned(), Value::Null),
        ]);
        let bound = interface().bind("lp:p:d:n:v", &inputs).expect("binds");
        assert!(bound["literals"].get("window").is_none());
        assert!(bound["literals"].get("note").is_none());
    }

    #[test]
    fn a_value_of_the_wrong_shape_names_the_parameter_and_what_it_wanted() {
        let inputs = BTreeMap::from([
            ("dataset".to_owned(), json!("x")),
            ("since".to_owned(), json!("yesterday")),
        ]);
        let error = interface()
            .bind("lp:p:d:n:v", &inputs)
            .expect_err("refused");
        let rendered = error.to_string();
        assert!(rendered.contains("since"), "{rendered}");
        assert!(rendered.contains("RFC 3339"), "{rendered}");
        assert!(rendered.contains("yesterday"), "{rendered}");
    }

    #[test]
    fn an_optional_value_binds_to_the_variant_that_accepts_it() {
        let inputs = BTreeMap::from([
            ("dataset".to_owned(), json!("x")),
            ("since".to_owned(), json!("2026-08-01T00:00:00Z")),
            ("note".to_owned(), json!("first pass")),
        ]);
        let bound = interface().bind("lp:p:d:n:v", &inputs).expect("binds");
        let union = &bound["literals"]["note"]["scalar"]["union"];
        assert_eq!(
            union["value"]["scalar"]["primitive"]["string_value"],
            "first pass"
        );
        assert_eq!(union["type"]["simple"], "STRING");
    }

    #[test]
    fn a_timestamp_reaches_flyte_in_utc_whatever_offset_it_arrived_in() {
        let inputs = BTreeMap::from([
            ("dataset".to_owned(), json!("x")),
            ("since".to_owned(), json!("2026-08-01T02:00:00+02:00")),
        ]);
        let bound = interface().bind("lp:p:d:n:v", &inputs).expect("binds");
        assert_eq!(
            bound["literals"]["since"]["scalar"]["primitive"]["datetime"],
            "2026-08-01T00:00:00Z"
        );
    }

    #[test]
    fn an_enum_value_outside_the_declared_set_never_leaves_the_process() {
        let inputs = BTreeMap::from([
            ("dataset".to_owned(), json!("x")),
            ("since".to_owned(), json!("2026-08-01T00:00:00Z")),
            ("split".to_owned(), json!("dev")),
        ]);
        let error = interface()
            .bind("lp:p:d:n:v", &inputs)
            .expect_err("refused");
        assert!(error.to_string().contains("train, holdout"), "{error}");
    }

    #[test]
    fn a_simple_type_is_read_whether_the_gateway_sent_a_name_or_a_number() {
        assert_eq!(
            simple_type(&json!({ "simple": "DATETIME" })),
            Some("DATETIME")
        );
        assert_eq!(simple_type(&json!({ "simple": 5 })), Some("DATETIME"));
        assert_eq!(simple_type(&json!({ "collection_type": {} })), None);
    }

    #[test]
    fn a_default_renders_back_into_something_a_field_can_hold() {
        assert_eq!(
            literal_to_json(&json!({ "scalar": { "primitive": { "integer": "42" } } })),
            Some(json!(42))
        );
        assert_eq!(
            literal_to_json(&json!({ "collection": { "literals": [
                { "scalar": { "primitive": { "string_value": "a" } } }
            ] } })),
            Some(json!(["a"]))
        );
        assert_eq!(
            literal_to_json(&json!({ "scalar": { "blob": { "uri": "s3://bucket/key" } } })),
            Some(json!("s3://bucket/key"))
        );
        assert_eq!(literal_to_json(&json!({ "unrecognised": {} })), None);
    }
}
