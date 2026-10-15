//! Tiny JSON-Schema subset used to describe and validate tool arguments.
//!
//! Supported keywords: `type` (string, integer, number, boolean), `enum`,
//! `minimum`, `maximum`, `required`, `properties` and
//! `additionalProperties: false`. That is everything the tool table needs, and
//! keeping it this small lets the server reject bad input with precise
//! messages instead of passing it to the simulator.

use serde_json::{Map, Value, json};

/// A string parameter.
pub fn string(description: &str, examples: &[&str]) -> Value {
    json!({ "type": "string", "description": description, "examples": examples })
}

/// An integer parameter with optional bounds.
pub fn integer(description: &str, min: Option<i64>, max: Option<i64>, examples: &[i64]) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!("integer"));
    m.insert("description".into(), json!(description));
    m.insert("examples".into(), json!(examples));
    if let Some(v) = min {
        m.insert("minimum".into(), json!(v));
    }
    if let Some(v) = max {
        m.insert("maximum".into(), json!(v));
    }
    Value::Object(m)
}

/// A numeric parameter with optional bounds.
pub fn number(description: &str, min: Option<f64>, max: Option<f64>, examples: &[f64]) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!("number"));
    m.insert("description".into(), json!(description));
    m.insert("examples".into(), json!(examples));
    if let Some(v) = min {
        m.insert("minimum".into(), json!(v));
    }
    if let Some(v) = max {
        m.insert("maximum".into(), json!(v));
    }
    Value::Object(m)
}

/// An array of strings.
pub fn string_array(description: &str, examples: &[&str]) -> Value {
    json!({ "type": "array", "items": { "type": "string" },
            "description": description, "examples": [examples] })
}

/// A boolean parameter.
pub fn boolean(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

/// A string parameter restricted to a fixed set of values.
pub fn enumeration(description: &str, values: &[&str]) -> Value {
    json!({ "type": "string", "description": description, "enum": values, "examples": [values[0]] })
}

/// An object schema that rejects unknown parameters.
pub fn object(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let properties: Map<String, Value> =
        props.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Checks `args` against `schema`, returning a message naming the first
/// problem found.
pub fn validate(schema: &Value, args: &Value) -> Result<(), String> {
    let empty = Map::new();
    let obj = match args {
        Value::Null => &empty,
        Value::Object(m) => m,
        other => return Err(format!("arguments must be an object, got {}", kind(other))),
    };
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    if let Some(req) = schema.get("required").and_then(Value::as_array) {
        for r in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(r) {
                return Err(format!("missing required parameter `{r}`"));
            }
        }
    }
    for (k, v) in obj {
        let Some(p) = props.get(k) else {
            let allowed: Vec<&str> = props.keys().map(String::as_str).collect();
            return Err(format!(
                "unknown parameter `{k}`; allowed: {}",
                if allowed.is_empty() {
                    "(none)".to_string()
                } else {
                    allowed.join(", ")
                }
            ));
        };
        check_value(k, p, v)?;
    }
    Ok(())
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn check_value(name: &str, schema: &Value, v: &Value) -> Result<(), String> {
    let ty = schema.get("type").and_then(Value::as_str).unwrap_or("");
    let ok = match ty {
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
        "number" => v.is_number(),
        _ => true,
    };
    if !ok {
        return Err(format!("`{name}` must be {ty}, got {}", kind(v)));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(v) {
            let list: Vec<String> = allowed
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect();
            return Err(format!("`{name}` must be one of: {}", list.join(", ")));
        }
    }
    if let Some(x) = v.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
            if x < min {
                return Err(format!("`{name}` is {x}, below the minimum {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
            if x > max {
                return Err(format!("`{name}` is {x}, above the maximum {max}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> Value {
        object(
            vec![
                ("n", integer("count", Some(1), Some(8), &[2])),
                ("mode", enumeration("mode", &["a", "b"])),
                ("flag", boolean("flag")),
                ("name", string("name", &["x"])),
            ],
            &["n"],
        )
    }

    #[test]
    fn accepts_valid_and_rejects_each_kind_of_mistake() {
        assert!(
            validate(
                &s(),
                &json!({"n": 3, "mode": "a", "flag": true, "name": "q"})
            )
            .is_ok()
        );
        let cases = [
            (json!({}), "missing required parameter `n`"),
            (json!({"n": "3"}), "`n` must be integer"),
            (json!({"n": 3.5}), "`n` must be integer"),
            (json!({"n": 0}), "below the minimum"),
            (json!({"n": 9}), "above the maximum"),
            (json!({"n": 1, "mode": "c"}), "must be one of: a, b"),
            (json!({"n": 1, "flag": 1}), "`flag` must be boolean"),
            (json!({"n": 1, "extra": 1}), "unknown parameter `extra`"),
            (json!([1]), "must be an object"),
        ];
        for (args, want) in cases {
            let e = validate(&s(), &args).unwrap_err();
            assert!(e.contains(want), "{args}: {e}");
        }
    }

    #[test]
    fn null_arguments_are_an_empty_object() {
        let none = object(vec![], &[]);
        assert!(validate(&none, &Value::Null).is_ok());
        assert!(validate(&s(), &Value::Null).is_err());
    }
}
