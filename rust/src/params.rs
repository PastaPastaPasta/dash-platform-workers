use dash_sdk::dpp::platform_value::string_encoding::Encoding;
use dash_sdk::platform::Identifier;
use serde_json::{Map, Value};
use wasm_bindgen::JsError;

use crate::js;

/// A method's JSON parameters, with typed accessors that name the missing or malformed field.
pub(crate) struct Params(Map<String, Value>);

impl Params {
    pub(crate) fn parse(json: &str) -> Result<Self, JsError> {
        match serde_json::from_str(json).map_err(js)? {
            Value::Object(map) => Ok(Self(map)),
            Value::Null => Ok(Self(Map::new())),
            _ => Err(JsError::new("params must be a JSON object")),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    fn required(&self, key: &str) -> Result<&Value, JsError> {
        self.get(key)
            .ok_or_else(|| JsError::new(&format!("missing parameter: {key}")))
    }

    pub(crate) fn str(&self, key: &str) -> Result<&str, JsError> {
        self.required(key)?
            .as_str()
            .ok_or_else(|| JsError::new(&format!("{key} must be a string")))
    }

    pub(crate) fn opt_str(&self, key: &str) -> Result<Option<&str>, JsError> {
        self.get(key).map(|_| self.str(key)).transpose()
    }

    pub(crate) fn id(&self, key: &str) -> Result<Identifier, JsError> {
        parse_id(self.str(key)?, key)
    }

    pub(crate) fn ids(&self, key: &str) -> Result<Vec<Identifier>, JsError> {
        self.required(key)?
            .as_array()
            .ok_or_else(|| JsError::new(&format!("{key} must be an array of identifiers")))?
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| JsError::new(&format!("{key} must hold strings")))
                    .and_then(|s| parse_id(s, key))
            })
            .collect()
    }

    /// Amounts arrive as numbers or decimal strings, since JS numbers lose precision past 2^53.
    pub(crate) fn u64(&self, key: &str) -> Result<u64, JsError> {
        let value = self.required(key)?;
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| JsError::new(&format!("{key} must be a non-negative integer")))
    }

    pub(crate) fn opt_u64(&self, key: &str) -> Result<Option<u64>, JsError> {
        self.get(key).map(|_| self.u64(key)).transpose()
    }

    pub(crate) fn opt_u32(&self, key: &str) -> Result<Option<u32>, JsError> {
        self.opt_u64(key)?
            .map(|v| u32::try_from(v).map_err(|_| JsError::new(&format!("{key} is out of range"))))
            .transpose()
    }

    pub(crate) fn u16(&self, key: &str) -> Result<u16, JsError> {
        u16::try_from(self.u64(key)?).map_err(|_| JsError::new(&format!("{key} is out of range")))
    }
}

fn parse_id(s: &str, key: &str) -> Result<Identifier, JsError> {
    Identifier::from_string(s, Encoding::Base58)
        .map_err(|e| JsError::new(&format!("{key}: invalid identifier: {e}")))
}

pub(crate) fn id_string(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}
