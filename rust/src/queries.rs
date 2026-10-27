//! Proved reads. Each returns JSON; amounts are decimal strings.
use std::sync::Arc;

use dash_sdk::dpp::identity::accessors::IdentityGettersV0;
use dash_sdk::dpp::identity::Identity;
use dash_sdk::dpp::platform_value::Value;
use dash_sdk::dpp::prelude::DataContract;
use dash_sdk::dpp::tokens::contract_info::v0::TokenContractInfoV0Accessors;
use dash_sdk::dpp::tokens::status::v0::TokenStatusV0Accessors;
use dash_sdk::dpp::tokens::status::TokenStatus;
use dash_sdk::drive::query::{OrderClause, WhereClause};
use dash_sdk::platform::proto::get_documents_request::get_documents_request_v0::Start;
use dash_sdk::platform::fetch_current_no_parameters::FetchCurrent;
use dash_sdk::platform::tokens::identity_token_balances::{
    IdentitiesTokenBalances, IdentitiesTokenBalancesQuery, IdentityTokenBalances,
    IdentityTokenBalancesQuery,
};
use dash_sdk::platform::types::identity::PublicKeyHash;
use dash_sdk::platform::{Document, DocumentQuery, Fetch, FetchMany, Identifier};
use dash_sdk::query_types::{
    IdentityBalance, IdentityBalances, IdentityContractNonceFetcher, IdentityNonceFetcher,
    TotalCreditsInPlatform,
};
use dash_sdk::dpp::balances::credits::TokenAmount;
use dash_sdk::dpp::balances::total_single_token_balance::TotalSingleTokenBalance;
use dash_sdk::dpp::block::extended_epoch_info::v0::ExtendedEpochInfoV0Getters;
use dash_sdk::dpp::block::extended_epoch_info::ExtendedEpochInfo;
use dash_sdk::dpp::tokens::contract_info::TokenContractInfo;
use dash_sdk::Sdk;
use serde_json::{json, Map, Value as Json};
use wasm_bindgen::JsError;

use crate::js;
use crate::params::{id_string, Params};

type Result = std::result::Result<Json, JsError>;

pub(crate) async fn fetch_identity(sdk: &Sdk, id: Identifier) -> std::result::Result<Identity, JsError> {
    Identity::fetch(sdk, id)
        .await
        .map_err(js)?
        .ok_or_else(|| JsError::new(&format!("identity not found: {}", id_string(&id))))
}

pub(crate) async fn fetch_contract(
    sdk: &Sdk,
    id: Identifier,
) -> std::result::Result<Arc<DataContract>, JsError> {
    DataContract::fetch(sdk, id)
        .await
        .map_err(js)?
        .map(Arc::new)
        .ok_or_else(|| JsError::new(&format!("contract not found: {}", id_string(&id))))
}

pub(crate) fn identity_json(identity: &Identity) -> Result {
    let mut value = serde_json::to_value(identity).map_err(js)?;
    // Balances can exceed 2^53, so they cross as strings like every other amount.
    value["balance"] = json!(identity.balance().to_string());
    Ok(value)
}

pub(crate) fn document_json(document: &Document) -> Result {
    serde_json::to_value(document).map_err(js)
}

fn amounts<K>(entries: impl IntoIterator<Item = (K, Option<u64>)>, key: impl Fn(&K) -> String) -> Json {
    Json::Object(
        entries
            .into_iter()
            .map(|(k, v)| (key(&k), v.map_or(Json::Null, |v| json!(v.to_string()))))
            .collect::<Map<_, _>>(),
    )
}

pub(crate) async fn identity(sdk: &Sdk, p: &Params) -> Result {
    match Identity::fetch(sdk, p.id("identityId")?).await.map_err(js)? {
        Some(identity) => identity_json(&identity),
        None => Ok(Json::Null),
    }
}

pub(crate) async fn identity_balance(sdk: &Sdk, p: &Params) -> Result {
    let balance = IdentityBalance::fetch(sdk, p.id("identityId")?).await.map_err(js)?;
    Ok(balance.map_or(Json::Null, |b| json!(b.to_string())))
}

pub(crate) async fn identity_balances(sdk: &Sdk, p: &Params) -> Result {
    let balances: IdentityBalances = IdentityBalance::fetch_many(sdk, p.ids("identityIds")?)
        .await
        .map_err(js)?;
    Ok(amounts(balances, id_string))
}

pub(crate) async fn identity_nonce(sdk: &Sdk, p: &Params) -> Result {
    let nonce = IdentityNonceFetcher::fetch(sdk, p.id("identityId")?)
        .await
        .map_err(js)?;
    Ok(nonce.map_or(Json::Null, |n| json!(n.0.to_string())))
}

pub(crate) async fn identity_contract_nonce(sdk: &Sdk, p: &Params) -> Result {
    let query = (p.id("identityId")?, p.id("contractId")?);
    let nonce = IdentityContractNonceFetcher::fetch(sdk, query)
        .await
        .map_err(js)?;
    Ok(nonce.map_or(Json::Null, |n| json!(n.0.to_string())))
}

pub(crate) async fn identity_by_public_key_hash(sdk: &Sdk, p: &Params) -> Result {
    let hash: [u8; 20] = hex::decode(p.str("publicKeyHash")?)
        .map_err(js)?
        .try_into()
        .map_err(|_| JsError::new("publicKeyHash must be 20 bytes of hex"))?;
    match Identity::fetch(sdk, PublicKeyHash(hash)).await.map_err(js)? {
        Some(identity) => identity_json(&identity),
        None => Ok(Json::Null),
    }
}

pub(crate) async fn dpns_resolve(sdk: &Sdk, p: &Params) -> Result {
    let id = sdk.resolve_dpns_name(p.str("name")?).await.map_err(js)?;
    Ok(id.map_or(Json::Null, |id| json!(id_string(&id))))
}

pub(crate) async fn dpns_usernames(sdk: &Sdk, p: &Params) -> Result {
    let names = sdk
        .get_dpns_usernames_by_identity(p.id("identityId")?, p.opt_u32("limit")?)
        .await
        .map_err(js)?;
    Ok(names.into_iter().map(|n| json!(n.full_name)).collect())
}

pub(crate) async fn dpns_available(sdk: &Sdk, p: &Params) -> Result {
    let available = sdk.is_dpns_name_available(p.str("name")?).await.map_err(js)?;
    Ok(json!(available))
}

pub(crate) async fn contract_json(sdk: &Sdk, p: &Params) -> Result {
    match DataContract::fetch(sdk, p.id("contractId")?).await.map_err(js)? {
        Some(contract) => serde_json::to_value(&contract).map_err(js),
        None => Ok(Json::Null),
    }
}

pub(crate) async fn contracts(sdk: &Sdk, p: &Params) -> Result {
    let contracts = DataContract::fetch_many(sdk, p.ids("contractIds")?)
        .await
        .map_err(js)?;
    let mut out = Map::new();
    for (id, contract) in contracts {
        let value = match contract {
            Some(c) => serde_json::to_value(&c).map_err(js)?,
            None => Json::Null,
        };
        out.insert(id_string(&id), value);
    }
    Ok(Json::Object(out))
}

/// Clauses use the evo-sdk array form: `where: [["field", "op", value]]`,
/// `orderBy: [["field", "asc"]]`.
fn clauses<T, E: std::fmt::Display>(
    p: &Params,
    key: &str,
    parse: impl Fn(&[Value]) -> std::result::Result<T, E>,
) -> std::result::Result<Vec<T>, JsError> {
    let Some(json) = p.get(key) else { return Ok(vec![]) };
    let Value::Array(clauses) = Value::from(json.clone()) else {
        return Err(JsError::new(&format!("{key} must be an array of clauses")));
    };
    clauses
        .into_iter()
        .map(|clause| match clause {
            Value::Array(parts) => parse(&parts).map_err(js),
            _ => Err(JsError::new(&format!("each {key} clause must be an array"))),
        })
        .collect()
}

async fn document_query(sdk: &Sdk, p: &Params) -> std::result::Result<DocumentQuery, JsError> {
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let mut query = DocumentQuery::new(contract, p.str("documentTypeName")?).map_err(js)?;
    query.where_clauses = clauses(p, "where", |parts: &[Value]| WhereClause::from_components(parts))?;
    query.order_by_clauses = clauses(p, "orderBy", OrderClause::from_components)?;
    if let Some(limit) = p.opt_u32("limit")? {
        query.limit = limit;
    }
    query.start = match (p.opt_str("startAfter")?, p.opt_str("startAt")?) {
        (Some(_), Some(_)) => return Err(JsError::new("pass startAfter or startAt, not both")),
        (Some(_), None) => Some(Start::StartAfter(p.id("startAfter")?.to_vec())),
        (None, Some(_)) => Some(Start::StartAt(p.id("startAt")?.to_vec())),
        (None, None) => None,
    };
    Ok(query)
}

pub(crate) async fn documents(sdk: &Sdk, p: &Params) -> Result {
    let documents = Document::fetch_many(sdk, document_query(sdk, p).await?)
        .await
        .map_err(js)?;
    documents
        .into_values()
        .flatten()
        .map(|d| document_json(&d))
        .collect()
}

pub(crate) async fn document(sdk: &Sdk, p: &Params) -> Result {
    let contract = fetch_contract(sdk, p.id("dataContractId")?).await?;
    let query = DocumentQuery::new(contract, p.str("documentTypeName")?)
        .map_err(js)?
        .with_document_id(&p.id("documentId")?);
    match Document::fetch(sdk, query).await.map_err(js)? {
        Some(document) => document_json(&document),
        None => Ok(Json::Null),
    }
}

pub(crate) async fn token_identity_balances(sdk: &Sdk, p: &Params) -> Result {
    let query = IdentityTokenBalancesQuery {
        identity_id: p.id("identityId")?,
        token_ids: p.ids("tokenIds")?,
    };
    let balances: IdentityTokenBalances = TokenAmount::fetch_many(sdk, query).await.map_err(js)?;
    Ok(amounts(balances.0, id_string))
}

pub(crate) async fn token_identities_balances(sdk: &Sdk, p: &Params) -> Result {
    let query = IdentitiesTokenBalancesQuery {
        identity_ids: p.ids("identityIds")?,
        token_id: p.id("tokenId")?,
    };
    let balances: IdentitiesTokenBalances = TokenAmount::fetch_many(sdk, query).await.map_err(js)?;
    Ok(amounts(balances.0, id_string))
}

pub(crate) async fn token_total_supply(sdk: &Sdk, p: &Params) -> Result {
    let supply = TotalSingleTokenBalance::fetch(sdk, p.id("tokenId")?)
        .await
        .map_err(js)?;
    Ok(supply.map_or(Json::Null, |s| {
        json!({
            "totalSupply": s.token_supply.to_string(),
            "totalBalances": s.aggregated_token_account_balances.to_string(),
        })
    }))
}

pub(crate) async fn token_statuses(sdk: &Sdk, p: &Params) -> Result {
    let statuses = TokenStatus::fetch_many(sdk, p.ids("tokenIds")?)
        .await
        .map_err(js)?;
    Ok(Json::Object(
        statuses
            .into_iter()
            .map(|(id, status)| {
                let value = status.map_or(Json::Null, |s| json!({ "paused": s.paused() }));
                (id_string(&id), value)
            })
            .collect(),
    ))
}

pub(crate) async fn token_contract_info(sdk: &Sdk, p: &Params) -> Result {
    let info = TokenContractInfo::fetch(sdk, p.id("tokenId")?)
        .await
        .map_err(js)?;
    Ok(info.map_or(Json::Null, |info| {
        json!({
            "contractId": id_string(&info.contract_id()),
            "tokenPosition": info.token_contract_position(),
        })
    }))
}

pub(crate) async fn current_epoch(sdk: &Sdk) -> Result {
    let epoch = ExtendedEpochInfo::fetch_current(sdk).await.map_err(js)?;
    Ok(json!({
        "index": epoch.index(),
        "firstBlockHeight": epoch.first_block_height(),
        "firstCoreBlockHeight": epoch.first_core_block_height(),
        "startTimeMs": epoch.first_block_time(),
        "feeMultiplierPermille": epoch.fee_multiplier_permille(),
        "protocolVersion": epoch.protocol_version(),
    }))
}

pub(crate) async fn total_credits(sdk: &Sdk) -> Result {
    let total = TotalCreditsInPlatform::fetch_current(sdk).await.map_err(js)?;
    Ok(json!(total.0.to_string()))
}
