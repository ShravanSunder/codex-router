//! Strict schema JSON admission before RFC 8785 serialization can discard information.
use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;

pub(crate) fn parse_schema_document(bytes: &[u8]) -> serde_json::Result<Value> {
    serde_json::from_slice::<SchemaValue>(bytes).map(|value| value.0)
}

struct SchemaValue(Value);
impl<'de> Deserialize<'de> for SchemaValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(SchemaVisitor)
    }
}
struct SchemaVisitor;
impl<'de> Visitor<'de> for SchemaVisitor {
    type Value = SchemaValue;
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON with unique object keys and finite numbers")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Self::Value, M::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(M::Error::custom("duplicate schema key"));
            }
            values.insert(key, access.next_value::<SchemaValue>()?.0);
        }
        Ok(SchemaValue(Value::Object(values)))
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut access: S) -> Result<Self::Value, S::Error> {
        let mut values = Vec::new();
        while let Some(value) = access.next_element::<SchemaValue>()? {
            values.push(value.0);
        }
        Ok(SchemaValue(Value::Array(values)))
    }
    fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(SchemaValue(Value::String(value.to_owned())))
    }
    fn visit_bool<E: Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(SchemaValue(Value::Bool(value)))
    }
    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(SchemaValue(Value::Null))
    }
    fn visit_i64<E: Error>(self, value: i64) -> Result<Self::Value, E> {
        require_exact_integer::<E>(value.unsigned_abs())?;
        Ok(SchemaValue(Value::Number(value.into())))
    }
    fn visit_u64<E: Error>(self, value: u64) -> Result<Self::Value, E> {
        require_exact_integer::<E>(value)?;
        Ok(SchemaValue(Value::Number(value.into())))
    }
    fn visit_f64<E: Error>(self, value: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(value)
            .map(|number| SchemaValue(Value::Number(number)))
            .ok_or_else(|| E::custom("non-finite schema number"))
    }
}

fn require_exact_integer<E: Error>(magnitude: u64) -> Result<(), E> {
    // Binary64 preserves 53 significant bits; powers of two beyond 2^53 remain exact.
    let significant_bits = u64::BITS - magnitude.leading_zeros();
    if significant_bits > 53 + magnitude.trailing_zeros() {
        Err(E::custom(
            "schema integer cannot be represented exactly as binary64",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod validation_tests {
    use super::parse_schema_document;

    #[test]
    fn rejects_duplicate_keys_even_when_escaped_or_nested() {
        for source in [
            r#"{"type":"object","type":"string"}"#,
            r#"{"definitions":{"a":{},"\u0061":{}}}"#,
        ] {
            assert!(parse_schema_document(source.as_bytes()).is_err());
        }
    }

    #[test]
    fn canonicalization_preserves_utf16_order_and_ecmascript_numbers() {
        let parsed =
            parse_schema_document(r#"{"\ue000":1.0,"\ud83d\ude00":1e-7,"a":-0.0}"#.as_bytes())
                .unwrap_or_else(|error| panic!("parse: {error}"));
        let canonical = serde_json_canonicalizer::to_vec(&parsed)
            .unwrap_or_else(|error| panic!("canonicalize: {error}"));
        assert_eq!(canonical, "{\"a\":0,\"😀\":1e-7,\"\u{e000}\":1}".as_bytes());
    }

    #[test]
    fn rejects_non_unicode_and_non_finite_values() {
        for source in [r#"{"a":"\ud800"}"#, r#"{"a":1e999}"#, r#"{"a":NaN}"#] {
            assert!(parse_schema_document(source.as_bytes()).is_err());
        }
    }

    #[test]
    fn rejects_integer_values_that_canonicalization_would_silently_round() {
        assert!(parse_schema_document(b"{\"maximum\":9007199254740993}").is_err());
        assert!(parse_schema_document(b"{\"minimum\":-9007199254740993}").is_err());
        assert!(parse_schema_document(b"{\"maximum\":18446744073709551615}").is_err());
        assert!(parse_schema_document(b"{\"maximum\":1152921504606846976}").is_ok());
    }
}
