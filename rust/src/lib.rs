//! Dash Platform for Cloudflare Workers: a curated `dash-sdk` surface behind one JSON entry point.
//!
//! Every `#[wasm_bindgen]` export is kept by the linker together with everything it reaches, so
//! the module exports only `connect` and `call`; `call` dispatches by method name. That keeps the
//! glue small and lets the bundle stay under the Workers Free plan's 3 MiB limit.
mod client;
mod core;
mod params;
mod queries;
mod transitions;

use params::Params;
use wasm_bindgen::prelude::*;

/// Errors cross into JS as `Error` objects carrying the SDK's message.
pub(crate) fn js<E: std::fmt::Display>(error: E) -> JsError {
    JsError::new(&error.to_string())
}

/// Connects to `network` ("mainnet" or "testnet") and loads quorum keys. Call once per isolate.
#[wasm_bindgen]
pub async fn connect(network: String) -> Result<(), JsError> {
    client::connect(&network).await
}

/// Runs `method` with JSON `params`, returning JSON. Credit and token amounts are decimal strings.
#[wasm_bindgen]
pub async fn call(method: String, params: String) -> Result<String, JsError> {
    let p = Params::parse(&params)?;
    let sdk = client::sdk()?;
    let sdk = &sdk;
    let result = match method.as_str() {
        "identities.fetch" => queries::identity(sdk, &p).await,
        "identities.balance" => queries::identity_balance(sdk, &p).await,
        "identities.balances" => queries::identity_balances(sdk, &p).await,
        "identities.nonce" => queries::identity_nonce(sdk, &p).await,
        "identities.contractNonce" => queries::identity_contract_nonce(sdk, &p).await,
        "identities.byPublicKeyHash" => queries::identity_by_public_key_hash(sdk, &p).await,
        "identities.create" => transitions::identity_create(sdk, &p).await,
        "identities.topUp" => transitions::identity_top_up(sdk, &p).await,
        "identities.creditTransfer" => transitions::credit_transfer(sdk, &p).await,
        "identities.creditWithdrawal" => transitions::credit_withdrawal(sdk, &p).await,

        "dpns.resolveName" => queries::dpns_resolve(sdk, &p).await,
        "dpns.usernames" => queries::dpns_usernames(sdk, &p).await,
        "dpns.isNameAvailable" => queries::dpns_available(sdk, &p).await,
        "dpns.registerName" => transitions::dpns_register(sdk, &p).await,

        "contracts.fetch" => queries::contract_json(sdk, &p).await,
        "contracts.fetchMany" => queries::contracts(sdk, &p).await,

        "documents.query" => queries::documents(sdk, &p).await,
        "documents.get" => queries::document(sdk, &p).await,
        "documents.create" => transitions::document_create(sdk, &p).await,
        "documents.replace" => transitions::document_replace(sdk, &p).await,
        "documents.delete" => transitions::document_delete(sdk, &p).await,
        "documents.transfer" => transitions::document_transfer(sdk, &p).await,

        "tokens.identityBalances" => queries::token_identity_balances(sdk, &p).await,
        "tokens.identitiesBalances" => queries::token_identities_balances(sdk, &p).await,
        "tokens.totalSupply" => queries::token_total_supply(sdk, &p).await,
        "tokens.statuses" => queries::token_statuses(sdk, &p).await,
        "tokens.contractInfo" => queries::token_contract_info(sdk, &p).await,
        "tokens.transfer" => transitions::token_transfer(sdk, &p).await,
        "tokens.mint" => transitions::token_mint(sdk, &p).await,
        "tokens.burn" => transitions::token_burn(sdk, &p).await,

        "core.getTransaction" => core::get_transaction(sdk, &p).await,
        "core.broadcastTransaction" => core::broadcast_transaction(sdk, &p).await,
        "assetLocks.create" => core::create_asset_lock(sdk, &p).await,
        "assetLocks.proof" => core::asset_lock_proof(sdk, &p).await,

        "keys.derive" => transitions::key_derive(sdk, &p),
        "epoch.current" => queries::current_epoch(sdk).await,
        "system.totalCredits" => queries::total_credits(sdk).await,

        _ => Err(JsError::new(&format!("unknown method: {method}"))),
    }?;
    Ok(result.to_string())
}
