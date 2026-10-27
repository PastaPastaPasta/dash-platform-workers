//! Core-chain access through DAPI and asset locks, which turn Core DASH into Platform credits.
//!
//! DAPI has no address index, so funding outputs are named by the transaction that created them.
//! Core data from DAPI is not proved: callers trust the evonode that answered, except where this
//! module checks it (a funding transaction must hash to the txid asked for).
use std::collections::BTreeSet;

use bip37_bloom_filter::{BloomFilter, BloomFilterData};
use dash_sdk::dapi_grpc::core::v0::{
    transactions_with_proofs_request::FromBlock, transactions_with_proofs_response::Responses,
    BroadcastTransactionRequest, GetBlockchainStatusRequest, TransactionsWithProofsRequest,
};
use dash_sdk::dapi_grpc::tonic::Streaming;
use dash_sdk::dpp::bincode;
use dash_sdk::dpp::dashcore::blockdata::script::Builder;
use dash_sdk::dpp::dashcore::consensus::{deserialize, serialize};
use dash_sdk::dpp::dashcore::ecdsa::Signature;
use dash_sdk::dpp::dashcore::hashes::Hash;
use dash_sdk::dpp::dashcore::sighash::SighashCache;
use dash_sdk::dpp::dashcore::transaction::special_transaction::asset_lock::AssetLockPayload;
use dash_sdk::dpp::dashcore::transaction::special_transaction::TransactionPayload;
use dash_sdk::dpp::dashcore::{
    Address, EcdsaSighashType, InstantLock, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid,
};
use dash_sdk::dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dash_sdk::dpp::identity::state_transition::asset_lock_proof::InstantAssetLockProof;
use dash_sdk::dpp::prelude::AssetLockProof;
use dash_sdk::platform::dapi::{DapiRequestExecutor, IntoInner, RequestSettings};
use dash_sdk::Sdk;
use futures::future::{select, Either};
use serde_json::{json, Value as Json};
use wasm_bindgen::prelude::*;

use crate::params::Params;
use crate::{js, JsResult};

/// Outputs below this are uneconomical to spend; their value goes to the fee instead.
const DUST: u64 = 1_000;
/// Fee rate in duffs per byte; Core's minimum relay fee is 1 duff/byte.
const FEE_PER_BYTE: u64 = 1;
/// The least Platform accepts from an asset lock, in duffs, for each use.
const MIN_LOCK_FOR_IDENTITY: u64 = 200_000;
const MIN_LOCK_FOR_TOP_UP: u64 = 50_000;
/// How long `assetLocks.create` waits for the InstantSend lock before leaving it to a chain lock.
const INSTANT_LOCK_WAIT_MS: i32 = 30_000;

async fn transaction(sdk: &Sdk, id: &str) -> JsResult<dash_sdk::core::FetchedCoreTransaction> {
    let txid: Txid = id.parse().map_err(|e| js(format!("invalid txid {id}: {e}")))?;
    let fetched = sdk
        .get_transaction(id)
        .await
        .map_err(js)?
        .ok_or_else(|| js(format!("transaction not found: {id}")))?;
    // DAPI's answer is unproved; the hash at least pins its content to what was asked for.
    if fetched.transaction.txid() != txid {
        return Err(js(format!("evonode returned a different transaction for {id}")));
    }
    Ok(fetched)
}

pub(crate) async fn get_transaction(sdk: &Sdk, p: &Params) -> JsResult {
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

/// Broadcasts `tx`, counting "already known" as success: the transaction is out either way.
async fn broadcast(sdk: &Sdk, tx: &Transaction) -> JsResult<()> {
    let request = BroadcastTransactionRequest {
        transaction: serialize(tx),
        allow_high_fees: false,
        bypass_limits: false,
    };
    match sdk.execute(request, RequestSettings::default()).await.into_inner() {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().to_lowercase().contains("already") => Ok(()),
        Err(e) => Err(js(format!("broadcast failed: {e}"))),
    }
}

pub(crate) async fn broadcast_transaction(sdk: &Sdk, p: &Params) -> JsResult {
    let tx: Transaction = deserialize(&hex::decode(p.str("hex")?).map_err(js)?).map_err(js)?;
    broadcast(sdk, &tx).await?;
    Ok(json!({ "txid": tx.txid().to_string() }))
}

/// Locks Core DASH into Platform credits for `assetLockPrivateKey`.
///
/// Spends every output of the `fundingTxids` transactions that pays `fundingPrivateKey`'s
/// address and returns the change there; without `amount` (duffs) it locks everything but the
/// fee. `use` ("identity", the default, or "topUp") sets the least amount Platform accepts.
/// Broadcasts, then waits briefly for the InstantSend lock and returns the proof built from it;
/// if none arrives, `proof` is null and `assetLocks.proof` gives a chain proof later.
pub(crate) async fn create_asset_lock(sdk: &Sdk, p: &Params) -> JsResult {
    let funding_key = p.signer("fundingPrivateKey", sdk.network)?.private_key().clone();
    let lock_key = p.signer("assetLockPrivateKey", sdk.network)?.private_key().clone();
    let funding_public_key = funding_key.public_key();
    let funding_script = ScriptBuf::new_p2pkh(&funding_public_key.pubkey_hash());
    let credit_script = ScriptBuf::new_p2pkh(&lock_key.public_key().pubkey_hash());
    let minimum = match p.opt_str("use")?.unwrap_or("identity") {
        "identity" => MIN_LOCK_FOR_IDENTITY,
        "topUp" => MIN_LOCK_FOR_TOP_UP,
        other => return Err(js(format!("use must be \"identity\" or \"topUp\", not {other}"))),
    };

    let mut spent = Vec::new();
    for id in p.strs("fundingTxids")?.into_iter().collect::<BTreeSet<_>>() {
        let funding = transaction(sdk, id).await?.transaction;
        let txid = funding.txid();
        for (vout, out) in (0u32..).zip(funding.output) {
            if out.script_pubkey == funding_script {
                spent.push((OutPoint::new(txid, vout), out));
            }
        }
    }
    if spent.is_empty() {
        return Err(js("no output of fundingTxids pays fundingPrivateKey's address"));
    }
    let available: u64 = spent.iter().map(|(_, out)| out.value).sum();

    // Compressed P2PKH inputs ~148 bytes, outputs ~34, one-credit-output payload ~40, header ~12.
    let fee = |outputs: u64| (12 + 148 * spent.len() as u64 + 34 * outputs + 40) * FEE_PER_BYTE;
    let (amount, change) = match p.opt_u64("amount")? {
        Some(amount) => {
            let needed = amount.checked_add(fee(2)).ok_or_else(|| js("amount is too large"))?;
            let change = available
                .checked_sub(needed)
                .ok_or_else(|| js(format!("funding holds {available} duffs; need {needed} with fee")))?;
            (amount, if change < DUST { 0 } else { change })
        }
        None => (available.saturating_sub(fee(1)), 0),
    };
    if amount < minimum {
        return Err(js(format!("Platform needs at least {minimum} duffs in this lock; it would hold {amount}")));
    }

    let mut output = vec![TxOut { value: amount, script_pubkey: ScriptBuf::new_op_return(&[]) }];
    if change > 0 {
        output.push(TxOut { value: change, script_pubkey: funding_script.clone() });
    }
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
            credit_outputs: vec![TxOut { value: amount, script_pubkey: credit_script.clone() }],
        })),
    };
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
                    .push_key(&funding_public_key)
                    .into_script())
            })
            .collect::<JsResult<Vec<_>>>()?
    };
    for (input, script_sig) in tx.input.iter_mut().zip(script_sigs) {
        input.script_sig = script_sig;
    }

    // Subscribe before broadcasting so the InstantSend lock cannot slip past. The filter names the
    // funding key (its input) as well as both addresses, so it matches however Core filters.
    let stream = instant_lock_stream(
        sdk,
        &[
            &credit_script.as_bytes()[3..23],
            &funding_script.as_bytes()[3..23],
            &funding_public_key.to_bytes(),
        ],
    )
    .await;
    broadcast(sdk, &tx).await?;
    let proof = match stream {
        Ok(stream) => match wait_for_instant_lock(stream, &tx).await {
            Some(lock) => Some(proof_hex(&AssetLockProof::Instant(InstantAssetLockProof {
                instant_lock: lock,
                transaction: tx.clone(),
                output_index: 0,
            }))?),
            None => None,
        },
        Err(_) => None,
    };
    Ok(json!({
        "txid": tx.txid().to_string(),
        "hex": hex::encode(serialize(&tx)),
        "amount": amount.to_string(),
        "change": change.to_string(),
        "changeAddress": Address::p2pkh(&funding_public_key, sdk.network).to_string(),
        "proof": proof,
    }))
}

async fn instant_lock_stream(sdk: &Sdk, elements: &[&[u8]]) -> JsResult<Streaming<dash_sdk::dapi_grpc::core::v0::TransactionsWithProofsResponse>> {
    let tip = sdk
        .execute(GetBlockchainStatusRequest {}, RequestSettings::default())
        .await
        .into_inner()
        .map_err(js)?
        .chain
        .ok_or_else(|| js("evonode reported no chain status"))?
        .best_block_hash;
    let filter = elements
        .iter()
        .fold(BloomFilter::builder(elements.len() as u32, 0.001).map_err(|_| js("bloom filter"))?, |b, e| b.add_element(e))
        .build();
    let BloomFilterData { v_data, n_hash_funcs, n_tweak, n_flags } = filter.into();
    let request = TransactionsWithProofsRequest {
        bloom_filter: Some(dash_sdk::dapi_grpc::core::v0::BloomFilter { v_data, n_hash_funcs, n_tweak, n_flags }),
        count: 0,
        send_transaction_hashes: true,
        from_block: Some(FromBlock::FromBlockHash(tip)),
    };
    sdk.execute(request, RequestSettings::default()).await.into_inner().map_err(js)
}

/// The InstantSend lock for `tx` from `stream`, or None if the stream ends or time runs out.
async fn wait_for_instant_lock(
    mut stream: Streaming<dash_sdk::dapi_grpc::core::v0::TransactionsWithProofsResponse>,
    tx: &Transaction,
) -> Option<InstantLock> {
    let txid = tx.txid();
    let watch = async move {
        while let Ok(Some(message)) = stream.message().await {
            if let Some(Responses::InstantSendLockMessages(locks)) = message.responses {
                for bytes in locks.messages {
                    if let Ok(lock) = deserialize::<InstantLock>(&bytes) {
                        if lock.txid == txid {
                            return Some(lock);
                        }
                    }
                }
            }
        }
        None
    };
    match select(Box::pin(watch), Box::pin(sleep(INSTANT_LOCK_WAIT_MS))).await {
        Either::Left((lock, _)) => lock,
        Either::Right(_) => None,
    }
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(callback: &js_sys::Function, ms: i32);
}

async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| set_timeout(&resolve, ms));
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

fn proof_hex(proof: &AssetLockProof) -> JsResult<String> {
    Ok(hex::encode(bincode::encode_to_vec(proof, bincode::config::standard()).map_err(js)?))
}

/// A chain proof for an asset lock's credit output once its block is chain-locked, as the hex
/// that `identities.create` and `identities.topUp` take; `null` until then. Platform may still
/// refuse it for a block or two, until its own view of Core reaches that height; retry then.
pub(crate) async fn asset_lock_proof(sdk: &Sdk, p: &Params) -> JsResult {
    let id = p.str("txid")?;
    let tx = transaction(sdk, id).await?;
    if !tx.is_chain_locked || tx.height == 0 {
        return Ok(Json::Null);
    }
    let proof = AssetLockProof::Chain(ChainAssetLockProof {
        core_chain_locked_height: tx.height,
        out_point: OutPoint::new(tx.transaction.txid(), p.opt_u32("outputIndex")?.unwrap_or(0)),
    });
    Ok(json!(proof_hex(&proof)?))
}
