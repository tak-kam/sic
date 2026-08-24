//! Writing OTLP/JSON by hand.
//!
//! The shapes here are small and fixed, so a builder is enough; a serialization
//! framework would be a dependency for something a hundred lines covers.
//!
//! What a builder does not decide is how a string is escaped. That belongs to
//! the format rather than to OTLP, and comes from `sic-json`.

#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
        )
    }

    pub fn str(text: impl Into<String>) -> Value {
        Value::Str(text.into())
    }

    /// An OTLP attribute: a key and a typed value.
    pub fn attribute(key: &str, value: Value) -> Value {
        let typed = match &value {
            Value::Str(_) => "stringValue",
            Value::Int(_) => "intValue",
            Value::Bool(_) => "boolValue",
            _ => "stringValue",
        };
        Value::object(vec![
            ("key", Value::str(key)),
            ("value", Value::object(vec![(typed, value)])),
        ])
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Value::Str(text) => sic_json::write_quoted(out, text),
            // OTLP writes 64-bit numbers as strings, because JSON numbers are
            // doubles and would lose the low bits.
            Value::Int(v) => out.push_str(&v.to_string()),
            Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Value::Object(fields) => {
                out.push('{');
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    sic_json::write_quoted(out, name);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}
