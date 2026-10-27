//! Proved reads. Each returns JSON; amounts are decimal strings.
use std::sync::Arc;

use dash_sdk::dpp::balances::credits::TokenAmount;
use dash_sdk::dpp::balances::total_single_token_balance::TotalSingleTokenBalance;
use dash_sdk::dpp::block::extended_epoch_info::v0::ExtendedEpochInfoV0Getters;
use dash_sdk::dpp::block::extended_epoch_info::ExtendedEpochInfo;
use dash_sdk::dpp::identity::accessors::IdentityGettersV0;
use dash_sdk::dpp::identity::Identity;
use dash_sdk::dpp::platform_value::Value;
use dash_sdk::dpp::prelude::DataContract;
use dash_sdk::dpp::tokens::contract_info::v0::TokenContractInfoV0Accessors;
use dash_sdk::dpp::tokens::contract_info::TokenContractInfo;
use dash_sdk::dpp::tokens::status::v0::TokenStatusV0Accessors;
use dash_sdk::dpp::tokens::status::TokenStatus;
use dash_sdk::drive::query::{OrderClause, WhereClause};
use dash_sdk::platform::fetch_current_no_parameters::FetchCurrent;
use dash_sdk::platform::proto::get_documents_request::get_documents_request_v0::Start;
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
use dash_sdk::Sdk;
use serde_json::{json, Value as Json};

use crate::client;
use crate::params::{id_string, Params};
use crate::{js, JsResult};

pub(crate) async fn fetch_identity(sdk: &Sdk, id: Identifier) -> JsResult<Identity> {
    Identity::fetch(sdk, id)
        .await
        .map_err(js)?
        .ok_or_else(|| js(format!("identity not found: {}", id_string(&id))))
}

/// The contract named by `dataContractId`.
pub(crate) async fn fetch_contract(sdk: &Sdk, p: &Params) -> JsResult<Arc<DataContract>> {
    let id = p.id("dataContractId")?;
    DataContract::fetch(sdk, id)
        .await
        .map_err(js)?
        .map(client::remember_contract)
        .ok_or_else(|| js(format!("contract not found: {}", id_string(&id))))
}

/// A query for the document named by `dataContractId`, `documentTypeName` and `documentId`.
pub(crate) async fn document_by_id(sdk: &Sdk, p: &Params) -> JsResult<DocumentQuery> {
    Ok(DocumentQuery::new(fetch_contract(sdk, p).await?, p.str("documentTypeName")?)
        .map_err(js)?
        .with_document_id(&p.id("documentId")?))
}

pub(crate) fn identity_json(identity: &Identity) -> JsResult {
    let mut value = serde_json::to_value(identity).map_err(js)?;
    value["balance"] = json!(identity.balance().to_string());
    Ok(value)
}

pub(crate) fn document_json(document: &Document) -> JsResult {
    serde_json::to_value(document).map_err(js)
}

/// `null`, or `to_json` of the value.
pub(crate) fn nullable<T>(value: Option<T>, to_json: impl FnOnce(T) -> JsResult) -> JsResult {
    value.map_or(Ok(Json::Null), to_json)
}

/// `{ id: value | null }` for per-identifier results.
fn by_id<V>(entries: impl IntoIterator<Item = (Identifier, Option<V>)>, f: impl Fn(V) -> Json) -> Json {
    Json::Object(
        entries
            .into_iter()
            .map(|(id, v)| (id_string(&id), v.map_or(Json::Null, &f)))
            .collect(),
    )
}

fn amount(v: u64) -> Json {
    json!(v.to_string())
}

pub(crate) async fn identity(sdk: &Sdk, p: &Params) -> JsResult {
    nullable(Identity::fetch(sdk, p.id("identityId")?).await.map_err(js)?, |i| identity_json(&i))
}

pub(crate) async fn identity_balance(sdk: &Sdk, p: &Params) -> JsResult {
    let balance = IdentityBalance::fetch(sdk, p.id("identityId")?).await.map_err(js)?;
    Ok(balance.map_or(Json::Null, amount))
}

pub(crate) async fn identity_balances(sdk: &Sdk, p: &Params) -> JsResult {
    let balances: IdentityBalances = IdentityBalance::fetch_many(sdk, p.ids("identityIds")?)
        .await
        .map_err(js)?;
    Ok(by_id(balances, amount))
}

pub(crate) async fn identity_nonce(sdk: &Sdk, p: &Params) -> JsResult {
    let nonce = IdentityNonceFetcher::fetch(sdk, p.id("identityId")?).await.map_err(js)?;
    Ok(nonce.map_or(Json::Null, |n| amount(n.0)))
}

pub(crate) async fn identity_contract_nonce(sdk: &Sdk, p: &Params) -> JsResult {
    let query = (p.id("identityId")?, p.id("contractId")?);
    let nonce = IdentityContractNonceFetcher::fetch(sdk, query).await.map_err(js)?;
    Ok(nonce.map_or(Json::Null, |n| amount(n.0)))
}

pub(crate) async fn identity_by_public_key_hash(sdk: &Sdk, p: &Params) -> JsResult {
    let hash: [u8; 20] = hex::decode(p.str("publicKeyHash")?)
        .map_err(js)?
        .try_into()
        .map_err(|_| js("publicKeyHash must be 20 bytes of hex"))?;
    nullable(Identity::fetch(sdk, PublicKeyHash(hash)).await.map_err(js)?, |i| identity_json(&i))
}

pub(crate) async fn dpns_resolve(sdk: &Sdk, p: &Params) -> JsResult {
    let id = sdk.resolve_dpns_name(p.str("name")?).await.map_err(js)?;
    Ok(id.map_or(Json::Null, |id| json!(id_string(&id))))
}

pub(crate) async fn dpns_usernames(sdk: &Sdk, p: &Params) -> JsResult {
    let names = sdk
        .get_dpns_usernames_by_identity(p.id("identityId")?, p.opt_u32("limit")?)
        .await
        .map_err(js)?;
    Ok(names.into_iter().map(|n| json!(n.full_name)).collect())
}

pub(crate) async fn dpns_available(sdk: &Sdk, p: &Params) -> JsResult {
    Ok(json!(sdk.is_dpns_name_available(p.str("name")?).await.map_err(js)?))
}

fn contract_json(contract: DataContract) -> JsResult {
    serde_json::to_value(&contract).map_err(js)
}

pub(crate) async fn contract(sdk: &Sdk, p: &Params) -> JsResult {
    nullable(DataContract::fetch(sdk, p.id("contractId")?).await.map_err(js)?, contract_json)
}

pub(crate) async fn contracts(sdk: &Sdk, p: &Params) -> JsResult {
    DataContract::fetch_many(sdk, p.ids("contractIds")?)
        .await
        .map_err(js)?
        .into_iter()
        .map(|(id, c)| Ok((id_string(&id), nullable(c, contract_json)?)))
        .collect::<JsResult<serde_json::Map<_, _>>>()
        .map(Json::Object)
}

/// Clauses use the evo-sdk array form: `where: [["field", "op", value]]`,
/// `orderBy: [["field", "asc"]]`.
fn clauses<T, E: std::fmt::Display>(
    p: &Params,
    key: &str,
    parse: impl Fn(&[Value]) -> Result<T, E>,
) -> JsResult<Vec<T>> {
    let Some(json) = p.get(key) else { return Ok(vec![]) };
    let Value::Array(clauses) = Value::from(json.clone()) else {
        return Err(js(format!("{key} must be an array of clauses")));
    };
    clauses
        .into_iter()
        .map(|clause| match clause {
            Value::Array(parts) => parse(&parts).map_err(js),
            _ => Err(js(format!("each {key} clause must be an array"))),
        })
        .collect()
}

pub(crate) async fn documents(sdk: &Sdk, p: &Params) -> JsResult {
    let mut query =
        DocumentQuery::new(fetch_contract(sdk, p).await?, p.str("documentTypeName")?).map_err(js)?;
    query.where_clauses = clauses(p, "where", |parts: &[Value]| WhereClause::from_components(parts))?;
    query.order_by_clauses = clauses(p, "orderBy", OrderClause::from_components)?;
    if let Some(limit) = p.opt_u32("limit")? {
        query.limit = limit;
    }
    query.start = match (p.opt_id("startAfter")?, p.opt_id("startAt")?) {
        (Some(_), Some(_)) => return Err(js("pass startAfter or startAt, not both")),
        (Some(id), None) => Some(Start::StartAfter(id.to_vec())),
        (None, Some(id)) => Some(Start::StartAt(id.to_vec())),
        (None, None) => None,
    };
    Document::fetch_many(sdk, query)
        .await
        .map_err(js)?
        .into_values()
        .flatten()
        .map(|d| document_json(&d))
        .collect()
}

pub(crate) async fn document(sdk: &Sdk, p: &Params) -> JsResult {
    let document = Document::fetch(sdk, document_by_id(sdk, p).await?).await.map_err(js)?;
    nullable(document, |d| document_json(&d))
}

pub(crate) async fn token_identity_balances(sdk: &Sdk, p: &Params) -> JsResult {
    let query = IdentityTokenBalancesQuery { identity_id: p.id("identityId")?, token_ids: p.ids("tokenIds")? };
    let balances: IdentityTokenBalances = TokenAmount::fetch_many(sdk, query).await.map_err(js)?;
    Ok(by_id(balances.0, amount))
}

pub(crate) async fn token_identities_balances(sdk: &Sdk, p: &Params) -> JsResult {
    let query = IdentitiesTokenBalancesQuery { identity_ids: p.ids("identityIds")?, token_id: p.id("tokenId")? };
    let balances: IdentitiesTokenBalances = TokenAmount::fetch_many(sdk, query).await.map_err(js)?;
    Ok(by_id(balances.0, amount))
}

pub(crate) async fn token_total_supply(sdk: &Sdk, p: &Params) -> JsResult {
    let supply = TotalSingleTokenBalance::fetch(sdk, p.id("tokenId")?).await.map_err(js)?;
    Ok(supply.map_or(Json::Null, |s| {
        json!({
            "totalSupply": s.token_supply.to_string(),
            "totalBalances": s.aggregated_token_account_balances.to_string(),
        })
    }))
}

pub(crate) async fn token_statuses(sdk: &Sdk, p: &Params) -> JsResult {
    let statuses = TokenStatus::fetch_many(sdk, p.ids("tokenIds")?).await.map_err(js)?;
    Ok(by_id(statuses, |s| json!({ "paused": s.paused() })))
}

pub(crate) async fn token_contract_info(sdk: &Sdk, p: &Params) -> JsResult {
    let info = TokenContractInfo::fetch(sdk, p.id("tokenId")?).await.map_err(js)?;
    Ok(info.map_or(Json::Null, |info| {
        json!({
            "contractId": id_string(&info.contract_id()),
            "tokenPosition": info.token_contract_position(),
        })
    }))
}

pub(crate) async fn current_epoch(sdk: &Sdk) -> JsResult {
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

pub(crate) async fn total_credits(sdk: &Sdk) -> JsResult {
    Ok(amount(TotalCreditsInPlatform::fetch_current(sdk).await.map_err(js)?.0))
}
