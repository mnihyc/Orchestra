//! Organization analytics and spend limits are source facts, never model credentials or a prepaid balance.
#![forbid(unsafe_code)]
use async_trait::async_trait;
use orch_plugin_contract::Secret;
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

pub const NAME: &str = "anthropic";
pub const DOCUMENTATION: &str = "https://platform.claude.com/docs/en/manage-claude/usage-cost-api";
pub const LIMITS_DOCUMENTATION: &str =
    "https://platform.claude.com/docs/en/manage-claude/spend-limits-api";
const MAX_PAGES: usize = 8;

#[must_use]
pub fn known_vendor(base: &Url) -> bool {
    base.scheme() == "https"
        && base.host_str() == Some("api.anthropic.com")
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
        "Anthropic",
        "Reads organization costs and optional spend limits with a separate admin key. Amounts remain in their reported minor units; Priority Tier costs are not included. No prepaid balance is offered.",
    );
    source.facts = Some(SourceDescriptorFacts {
        reports: vec![FactMember::Identity, FactMember::Spend, FactMember::Windows],
        ..Default::default()
    });
    source.set_admin_credential(AdminCredential {
        labels: BTreeMap::from([
            ("en".into(), "Admin key".into()),
            ("zh-CN".into(), "管理密钥".into()),
        ]),
        reports: vec![FactMember::Identity, FactMember::Spend, FactMember::Windows],
    });
    source.summary_locales = BTreeMap::from([
        ("en".into(), source.summary.clone()),
        ("zh-CN".into(), "使用单独的管理密钥读取组织费用与可用的支出限额；金额保留提供方报告的最小货币单位，不包含 Priority Tier 费用，也不提供预付余额。".into()),
    ]);
    source.docs_url = Some(DOCUMENTATION.into());
    source
}
fn malformed(message: &str) -> SourceError {
    SourceError::new(SourceErrorCode::Malformed, message)
}
fn amount(value: &Json) -> Option<&str> {
    value
        .as_str()
        .filter(|text| orch_provider_contract::limits::is_decimal(text))
}
fn minor_unit(currency: &str) -> String {
    if currency.eq_ignore_ascii_case("USD") {
        "USD cents".into()
    } else {
        format!("{} minor units", currency.to_uppercase())
    }
}

pub fn cost_rows(page: &Json) -> Result<Vec<Json>, SourceError> {
    let buckets = page
        .get("data")
        .and_then(Json::as_array)
        .ok_or_else(|| malformed("the cost report has no bucket list"))?;
    let mut rows = Vec::new();
    for bucket in buckets {
        let start = bucket
            .get("starting_at")
            .and_then(Json::as_str)
            .ok_or_else(|| malformed("a cost bucket has no start"))?;
        let end = bucket.get("ending_at").and_then(Json::as_str);
        for row in bucket
            .get("results")
            .and_then(Json::as_array)
            .ok_or_else(|| malformed("a cost bucket has no result list"))?
        {
            let used = amount(&row["amount"])
                .ok_or_else(|| malformed("a cost row has no decimal amount"))?;
            let currency = row
                .get("currency")
                .and_then(Json::as_str)
                .ok_or_else(|| malformed("a cost row has no currency"))?;
            let workspace = row.get("workspace_id").and_then(Json::as_str);
            let description = row.get("description").and_then(Json::as_str);
            let identity = json!([start, workspace, description]);
            let label = [workspace, description]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            rows.push(json!({"id":format!("cost:{}",Hash::of(identity.to_string().as_bytes())),"label":label.chars().take(orch_provider_contract::limits::TEXT_MAX_CHARS).collect::<String>(),"unit":minor_unit(currency),"period":"day","used":used,"resets_at":end,"x_currency":currency,"x_workspace_id":workspace,"x_description":description,"x_starting_at":start}));
        }
    }
    Ok(rows)
}

/// Limits retain monetary text exactly; only the optional display percentage uses a numeric ratio.
pub fn limit_rows(page: &Json, effective: bool) -> Result<(Vec<Json>, Vec<Json>), SourceError> {
    let input = page
        .get("data")
        .and_then(Json::as_array)
        .ok_or_else(|| malformed("the limits report has no list"))?;
    let mut spend = Vec::new();
    let mut windows = Vec::new();
    for row in input {
        let currency = row
            .get("currency")
            .and_then(Json::as_str)
            .ok_or_else(|| malformed("a limit has no currency"))?;
        let period = row
            .get("period")
            .and_then(Json::as_str)
            .ok_or_else(|| malformed("a limit has no period"))?;
        let period_name = match period {
            "daily" => "day",
            "weekly" => "week",
            "monthly" => "month",
            _ => continue,
        };
        let limit = if row["amount"].is_null() {
            None
        } else {
            Some(amount(&row["amount"]).ok_or_else(|| malformed("a limit has no decimal amount"))?)
        };
        let used = if effective {
            Some(
                amount(&row["period_to_date_spend"])
                    .ok_or_else(|| malformed("an effective limit has no decimal spend"))?,
            )
        } else {
            None
        };
        let native_id = row.get("spend_limit_id").or_else(|| row.get("id"));
        let actor = row.get("actor");
        let scope = row.get("scope");
        let identity = json!([effective, native_id, actor, scope, period]);
        let id = format!("limit:{}", Hash::of(identity.to_string().as_bytes()));
        let label = if effective {
            "Effective spend limit"
        } else {
            "Configured spend limit"
        };
        spend.push(json!({"id":id,"label":label,"unit":minor_unit(currency),"period":period_name,"used":used,"limit":limit,"resets_at":null,"x_currency":currency,"x_scope":scope,"x_actor":actor,"x_effective":effective,"x_native_id":native_id,"x_is_enabled":row.get("is_enabled"),"x_reading_is_informational":effective}));
        if let (Some(used), Some(limit)) = (used, limit)
            && let (Ok(used), Ok(limit)) = (used.parse::<f64>(), limit.parse::<f64>())
            && used.is_finite()
            && limit.is_finite()
            && limit > 0.0
            && used >= 0.0
        {
            windows.push(json!({"id":id,"label":label,"duration_minutes":match period{"daily"=>Some(1440),"weekly"=>Some(10080),_=>None},"used_percent":used/limit*100.0,"resets_at":null}));
        }
    }
    Ok((spend, windows))
}

pub struct Anthropic {
    client: Arc<dyn VendorClient>,
    clock: Arc<dyn Clock>,
}
impl Anthropic {
    #[must_use]
    pub fn new(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Self {
        Self { client, clock }
    }
    async fn pages(
        &self,
        base: &Url,
        path: &str,
        headers: &BTreeMap<String, Secret>,
    ) -> Result<Option<Vec<Json>>, SourceError> {
        let initial = base
            .join(path)
            .map_err(|_| malformed("the source endpoint is invalid"))?;
        let mut url = initial.clone();
        let mut seen = BTreeSet::new();
        let mut pages = Vec::new();
        for _ in 0..MAX_PAGES {
            let path = match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().into(),
            };
            let Some(page) = self
                .client
                .get_headers(
                    base,
                    &path,
                    headers,
                    orch_provider_contract::limits::REPORT_MAX_BYTES,
                )
                .await
            else {
                return Ok(None);
            };
            let next = page
                .get("next_page")
                .and_then(Json::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_owned);
            let more = page
                .get("has_more")
                .and_then(Json::as_bool)
                .unwrap_or(next.is_some());
            pages.push(page);
            if !more {
                return Ok(Some(pages));
            }
            let next = next.ok_or_else(|| malformed("a page has no next cursor"))?;
            if !seen.insert(next.clone()) {
                return Err(malformed("a source cursor repeated"));
            }
            url = initial.clone();
            url.query_pairs_mut().append_pair("page", &next);
        }
        Err(malformed("the source exceeded its bounded page read"))
    }
}
#[async_trait]
impl FactsSource for Anthropic {
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
            .ok_or_else(|| SourceError::unsupported("an Anthropic organization endpoint"))?;
        let headers = BTreeMap::from([
            ("x-api-key".into(), admin.clone()),
            ("anthropic-version".into(), Secret::new("2026-10-21")),
        ]);
        let mut limited = headers.clone();
        limited.insert(
            "anthropic-beta".into(),
            Secret::new("spend-limit-reads-2026-09-26"),
        );
        let mut costs = base
            .join("/v1/organizations/cost_report")
            .map_err(|_| malformed("the cost endpoint is invalid"))?;
        let from = format!("{}T00:00:00Z", self.clock.now().format("%Y-%m-%d"));
        costs
            .query_pairs_mut()
            .append_pair("starting_at", &from)
            .append_pair("bucket_width", "1d")
            .append_pair("limit", "1")
            .append_pair("group_by[]", "workspace_id")
            .append_pair("group_by[]", "description");
        let cost_path = format!("{}?{}", costs.path(), costs.query().unwrap_or_default());
        let (identity, costs, limits, effective) = tokio::join!(
            self.client.get_headers(
                &base,
                "/v1/organizations/me",
                &headers,
                orch_provider_contract::limits::REPORT_MAX_BYTES
            ),
            self.pages(&base, &cost_path, &headers),
            self.pages(&base, "/v1/organizations/spend_limits?limit=100", &limited),
            self.pages(
                &base,
                "/v1/organizations/spend_limits/effective?limit=100",
                &limited
            ),
        );
        let mut report = serde_json::Map::new();
        if let Some(identity) = identity
            && let Some(name) = identity.get("name").and_then(Json::as_str)
        {
            report.insert("identity".into(), json!(name));
        }
        let mut spend = Vec::new();
        let mut windows = Vec::new();
        let mut tells_windows = false;
        let pages = costs?.ok_or_else(|| {
            SourceError::new(
                SourceErrorCode::NotTold,
                "the organization did not return its cost report",
            )
        })?;
        for page in pages {
            spend.extend(cost_rows(&page)?);
        }
        // Early-access refusals are optional observations, not a failure of the other reads.
        for (pages, is_effective) in [(limits?, false), (effective?, true)] {
            if let Some(pages) = pages {
                tells_windows = true;
                for page in pages {
                    let (more, usage) = limit_rows(&page, is_effective)?;
                    spend.extend(more);
                    windows.extend(usage);
                }
            }
        }
        report.insert("spend".into(), json!(spend));
        if tells_windows {
            report.insert("windows".into(), json!(windows));
        }
        Ok(FactsReport(Json::Object(report)))
    }
}
fn made(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
    Arc::new(Anthropic::new(client, clock))
}
#[must_use]
pub fn builtin() -> BuiltinSource {
    BuiltinSource {
        name: NAME,
        hosts: &["api.anthropic.com"],
        descriptor,
        matches: known_vendor,
        make: made,
    }
}
