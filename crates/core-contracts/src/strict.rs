use crate::CoreError;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Unique, M::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom("Duplicate JSON key"));
                    }
                    out.insert(key, map.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(out)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
                let mut out = Vec::new();
                while let Some(value) = seq.next_element::<Unique>()? {
                    out.push(value.0);
                }
                Ok(Unique(Value::Array(out)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(Value::String(v.into())))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Unique, E> {
                Number::from_f64(v)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| de::Error::custom("Nonfinite number"))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

pub fn strict_json(bytes: &[u8]) -> Result<Value, CoreError> {
    // Never include a serde error or untrusted input in logs/responses. They may
    // contain a supplied secret or credential-bearing URL.
    if bytes.len() >= crate::MAX_FRAME {
        return Err(CoreError::invalid("JSON exceeds frame limit"));
    }
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = Unique::deserialize(&mut parser)
        .map_err(|_| CoreError::invalid("Invalid UTF-8 JSON or duplicate key"))?;
    parser
        .end()
        .map_err(|_| CoreError::invalid("Trailing data after JSON"))?;
    Ok(value.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_nested_and_escaped_duplicate_keys() {
        for text in [r#"{"a":1,"a":2}"#, r#"{"x":[{"a":1,"\u0061":2}]}"#] {
            assert!(strict_json(text.as_bytes()).is_err());
        }
    }
    #[test]
    fn rejects_trailing_data_invalid_utf8_and_deep_nesting() {
        assert!(strict_json(b"{} {}").is_err());
        assert!(strict_json(&[b'"', 255, b'"']).is_err());
        assert!(
            strict_json(format!("{}0{}", "[".repeat(150), "]".repeat(150)).as_bytes()).is_err()
        );
    }
    #[test]
    fn preserves_json_scalars() {
        let text = r#"[null,true,false,-1,18446744073709551615,1.25,"فارسی"]"#.as_bytes();
        assert_eq!(
            strict_json(text).unwrap(),
            serde_json::from_slice::<Value>(text).unwrap()
        );
    }
    #[test]
    fn parser_errors_do_not_echo_secrets() {
        let e = strict_json(br#"{"password":"synthetic-secret","password":null}"#).unwrap_err();
        assert!(!e.to_string().contains("synthetic-secret"));
    }
}
