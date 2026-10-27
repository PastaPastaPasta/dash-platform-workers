//! Signed writes. Each waits for Platform's proved result and returns it as JSON.
//!
//! Callers pass `privateKey` (WIF or hex) and optionally `keyId`. Without `keyId` the identity
//! key the private key belongs to is used, among the keys allowed to sign the transition.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::sync::Arc;

use dash_sdk::dpp::bincode;
use dash_sdk::dpp::data_contract::accessors::v0::DataContractV0Getters;
use dash_sdk::dpp::data_contract::document_type::methods::DocumentTypeV0Methods;
use dash_sdk::dpp::dashcore::{Address, PrivateKey};
use dash_sdk::dpp::document::document_methods::DocumentMethodsV0;
use dash_sdk::dpp::document::{DocumentV0Getters, DocumentV0Setters};
use dash_sdk::dpp::identity::accessors::IdentityGettersV0;
use dash_sdk::dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dash_sdk::dpp::identity::identity_public_key::methods::hash::IdentityPublicKeyHashMethodsV0;
use dash_sdk::dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dash_sdk::dpp::identity::signer::Signer;
use dash_sdk::dpp::identity::{Identity, IdentityPublicKey, KeyType, Purpose, SecurityLevel};
use dash_sdk::dpp::platform_value::Value;
use dash_sdk::dpp::prelude::{AssetLockProof, DataContract};
use dash_sdk::platform::documents::transitions::{
    DocumentCreateResult, DocumentCreateTransitionBuilder, DocumentDeleteResult,
    DocumentDeleteTransitionBuilder, DocumentReplaceResult, DocumentReplaceTransitionBuilder,
    DocumentTransferResult, DocumentTransferTransitionBuilder,
};
use dash_sdk::platform::dpns_usernames::RegisterDpnsNameInput;
use dash_sdk::platform::tokens::builders::burn::TokenBurnTransitionBuilder;
use dash_sdk::platform::tokens::builders::mint::TokenMintTransitionBuilder;
use dash_sdk::platform::tokens::builders::transfer::TokenTransferTransitionBuilder;
use dash_sdk::platform::tokens::transitions::burn::BurnResult;
use dash_sdk::platform::tokens::transitions::mint::MintResult;
use dash_sdk::platform::tokens::transitions::transfer::TransferResult;
use dash_sdk::platform::transition::put_identity::PutIdentity;
use dash_sdk::platform::transition::top_up_identity::TopUpIdentity;
use dash_sdk::platform::transition::transfer::TransferToIdentity;
use dash_sdk::platform::transition::withdraw_from_identity::WithdrawFromIdentity;
use dash_sdk::platform::types::identity::PublicKeyHash;
use dash_sdk::platform::{Document, Fetch};
use dash_sdk::Sdk;
use serde_json::{json, Value as Json};
use simple_signer::signer::SimpleSigner;
use simple_signer::single_key_signer::SingleKeySigner;

use crate::params::{id_string, key_signer, Params};
use crate::queries::{
    document_by_id, document_json, fetch_contract, fetch_identity, identity_json, nullable,
};
use crate::{js, JsResult};

/// The identity acting, the key it signs with, and a signer holding that key's private half.
struct Signing {
    identity: Identity,
    key: IdentityPublicKey,
    signer: SingleKeySigner,
}

/// Keys that may sign a transition of `purpose`: enabled ones, and for document and token
/// transitions (AUTHENTICATION) not the MASTER key, which Platform reserves for key updates.
fn may_sign(key: &IdentityPublicKey, purpose: Purpose) -> bool {
    key.purpose() == purpose
        && key.disabled_at().is_none()
        && !(purpose == Purpose::AUTHENTICATION && key.security_level() == SecurityLevel::MASTER)
}

async fn signing(sdk: &Sdk, p: &Params, owner: &str, purpose: Purpose) -> JsResult<Signing> {
    let identity = fetch_identity(sdk, p.id(owner)?).await?;
    let signer = p.signer("privateKey", sdk.network)?;
    let key = match p.opt_u32("keyId")? {
        Some(key_id) => {
            let key = identity
                .public_keys()
                .get(&key_id)
                .ok_or_else(|| js(format!("identity has no key {key_id}")))?;
            if !may_sign(key, purpose) {
                return Err(js(format!(
                    "key {key_id} cannot sign this: it needs an enabled non-master {purpose:?} key"
                )));
            }
            if !signer.can_sign_with(key) {
                return Err(js(format!("privateKey does not belong to key {key_id}")));
            }
            key.clone()
        }
        None => identity
            .public_keys()
            .values()
            .find(|k| may_sign(k, purpose) && signer.can_sign_with(k))
            .cloned()
            .ok_or_else(|| {
                js(format!("privateKey matches no enabled non-master {purpose:?} key of the identity"))
            })?,
    };
    Ok(Signing { identity, key, signer })
}

fn properties(p: &Params) -> JsResult<BTreeMap<String, Value>> {
    Value::from(p.required("data")?.clone())
        .into_btree_string_map()
        .map_err(|_| js("data must be an object"))
}

/// An asset lock proof as hex of its standard bincode encoding (what `assetLocks.proof` and
/// evo-sdk's `AssetLockProof.toHex()` produce), with the private key of its credit output.
fn asset_lock(p: &Params, sdk: &Sdk) -> JsResult<(AssetLockProof, PrivateKey)> {
    let bytes = hex::decode(p.str("assetLockProof")?).map_err(js)?;
    let (proof, _) =
        bincode::decode_from_slice::<AssetLockProof, _>(&bytes, bincode::config::standard())
            .map_err(|e| js(format!("assetLockProof: {e}")))?;
    let key = p.signer("assetLockPrivateKey", sdk.network)?;
    Ok((proof, key.private_key().clone()))
}

const PURPOSES: [&str; 7] =
    ["AUTHENTICATION", "ENCRYPTION", "DECRYPTION", "TRANSFER", "SYSTEM", "VOTING", "OWNER"];
const SECURITY_LEVELS: [&str; 4] = ["MASTER", "CRITICAL", "HIGH", "MEDIUM"];

fn enum_param<T: TryFrom<u8>>(value: &Json, names: &[&str], what: &str) -> JsResult<T> {
    let index = match value {
        Json::String(name) => names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .and_then(|i| u8::try_from(i).ok()),
        Json::Number(n) => n.as_u64().and_then(|n| u8::try_from(n).ok()),
        _ => None,
    };
    index
        .and_then(|i| T::try_from(i).ok())
        .ok_or_else(|| js(format!("{what} must be one of {}", names.join(", "))))
}

/// Security levels Platform accepts for each purpose on keys added at creation.
fn allowed_at_creation(purpose: Purpose, level: SecurityLevel) -> bool {
    match purpose {
        Purpose::AUTHENTICATION => true,
        Purpose::ENCRYPTION | Purpose::DECRYPTION => level == SecurityLevel::MEDIUM,
        Purpose::TRANSFER => level == SecurityLevel::CRITICAL,
        _ => false,
    }
}

/// Registers a new identity funded by an asset lock. `keys` lists its secp256k1 keys in id
/// order: `{ privateKey, purpose, securityLevel }`, exactly one of them the MASTER
/// AUTHENTICATION key. Keys are checked here first, because Platform charges the asset lock
/// for an identity it rejects on key structure.
pub(crate) async fn identity_create(sdk: &Sdk, p: &Params) -> JsResult {
    let (proof, asset_lock_key) = asset_lock(p, sdk)?;
    let specs = p
        .required("keys")?
        .as_array()
        .filter(|keys| !keys.is_empty())
        .ok_or_else(|| js("keys must be a non-empty array"))?;
    let mut signer = SimpleSigner::default();
    let mut keys = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (id, spec) in (0u32..).zip(specs) {
        let field = |name: &str| format!("keys[{id}].{name}");
        let private_key = spec
            .get("privateKey")
            .and_then(Json::as_str)
            .ok_or_else(|| js(format!("{} is required", field("privateKey"))))
            .and_then(|k| key_signer(k, sdk.network, &field("privateKey")))?;
        let purpose: Purpose = enum_param(
            spec.get("purpose").unwrap_or(&json!("AUTHENTICATION")),
            &PURPOSES,
            &field("purpose"),
        )?;
        let security_level: SecurityLevel = enum_param(
            spec.get("securityLevel").unwrap_or(&json!("HIGH")),
            &SECURITY_LEVELS,
            &field("securityLevel"),
        )?;
        if !allowed_at_creation(purpose, security_level) {
            return Err(js(format!("keys[{id}]: a {purpose:?} key cannot be {security_level:?}")));
        }
        let data = private_key.private_key().public_key().to_bytes();
        if !seen.insert(data.clone()) {
            return Err(js(format!("keys[{id}] repeats an earlier key")));
        }
        let key: IdentityPublicKey = IdentityPublicKeyV0 {
            id,
            purpose,
            security_level,
            contract_bounds: None,
            key_type: KeyType::ECDSA_SECP256K1,
            read_only: false,
            data: data.into(),
            disabled_at: None,
        }
        .into();
        signer.add_identity_public_key(key.clone(), *private_key.private_key().inner.to_bytes());
        keys.insert(id, key);
    }
    let masters = keys
        .values()
        .filter(|k| k.purpose() == Purpose::AUTHENTICATION && k.security_level() == SecurityLevel::MASTER)
        .count();
    if masters != 1 {
        return Err(js(format!("keys must hold exactly one MASTER AUTHENTICATION key, not {masters}")));
    }
    // Platform also charges the lock when a key already belongs to another identity.
    for key in keys.values() {
        let hash = key.public_key_hash().map_err(js)?;
        if Identity::fetch(sdk, PublicKeyHash(hash)).await.map_err(js)?.is_some() {
            return Err(js(format!("key {} already belongs to another identity", key.id())));
        }
    }
    let identity_id = proof.create_identifier().map_err(js)?;
    let identity = Identity::new_with_id_and_keys(identity_id, keys, sdk.version()).map_err(js)?;
    let identity = identity
        .put_to_platform_and_wait_for_response_with_private_key(sdk, proof, &asset_lock_key, &signer, None)
        .await
        .map_err(js)?;
    identity_json(&identity)
}

pub(crate) async fn identity_top_up(sdk: &Sdk, p: &Params) -> JsResult {
    let (proof, asset_lock_key) = asset_lock(p, sdk)?;
    let identity = fetch_identity(sdk, p.id("identityId")?).await?;
    let balance = identity
        .top_up_identity_with_private_key(sdk, proof, &asset_lock_key, None)
        .await
        .map_err(js)?;
    Ok(json!({ "balance": balance.to_string() }))
}

/// The public half of `privateKey`: compressed key, its hash160 (what `byPublicKeyHash` takes)
/// and its P2PKH address on the connected network.
pub(crate) fn key_derive(sdk: &Sdk, p: &Params) -> JsResult {
    let public_key = p.signer("privateKey", sdk.network)?.private_key().public_key();
    Ok(json!({
        "publicKey": hex::encode(public_key.to_bytes()),
        "publicKeyHash": hex::encode(public_key.pubkey_hash()),
        "address": Address::p2pkh(&public_key, sdk.network).to_string(),
    }))
}

pub(crate) async fn credit_transfer(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "identityId", Purpose::TRANSFER).await?;
    let (sender, recipient) = s
        .identity
        .transfer_credits(sdk, p.id("recipientId")?, p.u64("amount")?, Some(&s.key), s.signer, None)
        .await
        .map_err(js)?;
    Ok(json!({ "senderBalance": sender.to_string(), "recipientBalance": recipient.to_string() }))
}

pub(crate) async fn credit_withdrawal(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "identityId", Purpose::TRANSFER).await?;
    let address = p
        .opt_str("toAddress")?
        .map(|a| a.parse::<Address<_>>().map_err(js)?.require_network(sdk.network).map_err(js))
        .transpose()?;
    let balance = s
        .identity
        .withdraw(sdk, address, p.u64("amount")?, p.opt_u32("coreFeePerByte")?, Some(&s.key), s.signer, None)
        .await
        .map_err(js)?;
    Ok(json!({ "balance": balance.to_string() }))
}

pub(crate) async fn dpns_register(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "identityId", Purpose::AUTHENTICATION).await?;
    let result = sdk
        .register_dpns_name(RegisterDpnsNameInput {
            label: p.str("label")?.to_string(),
            identity: s.identity,
            identity_public_key: s.key,
            signer: s.signer,
            preorder_callback: None,
            contest_fund: p.opt_u64("contestFund")?,
        })
        .await
        .map_err(js)?;
    Ok(json!({
        "name": result.full_domain_name,
        "domainDocument": document_json(&result.domain_document)?,
    }))
}

pub(crate) async fn document_create(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let contract = fetch_contract(sdk, p).await?;
    let type_name = p.str("documentTypeName")?;
    let mut entropy = [0u8; 32];
    getrandom::getrandom(&mut entropy).map_err(js)?;
    let document = contract
        .document_type_for_name(type_name)
        .map_err(js)?
        .create_document_from_data(Value::from(properties(p)?), s.identity.id(), 0, 0, entropy, sdk.version())
        .map_err(js)?;
    let builder = DocumentCreateTransitionBuilder::new(contract, type_name.to_string(), document, entropy);
    let DocumentCreateResult::Document(d) =
        sdk.document_create(builder, &s.key, &s.signer).await.map_err(js)?;
    document_json(&d)
}

/// The current version of a document the caller is about to change, which must be theirs,
/// with its contract.
async fn owned_document(sdk: &Sdk, p: &Params, s: &Signing) -> JsResult<(Arc<DataContract>, Document)> {
    let query = document_by_id(sdk, p).await?;
    let contract = query.data_contract.clone();
    let document = Document::fetch(sdk, query)
        .await
        .map_err(js)?
        .ok_or_else(|| js("document not found"))?;
    if document.owner_id() != s.identity.id() {
        return Err(js("document is owned by another identity"));
    }
    Ok((contract, document))
}

pub(crate) async fn document_replace(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let (contract, mut document) = owned_document(sdk, p, &s).await?;
    document.set_properties(properties(p)?);
    document.increment_revision().map_err(js)?;
    let builder =
        DocumentReplaceTransitionBuilder::new(contract, p.str("documentTypeName")?.to_string(), document);
    let DocumentReplaceResult::Document(d) =
        sdk.document_replace(builder, &s.key, &s.signer).await.map_err(js)?;
    document_json(&d)
}

pub(crate) async fn document_delete(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let builder = DocumentDeleteTransitionBuilder::new(
        fetch_contract(sdk, p).await?,
        p.str("documentTypeName")?.to_string(),
        p.id("documentId")?,
        s.identity.id(),
    );
    let DocumentDeleteResult::Deleted(id) =
        sdk.document_delete(builder, &s.key, &s.signer).await.map_err(js)?;
    Ok(json!({ "deleted": id_string(&id) }))
}

pub(crate) async fn document_transfer(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let (contract, mut document) = owned_document(sdk, p, &s).await?;
    document.increment_revision().map_err(js)?;
    let builder = DocumentTransferTransitionBuilder::new(
        contract,
        p.str("documentTypeName")?.to_string(),
        document,
        p.id("recipientId")?,
    );
    let DocumentTransferResult::Document(d) =
        sdk.document_transfer(builder, &s.key, &s.signer).await.map_err(js)?;
    document_json(&d)
}

// Token transitions answer with one of a few shapes, shared across their result enums.
fn historical(document: &Document) -> JsResult {
    Ok(json!({ "historicalDocument": document_json(document)? }))
}

fn group_document(power: impl Into<Json>, document: Option<Document>) -> JsResult {
    Ok(json!({ "groupPower": power.into(), "document": nullable(document, |d| document_json(&d))? }))
}

fn group_balance(power: impl Into<Json>, status: impl Debug, balance: Option<u64>) -> JsResult {
    Ok(json!({
        "groupPower": power.into(),
        "groupActionStatus": format!("{status:?}"),
        "balance": balance.map(|b| b.to_string()),
    }))
}

pub(crate) async fn token_transfer(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "identityId", Purpose::AUTHENTICATION).await?;
    let mut builder = TokenTransferTransitionBuilder::new(
        fetch_contract(sdk, p).await?,
        p.u16("tokenPosition")?,
        s.identity.id(),
        p.id("recipientId")?,
        p.u64("amount")?,
    );
    if let Some(note) = p.opt_str("publicNote")? {
        builder = builder.with_public_note(note.to_string());
    }
    match sdk.token_transfer(builder, &s.key, &s.signer).await.map_err(js)? {
        TransferResult::IdentitiesBalances(balances) => Ok(json!({
            "balances": balances
                .into_iter()
                .map(|(id, amount)| (id_string(&id), json!(amount.to_string())))
                .collect::<serde_json::Map<_, _>>(),
        })),
        TransferResult::HistoricalDocument(d) => historical(&d),
        TransferResult::GroupActionWithDocument(power, d) => group_document(power, d),
    }
}

pub(crate) async fn token_mint(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "identityId", Purpose::AUTHENTICATION).await?;
    let mut builder = TokenMintTransitionBuilder::new(
        fetch_contract(sdk, p).await?,
        p.u16("tokenPosition")?,
        s.identity.id(),
        p.u64("amount")?,
    );
    if let Some(recipient) = p.opt_id("recipientId")? {
        builder = builder.issued_to_identity_id(recipient);
    }
    if let Some(note) = p.opt_str("publicNote")? {
        builder = builder.with_public_note(note.to_string());
    }
    match sdk.token_mint(builder, &s.key, &s.signer).await.map_err(js)? {
        MintResult::TokenBalance(id, amount) => {
            Ok(json!({ "recipientId": id_string(&id), "balance": amount.to_string() }))
        }
        MintResult::HistoricalDocument(d) => historical(&d),
        MintResult::GroupActionWithDocument(power, d) => group_document(power, d),
        MintResult::GroupActionWithBalance(power, status, balance) => {
            group_balance(power, status, balance)
        }
    }
}

pub(crate) async fn token_burn(sdk: &Sdk, p: &Params) -> JsResult {
    let s = signing(sdk, p, "identityId", Purpose::AUTHENTICATION).await?;
    let mut builder = TokenBurnTransitionBuilder::new(
        fetch_contract(sdk, p).await?,
        p.u16("tokenPosition")?,
        s.identity.id(),
        p.u64("amount")?,
    );
    if let Some(note) = p.opt_str("publicNote")? {
        builder = builder.with_public_note(note.to_string());
    }
    match sdk.token_burn(builder, &s.key, &s.signer).await.map_err(js)? {
        BurnResult::TokenBalance(id, amount) => {
            Ok(json!({ "identityId": id_string(&id), "balance": amount.to_string() }))
        }
        BurnResult::HistoricalDocument(d) => historical(&d),
        BurnResult::GroupActionWithDocument(power, d) => group_document(power, d),
        BurnResult::GroupActionWithBalance(power, status, balance) => {
            group_balance(power, status, balance)
        }
    }
}
