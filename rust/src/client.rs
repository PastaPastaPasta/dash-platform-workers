use std::cell::RefCell;
use std::num::NonZeroUsize;

use dash_sdk::dpp::dashcore::Network;
use dash_sdk::{Sdk, SdkBuilder};
use rs_sdk_trusted_context_provider::TrustedHttpContextProvider;

use crate::{js, JsResult};

thread_local! {
    static SDK: RefCell<Option<Sdk>> = const { RefCell::new(None) };
}

/// Builds this isolate's SDK. Quorum keys are not preloaded: the provider fetches the one a
/// proof names the first time it is needed, so connecting costs no network round trip.
pub(crate) fn connect(network: &str) -> JsResult<()> {
    let (network, builder) = match network {
        "mainnet" => (Network::Mainnet, SdkBuilder::new_mainnet()),
        "testnet" => (Network::Testnet, SdkBuilder::new_testnet()),
        other => return Err(js(format!("unsupported network: {other}"))),
    };
    let provider = TrustedHttpContextProvider::new(network, None, NonZeroUsize::new(100).unwrap())
        .map_err(js)?;
    let sdk = builder.with_context_provider(provider).build().map_err(js)?;
    SDK.with(|s| *s.borrow_mut() = Some(sdk));
    Ok(())
}

pub(crate) fn sdk() -> JsResult<Sdk> {
    SDK.with(|s| s.borrow().clone())
        .ok_or_else(|| js("not connected: call connect() first"))
}
