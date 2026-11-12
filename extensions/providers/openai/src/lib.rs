//! Organization costs are observations, not a prepaid balance or an authentication test of the model key.
#![forbid(unsafe_code)]
use async_trait::async_trait;
use orch_provider_contract::builtins::{BuiltinSource, VendorClient};
use orch_provider_contract::source::{ADMIN_KEY_FIELD, AdminCredential, SourceDescriptorFacts};
use orch_provider_contract::{
    FactMember, FactsReport, FactsSource, SourceAsk, SourceDescriptor, SourceError, SourceErrorCode,
};
use orch_types::{Clock, Hash, Json};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

pub mod capabilities;

pub const NAME: &str = "openai";
pub const DOCUMENTATION: &str = "https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage/methods/costs";
// The outer source deadline and one-MiB reply limit still apply; repeated/endless cursors are refused.
const MAX_PAGES: usize = 8;

#[must_use]
pub fn known_vendor(base: &Url) -> bool {
    base.scheme() == "https"
        && base.host_str() == Some("api.openai.com")
        && base.port_or_known_default() == Some(443)
        && base.username().is_empty()
        && base.password().is_none()
        && base.query().is_none()
        && base.fragment().is_none()
        && matches!(base.path().trim_end_matches('/'), "" | "/v1")
}

#[must_use]
pub fn descriptor() -> SourceDescriptor {
    let mut source = SourceDescriptor::new(
        NAME,
        "OpenAI",
        "Reads daily organization costs by project and line item with a separate admin key; no prepaid balance is offered.",
    );
    source.facts = Some(SourceDescriptorFacts {
        reports: vec![FactMember::Spend],
        ..Default::default()
    });
    source.set_admin_credential(AdminCredential {
        labels: BTreeMap::from([
            ("en".into(), "Admin key".into()),
            ("zh-CN".into(), "管理密钥".into()),
        ]),
        reports: vec![FactMember::Spend],
    });
    source.summary = "Daily organization costs".into();
    source.summary_locales = BTreeMap::from([
        ("en".into(), "Daily organization costs".into()),
        ("zh-CN".into(), "每日组织费用".into()),
    ]);
    source.docs_url = Some(DOCUMENTATION.into());
    let _ = source.extensions.insert(
        "x_capability_declarations",
        json!(capabilities::declarations()),
    );
    source
}

pub struct OpenAi {
    client: Arc<dyn VendorClient>,
    clock: Arc<dyn Clock>,
}
impl OpenAi {
    #[must_use]
    pub fn new(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Self {
        Self { client, clock }
    }
}
fn malformed(message: &str) -> SourceError {
    SourceError::new(SourceErrorCode::Malformed, message)
}

/// Decimal text is kept in the vendor's unit; no floating-point sum or absent-value zero is invented.
pub fn cost_rows(page: &Json) -> Result<Vec<Json>, SourceError> {
    let buckets = page
        .get("data")
        .and_then(Json::as_array)
        .ok_or_else(|| malformed("the cost report has no bucket list"))?;
    let mut rows = Vec::new();
    for bucket in buckets {
        let start = bucket
            .get("start_time")
            .and_then(Json::as_i64)
            .ok_or_else(|| malformed("a cost bucket has no start"))?;
        let end = bucket
            .get("end_time")
            .and_then(Json::as_i64)
            .ok_or_else(|| malformed("a cost bucket has no end"))?;
        let results = bucket
            .get("results")
            .and_then(Json::as_array)
            .ok_or_else(|| malformed("a cost bucket has no result list"))?;
        for result in results {
            let amount = result
                .pointer("/amount/value")
                .ok_or_else(|| malformed("a cost row has no amount"))?;
            let used = match amount {
                Json::String(text) if orch_provider_contract::limits::is_decimal(text) => {
                    Some(text.clone())
                }
                Json::Number(number) => {
                    orch_provider_contract::limits::decimal_number(&number.to_string())
                }
                _ => None,
            }
            .ok_or_else(|| malformed("a cost row has no decimal amount"))?;
            let unit = result
                .pointer("/amount/currency")
                .and_then(Json::as_str)
                .filter(|unit| !unit.is_empty())
                .ok_or_else(|| malformed("a cost row has no currency"))?;
            let project = result.get("project_id").and_then(Json::as_str);
            let line = result.get("line_item").and_then(Json::as_str);
            let identity = json!([start, project, line]);
            let label = [project, line]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            rows.push(json!({ "id":format!("day:{start}:{}",Hash::of(identity.to_string().as_bytes())), "label":if label.is_empty(){None}else{Some(label.chars().take(orch_provider_contract::limits::TEXT_MAX_CHARS).collect::<String>())}, "unit":unit.to_uppercase(), "period":"day", "used":used, "resets_at":chrono::DateTime::from_timestamp(end,0).map(orch_types::Time::new), "x_project_id":project, "x_line_item":line, "x_bucket_start":start, "x_bucket_end":end }));
        }
    }
    Ok(rows)
}

#[async_trait]
impl FactsSource for OpenAi {
    fn describe(&self) -> SourceDescriptor {
        descriptor()
    }
    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        let Some(admin) = ask
            .secrets
            .get(ADMIN_KEY_FIELD)
            .filter(|key| !key.expose().is_empty())
        else {
            return Ok(FactsReport(json!({})));
        };
        let base = ask
            .endpoints
            .iter()
            .filter_map(|endpoint| Url::parse(&endpoint.base_url).ok())
            .find(known_vendor)
            .ok_or_else(|| SourceError::unsupported("an OpenAI organization endpoint"))?;
        let from = self
            .clock
            .now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| malformed("the observation date is unavailable"))?
            .and_utc()
            .timestamp();
        let mut url = base
            .join("/v1/organization/costs")
            .map_err(|_| malformed("the cost endpoint is unavailable"))?;
        url.query_pairs_mut()
            .append_pair("start_time", &from.to_string())
            .append_pair("bucket_width", "1d")
            .append_pair("limit", "1")
            .append_pair("group_by", "project_id")
            .append_pair("group_by", "line_item");
        let initial = url.clone();
        let mut seen = BTreeSet::new();
        let mut rows = Vec::new();
        for _ in 0..MAX_PAGES {
            let path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
            let page = self
                .client
                .get(
                    &base,
                    &path,
                    admin,
                    orch_provider_contract::limits::REPORT_MAX_BYTES,
                )
                .await
                .ok_or_else(|| {
                    SourceError::new(
                        SourceErrorCode::NotTold,
                        "the organization did not return its cost report",
                    )
                })?;
            rows.extend(cost_rows(&page)?);
            if page.get("has_more").and_then(Json::as_bool) != Some(true) {
                return Ok(FactsReport(json!({"spend":rows})));
            }
            let cursor = page
                .get("next_page")
                .and_then(Json::as_str)
                .filter(|cursor| !cursor.is_empty())
                .ok_or_else(|| malformed("a cost page has no next cursor"))?;
            if !seen.insert(cursor.to_owned()) {
                return Err(malformed("a cost cursor repeated"));
            }
            url = initial.clone();
            url.query_pairs_mut().append_pair("page", cursor);
        }
        Err(malformed("the cost report exceeded its bounded page read"))
    }
}
fn made(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
    Arc::new(OpenAi::new(client, clock))
}
#[must_use]
pub fn builtin() -> BuiltinSource {
    BuiltinSource {
        name: NAME,
        hosts: &["api.openai.com"],
        descriptor,
        matches: known_vendor,
        make: made,
    }
}
