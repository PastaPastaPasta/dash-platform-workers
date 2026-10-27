use std::cell::RefCell;
use std::num::NonZeroUsize;

use dash_sdk::dpp::dashcore::Network;
use dash_sdk::{Sdk, SdkBuilder};
use rs_sdk_trusted_context_provider::TrustedHttpContextProvider;
use wasm_bindgen::JsError;

use crate::js;

thread_local! {
    static SDK: RefCell<Option<Sdk>> = const { RefCell::new(None) };
}

pub(crate) async fn connect(network: &str) -> Result<(), JsError> {
    let (network, builder) = match network {
        "mainnet" => (Network::Mainnet, SdkBuilder::new_mainnet()),
        "testnet" => (Network::Testnet, SdkBuilder::new_testnet()),
        other => return Err(JsError::new(&format!("unsupported network: {other}"))),
    };
    let provider = TrustedHttpContextProvider::new(network, None, NonZeroUsize::new(100).unwrap())
        .map_err(js)?;
    provider.update_quorum_caches().await.map_err(js)?;
    let sdk = builder.with_context_provider(provider).build().map_err(js)?;
    SDK.with(|s| *s.borrow_mut() = Some(sdk));
    Ok(())
}

pub(crate) fn sdk() -> Result<Sdk, JsError> {
    SDK.with(|s| s.borrow().clone())
        .ok_or_else(|| JsError::new("not connected: call connect() first"))
}
