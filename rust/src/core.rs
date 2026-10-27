//! Core-chain access through DAPI and asset locks, which turn Core DASH into Platform credits.
//!
//! DAPI has no address index, so funding outputs are named by the transaction that created them.
use dash_sdk::dapi_grpc::core::v0::BroadcastTransactionRequest;
use dash_sdk::dpp::bincode;
use dash_sdk::dpp::dashcore::blockdata::script::Builder;
use dash_sdk::dpp::dashcore::consensus::{deserialize, serialize};
use dash_sdk::dpp::dashcore::ecdsa::Signature;
use dash_sdk::dpp::dashcore::hashes::Hash;
use dash_sdk::dpp::dashcore::sighash::SighashCache;
use dash_sdk::dpp::dashcore::transaction::special_transaction::asset_lock::AssetLockPayload;
use dash_sdk::dpp::dashcore::transaction::special_transaction::TransactionPayload;
use dash_sdk::dpp::dashcore::{
    Address, EcdsaSighashType, OutPoint, PrivateKey, ScriptBuf, Transaction, TxIn, TxOut, Txid,
};
use dash_sdk::dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dash_sdk::dpp::prelude::AssetLockProof;
use dash_sdk::platform::dapi::{DapiRequestExecutor, IntoInner, RequestSettings};
use dash_sdk::Sdk;
use serde_json::{json, Value as Json};
use simple_signer::single_key_signer::SingleKeySigner;
use wasm_bindgen::JsError;

use crate::js;
use crate::params::Params;

type Result = std::result::Result<Json, JsError>;

/// Outputs below this are uneconomical to spend; their value goes to the fee instead.
const DUST: u64 = 1_000;
/// Fee rate in duffs per byte; Core's minimum relay fee is 1 duff/byte.
const FEE_PER_BYTE: u64 = 1;

fn private_key(p: &Params, sdk: &Sdk, key: &str) -> std::result::Result<PrivateKey, JsError> {
    SingleKeySigner::from_string(p.str(key)?, sdk.network)
        .map(|signer| signer.private_key().clone())
        .map_err(|e| JsError::new(&format!("{key}: {e}")))
}

fn txid(s: &str) -> std::result::Result<Txid, JsError> {
    s.parse().map_err(|e| JsError::new(&format!("invalid txid {s}: {e}")))
}

async fn transaction(sdk: &Sdk, id: &str) -> std::result::Result<dash_sdk::core::FetchedCoreTransaction, JsError> {
    sdk.get_transaction(id)
        .await
        .map_err(js)?
        .ok_or_else(|| JsError::new(&format!("transaction not found: {id}")))
}

pub(crate) async fn get_transaction(sdk: &Sdk, p: &Params) -> Result {
    let Some(tx) = sdk.get_transaction(p.str("txid")?).await.map_err(js)? else {
        return Ok(Json::Null);
    };
    Ok(json!({
        "hex": hex::encode(serialize(&tx.transaction)),
        "height": tx.height,
        "isInstantLocked": tx.is_instant_locked,
        "isChainLocked": tx.is_chain_locked,
    }))
}

async fn broadcast(sdk: &Sdk, tx: &Transaction) -> std::result::Result<String, JsError> {
    let request = BroadcastTransactionRequest {
        transaction: serialize(tx),
        allow_high_fees: false,
        bypass_limits: false,
    };
    let response = sdk
        .execute(request, RequestSettings::default())
        .await
        .into_inner()
        .map_err(|e| JsError::new(&format!("broadcast failed: {e}")))?;
    Ok(response.transaction_id)
}

pub(crate) async fn broadcast_transaction(sdk: &Sdk, p: &Params) -> Result {
    let tx: Transaction = deserialize(&hex::decode(p.str("hex")?).map_err(js)?).map_err(js)?;
    Ok(json!({ "txid": broadcast(sdk, &tx).await? }))
}

/// Locks `amount` duffs into Platform credits for `assetLockPrivateKey`, spending every output
/// of the `fundingTxids` transactions that pays `fundingPrivateKey`'s address and returning the
/// change there. Broadcasts the transaction; prove it with `assetLocks.proof` once it is
/// chain-locked. Without `amount`, locks everything but the fee.
pub(crate) async fn create_asset_lock(sdk: &Sdk, p: &Params) -> Result {
    let funding_key = private_key(p, sdk, "fundingPrivateKey")?;
    let lock_key = private_key(p, sdk, "assetLockPrivateKey")?;
    let funding_script = ScriptBuf::new_p2pkh(&funding_key.public_key().pubkey_hash());
    let txids = p
        .get("fundingTxids")
        .and_then(Json::as_array)
        .filter(|ids| !ids.is_empty())
        .ok_or_else(|| JsError::new("fundingTxids must be a non-empty array"))?;

    let mut spent = Vec::new();
    for id in txids {
        let id = id.as_str().ok_or_else(|| JsError::new("fundingTxids must hold strings"))?;
        let funding = transaction(sdk, id).await?;
        for (vout, out) in (0u32..).zip(&funding.transaction.output) {
            if out.script_pubkey == funding_script {
                spent.push((OutPoint::new(txid(id)?, vout), out.clone()));
            }
        }
    }
    if spent.is_empty() {
        return Err(JsError::new("no output of fundingTxids pays fundingPrivateKey's address"));
    }
    let available: u64 = spent.iter().map(|(_, out)| out.value).sum();

    // P2PKH inputs ~148 bytes, outputs ~34, payload with one credit output ~40, header ~12.
    let size = |outputs: u64| 12 + 148 * spent.len() as u64 + 34 * outputs + 40;
    let (amount, change) = match p.opt_u64("amount")? {
        Some(amount) => {
            let fee = size(2) * FEE_PER_BYTE;
            let change = available
                .checked_sub(amount + fee)
                .ok_or_else(|| JsError::new(&format!("funding holds {available} duffs; need {} with fee", amount + fee)))?;
            if change < DUST { (amount, 0) } else { (amount, change) }
        }
        None => (available.saturating_sub(size(1) * FEE_PER_BYTE), 0),
    };
    if amount < DUST {
        return Err(JsError::new("amount is too small to lock"));
    }

    let mut output = vec![TxOut {
        value: amount,
        script_pubkey: ScriptBuf::new_op_return(&[]),
    }];
    if change > 0 {
        output.push(TxOut { value: change, script_pubkey: funding_script.clone() });
    }
    let credit_output = TxOut {
        value: amount,
        script_pubkey: ScriptBuf::new_p2pkh(&lock_key.public_key().pubkey_hash()),
    };
    let mut tx = Transaction {
        version: 3,
        lock_time: 0,
        input: spent
            .iter()
            .map(|(outpoint, _)| TxIn { previous_output: *outpoint, ..Default::default() })
            .collect(),
        output,
        special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(AssetLockPayload {
            version: AssetLockPayload::CURRENT_VERSION,
            credit_outputs: vec![credit_output],
        })),
    };

    let public_key = funding_key.public_key();
    let script_sigs = {
        let cache = SighashCache::new(&tx);
        spent
            .iter()
            .enumerate()
            .map(|(index, (_, out))| {
                let sighash = cache
                    .legacy_signature_hash(index, &out.script_pubkey, EcdsaSighashType::All.to_u32())
                    .map_err(js)?;
                let signature = funding_key.inner.sign(&sighash.to_byte_array());
                Ok(Builder::new()
                    .push_slice(Signature::sighash_all(signature.into()).serialize())
                    .push_key(&public_key)
                    .into_script())
            })
            .collect::<std::result::Result<Vec<_>, JsError>>()?
    };
    for (input, script_sig) in tx.input.iter_mut().zip(script_sigs) {
        input.script_sig = script_sig;
    }

    let txid = broadcast(sdk, &tx).await?;
    Ok(json!({
        "txid": txid,
        "amount": amount.to_string(),
        "change": change.to_string(),
        "changeAddress": Address::p2pkh(&public_key, sdk.network).to_string(),
    }))
}

/// The proof for an asset lock's credit output once its block is chain-locked, as the hex that
/// `identities.create` and `identities.topUp` take; `null` until then.
pub(crate) async fn asset_lock_proof(sdk: &Sdk, p: &Params) -> Result {
    let id = p.str("txid")?;
    let tx = transaction(sdk, id).await?;
    if !tx.is_chain_locked || tx.height == 0 {
        return Ok(Json::Null);
    }
    let proof = AssetLockProof::Chain(ChainAssetLockProof {
        core_chain_locked_height: tx.height,
        out_point: OutPoint::new(txid(id)?, p.opt_u32("outputIndex")?.unwrap_or(0)),
    });
    let bytes = bincode::encode_to_vec(&proof, bincode::config::standard()).map_err(js)?;
    Ok(json!(hex::encode(bytes)))
}
