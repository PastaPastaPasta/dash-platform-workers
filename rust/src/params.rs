use dash_sdk::dpp::dashcore::Network;
use dash_sdk::dpp::platform_value::string_encoding::Encoding;
use dash_sdk::platform::Identifier;
use serde_json::{Map, Value};
use simple_signer::single_key_signer::SingleKeySigner;

use crate::{js, JsResult};

/// A method's JSON parameters, with typed accessors that name the missing or malformed field.
pub(crate) struct Params(Map<String, Value>);

impl Params {
    pub(crate) fn parse(json: &str) -> JsResult<Self> {
        match serde_json::from_str(json).map_err(js)? {
            Value::Object(map) => Ok(Self(map)),
            Value::Null => Ok(Self(Map::new())),
            _ => Err(js("params must be a JSON object")),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    pub(crate) fn required(&self, key: &str) -> JsResult<&Value> {
        self.get(key).ok_or_else(|| js(format!("missing parameter: {key}")))
    }

    pub(crate) fn str(&self, key: &str) -> JsResult<&str> {
        self.required(key)?
            .as_str()
            .ok_or_else(|| js(format!("{key} must be a string")))
    }

    pub(crate) fn opt_str(&self, key: &str) -> JsResult<Option<&str>> {
        self.get(key).map(|_| self.str(key)).transpose()
    }

    pub(crate) fn id(&self, key: &str) -> JsResult<Identifier> {
        parse_id(self.str(key)?, key)
    }

    pub(crate) fn opt_id(&self, key: &str) -> JsResult<Option<Identifier>> {
        self.get(key).map(|_| self.id(key)).transpose()
    }

    pub(crate) fn ids(&self, key: &str) -> JsResult<Vec<Identifier>> {
        self.strs(key)?.into_iter().map(|s| parse_id(s, key)).collect()
    }

    pub(crate) fn strs(&self, key: &str) -> JsResult<Vec<&str>> {
        self.required(key)?
            .as_array()
            .filter(|items| !items.is_empty())
            .ok_or_else(|| js(format!("{key} must be a non-empty array")))?
            .iter()
            .map(|v| v.as_str().ok_or_else(|| js(format!("{key} must hold strings"))))
            .collect()
    }

    /// Amounts arrive as numbers or decimal strings, since JS numbers lose precision past 2^53.
    pub(crate) fn u64(&self, key: &str) -> JsResult<u64> {
        let value = self.required(key)?;
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| js(format!("{key} must be a non-negative integer")))
    }

    pub(crate) fn opt_u64(&self, key: &str) -> JsResult<Option<u64>> {
        self.get(key).map(|_| self.u64(key)).transpose()
    }

    pub(crate) fn opt_u32(&self, key: &str) -> JsResult<Option<u32>> {
        self.opt_u64(key)?.map(|v| narrow(v, key)).transpose()
    }

    pub(crate) fn u16(&self, key: &str) -> JsResult<u16> {
        narrow(self.u64(key)?, key)
    }

    /// A signer for the WIF or hex private key in `key`.
    pub(crate) fn signer(&self, key: &str, network: Network) -> JsResult<SingleKeySigner> {
        key_signer(self.str(key)?, network, key)
    }
}

pub(crate) fn key_signer(private_key: &str, network: Network, what: &str) -> JsResult<SingleKeySigner> {
    let signer = SingleKeySigner::from_string(private_key, network).map_err(|e| js(format!("{what}: {e}")))?;
    // Platform keys and P2PKH outputs here are built from the 33-byte compressed public key.
    if !signer.private_key().compressed {
        return Err(js(format!("{what}: uncompressed keys are not supported")));
    }
    Ok(signer)
}

fn narrow<T: TryFrom<u64>>(value: u64, key: &str) -> JsResult<T> {
    T::try_from(value).map_err(|_| js(format!("{key} is out of range")))
}

fn parse_id(s: &str, key: &str) -> JsResult<Identifier> {
    Identifier::from_string(s, Encoding::Base58)
        .map_err(|e| js(format!("{key}: invalid identifier: {e}")))
}

pub(crate) fn id_string(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}
