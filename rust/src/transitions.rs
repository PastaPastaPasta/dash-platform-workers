//! Signed writes. Each waits for Platform's proved result and returns it as JSON.
//!
//! Callers pass `privateKey` (WIF or hex) and optionally `keyId`. Without `keyId` the identity
//! key the private key belongs to is used, among keys of the purpose the transition needs.
use std::collections::BTreeMap;

use dash_sdk::dpp::data_contract::accessors::v0::DataContractV0Getters;
use dash_sdk::dpp::data_contract::document_type::methods::DocumentTypeV0Methods;
use dash_sdk::dpp::bincode;
use dash_sdk::dpp::dashcore::{Address, PrivateKey};
use dash_sdk::dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dash_sdk::dpp::identity::{KeyType, SecurityLevel};
use dash_sdk::dpp::prelude::AssetLockProof;
use dash_sdk::platform::transition::put_identity::PutIdentity;
use dash_sdk::platform::transition::top_up_identity::TopUpIdentity;
use simple_signer::signer::SimpleSigner;
use dash_sdk::dpp::document::document_methods::DocumentMethodsV0;
use dash_sdk::dpp::document::{DocumentV0Getters, DocumentV0Setters};
use dash_sdk::dpp::identity::accessors::IdentityGettersV0;
use dash_sdk::dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dash_sdk::dpp::identity::signer::Signer;
use dash_sdk::dpp::identity::{Identity, IdentityPublicKey, Purpose};
use dash_sdk::dpp::platform_value::Value;
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
use dash_sdk::platform::transition::transfer::TransferToIdentity;
use dash_sdk::platform::transition::withdraw_from_identity::WithdrawFromIdentity;
use dash_sdk::platform::{Document, DocumentQuery, Fetch};
use dash_sdk::Sdk;
use serde_json::{json, Value as Json};
use simple_signer::single_key_signer::SingleKeySigner;
use wasm_bindgen::JsError;

use crate::js;
use crate::params::{id_string, Params};
use crate::queries::{document_json, fetch_contract, fetch_identity, identity_json};

type Result = std::result::Result<Json, JsError>;

/// The identity acting, the key it signs with, and a signer holding that key's private half.
struct Signing {
    identity: Identity,
    key: IdentityPublicKey,
    signer: SingleKeySigner,
}

async fn signing(sdk: &Sdk, p: &Params, owner: &str, purpose: Purpose) -> std::result::Result<Signing, JsError> {
    let identity = fetch_identity(sdk, p.id(owner)?).await?;
    let signer =
        SingleKeySigner::from_string(p.str("privateKey")?, sdk.network).map_err(|e| JsError::new(&e))?;
    let key = match p.opt_u32("keyId")? {
        Some(key_id) => identity
            .public_keys()
            .get(&key_id)
            .cloned()
            .ok_or_else(|| JsError::new(&format!("identity has no key {key_id}")))?,
        None => identity
            .public_keys()
            .values()
            .find(|k| k.purpose() == purpose && k.disabled_at().is_none() && signer.can_sign_with(k))
            .cloned()
            .ok_or_else(|| {
                JsError::new(&format!(
                    "privateKey matches no enabled {purpose:?} key of the identity; pass keyId"
                ))
            })?,
    };
    if !signer.can_sign_with(&key) {
        return Err(JsError::new(&format!("privateKey does not belong to key {}", key.id())));
    }
    Ok(Signing { identity, key, signer })
}

fn entropy() -> std::result::Result<[u8; 32], JsError> {
    let mut entropy = [0u8; 32];
    getrandom::getrandom(&mut entropy).map_err(js)?;
    Ok(entropy)
}

fn properties(p: &Params) -> std::result::Result<BTreeMap<String, Value>, JsError> {
    let data = p
        .get("data")
        .ok_or_else(|| JsError::new("missing parameter: data"))?;
    Value::from(data.clone())
        .into_btree_string_map()
        .map_err(|_| JsError::new("data must be an object"))
}

/// An asset lock proof as hex of its standard bincode encoding (evo-sdk's `AssetLockProof.toHex()`).
fn asset_lock(p: &Params, sdk: &Sdk) -> std::result::Result<(AssetLockProof, PrivateKey), JsError> {
    let bytes = hex::decode(p.str("assetLockProof")?).map_err(js)?;
    let (proof, _) = bincode::decode_from_slice::<AssetLockProof, _>(&bytes, bincode::config::standard())
        .map_err(|e| JsError::new(&format!("assetLockProof: {e}")))?;
    let key = SingleKeySigner::from_string(p.str("assetLockPrivateKey")?, sdk.network)
        .map_err(|e| JsError::new(&format!("assetLockPrivateKey: {e}")))?;
    Ok((proof, key.private_key().clone()))
}

fn enum_param<T: TryFrom<u8>>(value: &Json, names: &[&str], what: &str) -> std::result::Result<T, JsError> {
    let index = match value {
        Json::String(name) => names.iter().position(|n| n.eq_ignore_ascii_case(name)),
        Json::Number(n) => n.as_u64().map(|n| n as usize),
        _ => None,
    };
    index
        .and_then(|i| u8::try_from(i).ok())
        .and_then(|i| T::try_from(i).ok())
        .ok_or_else(|| JsError::new(&format!("{what} must be one of {}", names.join(", "))))
}

/// Registers a new identity funded by an asset lock. `keys` lists its secp256k1 keys in id
/// order: `{ privateKey, purpose, securityLevel }`, and must start with a MASTER key.
pub(crate) async fn identity_create(sdk: &Sdk, p: &Params) -> Result {
    let (proof, asset_lock_key) = asset_lock(p, sdk)?;
    let specs = p
        .get("keys")
        .and_then(Json::as_array)
        .filter(|keys| !keys.is_empty())
        .ok_or_else(|| JsError::new("keys must be a non-empty array"))?;
    let mut signer = SimpleSigner::default();
    let mut keys = BTreeMap::new();
    for (id, spec) in (0u32..).zip(specs) {
        let private_key = spec
            .get("privateKey")
            .and_then(Json::as_str)
            .ok_or_else(|| JsError::new(&format!("keys[{id}].privateKey is required")))
            .and_then(|k| SingleKeySigner::from_string(k, sdk.network).map_err(|e| JsError::new(&e)))?;
        let purpose: Purpose = enum_param(
            spec.get("purpose").unwrap_or(&json!("AUTHENTICATION")),
            &["AUTHENTICATION", "ENCRYPTION", "DECRYPTION", "TRANSFER", "SYSTEM", "VOTING", "OWNER"],
            "purpose",
        )?;
        let security_level: SecurityLevel = enum_param(
            spec.get("securityLevel").unwrap_or(&json!("HIGH")),
            &["MASTER", "CRITICAL", "HIGH", "MEDIUM"],
            "securityLevel",
        )?;
        let key: IdentityPublicKey = IdentityPublicKeyV0 {
            id,
            purpose,
            security_level,
            contract_bounds: None,
            key_type: KeyType::ECDSA_SECP256K1,
            read_only: false,
            data: private_key.private_key().public_key().to_bytes().into(),
            disabled_at: None,
        }
        .into();
        signer.add_identity_public_key(key.clone(), *private_key.private_key().inner.to_bytes());
        keys.insert(id, key);
    }
    let identity_id = proof.create_identifier().map_err(js)?;
    let identity = Identity::new_with_id_and_keys(identity_id, keys, sdk.version()).map_err(js)?;
    let identity = identity
        .put_to_platform_and_wait_for_response_with_private_key(sdk, proof, &asset_lock_key, &signer, None)
        .await
        .map_err(js)?;
    identity_json(&identity)
}

/// The public half of `privateKey`: compressed key, its hash160 (what `byPublicKeyHash` takes)
/// and its P2PKH address on the connected network.
pub(crate) fn key_derive(sdk: &Sdk, p: &Params) -> Result {
    let signer = SingleKeySigner::from_string(p.str("privateKey")?, sdk.network).map_err(|e| JsError::new(&e))?;
    let public_key = signer.private_key().public_key();
    Ok(json!({
        "publicKey": hex::encode(public_key.to_bytes()),
        "publicKeyHash": hex::encode(public_key.pubkey_hash()),
        "address": Address::p2pkh(&public_key, sdk.network).to_string(),
    }))
}

pub(crate) async fn identity_top_up(sdk: &Sdk, p: &Params) -> Result {
    let (proof, asset_lock_key) = asset_lock(p, sdk)?;
    let identity = fetch_identity(sdk, p.id("identityId")?).await?;
    let balance = identity
        .top_up_identity_with_private_key(sdk, proof, &asset_lock_key, None)
        .await
        .map_err(js)?;
    Ok(json!({ "balance": balance.to_string() }))
}

pub(crate) async fn credit_transfer(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "identityId", Purpose::TRANSFER).await?;
    let (sender, recipient) = s
        .identity
        .transfer_credits(sdk, p.id("recipientId")?, p.u64("amount")?, Some(&s.key), s.signer, None)
        .await
        .map_err(js)?;
    Ok(json!({ "senderBalance": sender.to_string(), "recipientBalance": recipient.to_string() }))
}

pub(crate) async fn credit_withdrawal(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "identityId", Purpose::TRANSFER).await?;
    let address = p
        .opt_str("toAddress")?
        .map(|a| {
            a.parse::<Address<_>>()
                .map_err(js)?
                .require_network(sdk.network)
                .map_err(js)
        })
        .transpose()?;
    let balance = s
        .identity
        .withdraw(
            sdk,
            address,
            p.u64("amount")?,
            p.opt_u32("coreFeePerByte")?,
            Some(&s.key),
            s.signer,
            None,
        )
        .await
        .map_err(js)?;
    Ok(json!({ "balance": balance.to_string() }))
}

pub(crate) async fn dpns_register(sdk: &Sdk, p: &Params) -> Result {
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

pub(crate) async fn document_create(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let type_name = p.str("documentTypeName")?;
    let entropy = entropy()?;
    let document = contract
        .document_type_for_name(type_name)
        .map_err(js)?
        .create_document_from_data(
            Value::from(properties(p)?),
            s.identity.id(),
            0,
            0,
            entropy,
            sdk.version(),
        )
        .map_err(js)?;
    let builder = DocumentCreateTransitionBuilder::new(contract, type_name.to_string(), document, entropy);
    match sdk.document_create(builder, &s.key, &s.signer).await.map_err(js)? {
        DocumentCreateResult::Document(d) => document_json(&d),
    }
}

/// The current version of a document the caller is about to change, which must be theirs.
async fn owned_document(sdk: &Sdk, p: &Params, s: &Signing) -> std::result::Result<Document, JsError> {
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let query = DocumentQuery::new(contract, p.str("documentTypeName")?)
        .map_err(js)?
        .with_document_id(&p.id("documentId")?);
    Document::fetch(sdk, query)
        .await
        .map_err(js)?
        .ok_or_else(|| JsError::new("document not found"))
        .and_then(|d| {
            if d.owner_id() == s.identity.id() {
                Ok(d)
            } else {
                Err(JsError::new("document is owned by another identity"))
            }
        })
}

pub(crate) async fn document_replace(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let mut document = owned_document(sdk, p, &s).await?;
    document.set_properties(properties(p)?);
    document.increment_revision().map_err(js)?;
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let builder =
        DocumentReplaceTransitionBuilder::new(contract, p.str("documentTypeName")?.to_string(), document);
    match sdk.document_replace(builder, &s.key, &s.signer).await.map_err(js)? {
        DocumentReplaceResult::Document(d) => document_json(&d),
    }
}

pub(crate) async fn document_delete(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let builder = DocumentDeleteTransitionBuilder::new(
        contract,
        p.str("documentTypeName")?.to_string(),
        p.id("documentId")?,
        s.identity.id(),
    );
    match sdk.document_delete(builder, &s.key, &s.signer).await.map_err(js)? {
        DocumentDeleteResult::Deleted(id) => Ok(json!({ "deleted": id_string(&id) })),
    }
}

pub(crate) async fn document_transfer(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "ownerId", Purpose::AUTHENTICATION).await?;
    let mut document = owned_document(sdk, p, &s).await?;
    document.increment_revision().map_err(js)?;
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let builder = DocumentTransferTransitionBuilder::new(
        contract,
        p.str("documentTypeName")?.to_string(),
        document,
        p.id("recipientId")?,
    );
    match sdk.document_transfer(builder, &s.key, &s.signer).await.map_err(js)? {
        DocumentTransferResult::Document(d) => document_json(&d),
    }
}

fn optional_document(document: Option<Document>) -> Result {
    document.as_ref().map_or(Ok(Json::Null), document_json)
}

pub(crate) async fn token_transfer(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "identityId", Purpose::AUTHENTICATION).await?;
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let mut builder = TokenTransferTransitionBuilder::new(
        contract,
        p.u16("tokenPosition")?,
        s.identity.id(),
        p.id("recipientId")?,
        p.u64("amount")?,
    );
    if let Some(note) = p.opt_str("publicNote")? {
        builder = builder.with_public_note(note.to_string());
    }
    Ok(match sdk.token_transfer(builder, &s.key, &s.signer).await.map_err(js)? {
        TransferResult::IdentitiesBalances(balances) => json!({
            "balances": balances
                .into_iter()
                .map(|(id, amount)| (id_string(&id), json!(amount.to_string())))
                .collect::<serde_json::Map<_, _>>(),
        }),
        TransferResult::HistoricalDocument(d) => json!({ "historicalDocument": document_json(&d)? }),
        TransferResult::GroupActionWithDocument(power, d) => {
            json!({ "groupPower": power, "document": optional_document(d)? })
        }
    })
}

pub(crate) async fn token_mint(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "identityId", Purpose::AUTHENTICATION).await?;
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let mut builder = TokenMintTransitionBuilder::new(
        contract,
        p.u16("tokenPosition")?,
        s.identity.id(),
        p.u64("amount")?,
    );
    if p.get("recipientId").is_some() {
        builder = builder.issued_to_identity_id(p.id("recipientId")?);
    }
    if let Some(note) = p.opt_str("publicNote")? {
        builder = builder.with_public_note(note.to_string());
    }
    Ok(match sdk.token_mint(builder, &s.key, &s.signer).await.map_err(js)? {
        MintResult::TokenBalance(id, amount) => {
            json!({ "recipientId": id_string(&id), "balance": amount.to_string() })
        }
        MintResult::HistoricalDocument(d) => json!({ "historicalDocument": document_json(&d)? }),
        MintResult::GroupActionWithDocument(power, d) => {
            json!({ "groupPower": power, "document": optional_document(d)? })
        }
        MintResult::GroupActionWithBalance(power, status, balance) => json!({
            "groupPower": power,
            "groupActionStatus": format!("{status:?}"),
            "balance": balance.map(|b| b.to_string()),
        }),
    })
}

pub(crate) async fn token_burn(sdk: &Sdk, p: &Params) -> Result {
    let s = signing(sdk, p, "identityId", Purpose::AUTHENTICATION).await?;
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let mut builder = TokenBurnTransitionBuilder::new(
        contract,
        p.u16("tokenPosition")?,
        s.identity.id(),
        p.u64("amount")?,
    );
    if let Some(note) = p.opt_str("publicNote")? {
        builder = builder.with_public_note(note.to_string());
    }
    Ok(match sdk.token_burn(builder, &s.key, &s.signer).await.map_err(js)? {
        BurnResult::TokenBalance(id, amount) => {
            json!({ "identityId": id_string(&id), "balance": amount.to_string() })
        }
        BurnResult::HistoricalDocument(d) => json!({ "historicalDocument": document_json(&d)? }),
        BurnResult::GroupActionWithDocument(power, d) => {
            json!({ "groupPower": power, "document": optional_document(d)? })
        }
        BurnResult::GroupActionWithBalance(power, status, balance) => json!({
            "groupPower": power,
            "groupActionStatus": format!("{status:?}"),
            "balance": balance.map(|b| b.to_string()),
        }),
    })
}
