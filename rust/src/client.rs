use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::sync::Arc;

use dash_sdk::dpp::dashcore::Network;
use dash_sdk::dpp::data_contract::accessors::v0::DataContractV0Getters;
use dash_sdk::dpp::data_contract::accessors::v1::DataContractV1Getters;
use dash_sdk::dpp::prelude::DataContract;
use dash_sdk::dpp::tokens::calculate_token_id;
use dash_sdk::{Sdk, SdkBuilder};
use rs_sdk_trusted_context_provider::TrustedHttpContextProvider;

use crate::{js, JsResult};

thread_local! {
    static SDK: RefCell<Option<(Sdk, TrustedHttpContextProvider)>> = const { RefCell::new(None) };
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
    let sdk = builder.with_context_provider(provider.clone()).build().map_err(js)?;
    SDK.with(|s| *s.borrow_mut() = Some((sdk, provider)));
    Ok(())
}

pub(crate) fn sdk() -> JsResult<Sdk> {
    SDK.with(|s| s.borrow().as_ref().map(|(sdk, _)| sdk.clone()))
        .ok_or_else(|| js("not connected: call connect() first"))
}

/// Tells the context provider about a contract this isolate fetched, with its tokens'
/// configurations. Verifying the proof of a write's result looks both up there, and the
/// provider otherwise knows only the system contracts compiled into it.
pub(crate) fn remember_contract(contract: DataContract) -> Arc<DataContract> {
    SDK.with(|s| match s.borrow().as_ref() {
        Some((_, provider)) => {
            for (position, config) in contract.tokens() {
                let token_id = calculate_token_id(&contract.id().to_buffer(), *position);
                provider.add_known_token_configuration(token_id.into(), config.clone());
            }
            provider.add_known_contract_if_newer(contract)
        }
        None => Arc::new(contract),
    })
}
