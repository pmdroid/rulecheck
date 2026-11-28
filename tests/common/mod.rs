//! A small JSON Schema checker for the keywords that Rulecheck's published
//! schemas use. It panics on an unknown keyword so a schema change cannot
//! silently skip a check.
use serde_json::Value;

/// Returns every violation of `schema` by `value`, as `path: reason` strings.
pub fn violations(schema: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(schema, schema, value, "$", &mut errors);
    errors
}

fn matches(root: &Value, schema: &Value, value: &Value) -> bool {
    let mut errors = Vec::new();
    check(root, schema, value, "$", &mut errors);
    errors.is_empty()
}

fn check(root: &Value, schema: &Value, value: &Value, at: &str, errors: &mut Vec<String>) {
    let schema = schema.as_object().expect("schema object");
    let mut reasons = Vec::new();
    for (keyword, rule) in schema {
        match keyword.as_str() {
            "$schema" | "title" | "description" | "$defs" | "then" | "else" => {}
            "$ref" => {
                let name = rule.as_str().unwrap().strip_prefix("#/$defs/").unwrap();
                check(root, &root["$defs"][name], value, at, errors);
            }
            "type" => {
                let types: Vec<&str> = match rule {
                    Value::String(one) => vec![one.as_str()],
                    many => many
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|t| t.as_str().unwrap())
                        .collect(),
                };
                let actual = match value {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Array(_) => "array",
                    Value::Object(_) => "object",
                };
                if !types
                    .iter()
                    .any(|t| *t == actual || (*t == "number" && actual == "integer"))
                {
                    reasons.push(format!("expected {types:?}, found {actual}"));
                }
            }
            "const" => {
                if value != rule {
                    reasons.push(format!("expected {rule}"));
                }
            }
            "enum" => {
                if !rule.as_array().unwrap().contains(value) {
                    reasons.push(format!("expected one of {rule}"));
                }
            }
            "required" => {
                for key in rule.as_array().unwrap() {
                    if value.is_object() && value.get(key.as_str().unwrap()).is_none() {
                        reasons.push(format!("missing {key}"));
                    }
                }
            }
            "properties" => {
                if let Some(object) = value.as_object() {
                    for (key, property) in rule.as_object().unwrap() {
                        if let Some(child) = object.get(key) {
                            check(root, property, child, &format!("{at}.{key}"), errors);
                        }
                    }
                }
            }
            "additionalProperties" => {
                assert_eq!(rule, &Value::Bool(false));
                let known = schema["properties"].as_object().unwrap();
                for key in value.as_object().into_iter().flat_map(|o| o.keys()) {
                    if !known.contains_key(key) {
                        reasons.push(format!("unknown property {key:?}"));
                    }
                }
            }
            "items" => {
                for (index, item) in value.as_array().into_iter().flatten().enumerate() {
                    check(root, rule, item, &format!("{at}[{index}]"), errors);
                }
            }
            "minItems" | "maxItems" | "minLength" | "maxLength" => {
                let actual = match value {
                    Value::Array(items) if keyword.ends_with("Items") => items.len(),
                    Value::String(text) if keyword.ends_with("Length") => text.chars().count(),
                    _ => continue,
                };
                let limit = rule.as_u64().unwrap() as usize;
                if (keyword.starts_with("min") && actual < limit)
                    || (keyword.starts_with("max") && actual > limit)
                {
                    reasons.push(format!("{keyword} {limit}, found {actual}"));
                }
            }
            "minimum" => {
                if let Some(number) = value.as_f64()
                    && number < rule.as_f64().unwrap()
                {
                    reasons.push(format!("below minimum {rule}"));
                }
            }
            "maximum" => {
                if let Some(number) = value.as_f64()
                    && number > rule.as_f64().unwrap()
                {
                    reasons.push(format!("above maximum {rule}"));
                }
            }
            "pattern" => {
                if let Some(text) = value.as_str()
                    && !regex::Regex::new(rule.as_str().unwrap())
                        .unwrap()
                        .is_match(text)
                {
                    reasons.push(format!("{text:?} does not match {rule}"));
                }
            }
            "if" => {
                let branch = if matches(root, rule, value) {
                    "then"
                } else {
                    "else"
                };
                if let Some(branch) = schema.get(branch) {
                    check(root, branch, value, at, errors);
                }
            }
            "not" => {
                if matches(root, rule, value) {
                    reasons.push(format!("must not match {rule}"));
                }
            }
            "oneOf" => {
                let count = rule
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|option| matches(root, option, value))
                    .count();
                if count != 1 {
                    reasons.push(format!("matches {count} oneOf options"));
                }
            }
            other => panic!("unsupported schema keyword {other}"),
        }
    }
    errors.extend(reasons.into_iter().map(|reason| format!("{at}: {reason}")));
}
