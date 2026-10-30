//! OpenRouter's ordinary-key spending, credits and lapse, using bounded read-only vendor requests.
//!
//! One read asks `GET /api/v1/key` with the provider's key, and then `GET /api/v1/credits`, which OpenRouter
//! answers only for a management key. The key's answer gives its spending (`usage`, by day, week and month),
//! its limit with how often it starts over (`limit`, `limit_remaining`, `limit_reset`), the budgets enforced on
//! it (`budgets`, the one that blocks first being `effective_budget`), the free models' requests of the day
//! (`free_model_daily_requests`) and its lapse. Each limit that starts over is reported as a subscription's
//! window is, with its next reset: daily at 00:00 UTC, weekly on Monday at 00:00 UTC, monthly on the first at
//! 00:00 UTC, as OpenRouter's limits page says; a limit that never starts over is no window but a balance (the
//! key's, beside its spending, and a budget's). The account's credits are told only where the second request
//! answers; an ordinary key's refusal tells nothing and is no error.

use chrono::{DateTime, Datelike, Days, Months, Utc};
use orch_agent_contract::facts::{
    Balance, CredentialFacts, CredentialYield, KeyCredits, KeyUsage, ProviderFacts, UsageWindow,
};
use orch_plugin_contract::{Extensions, Secret};
use orch_provider_contract::builtins::VendorClient;
use orch_types::{Clock, Json, Time};
use std::time::Duration;
use url::Url;

pub mod source;
pub use source::{OpenRouter, builtin};

/// Vendors whose ordinary keys have documented usage reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownVendor {
    /// OpenRouter's key and account-credit endpoints.
    OpenRouter,
}

/// Hosts are matched exactly; vendor documentation records why this list may make a request.
const KNOWN_HOSTS: &[(&str, KnownVendor, &str)] = &[(
    "openrouter.ai",
    KnownVendor::OpenRouter,
    "https://openrouter.ai/docs/api/reference/api-keys/get-current-api-key",
)];
/// OpenRouter documents its account-credit read separately, and refuses it to an ordinary key.
pub const OPENROUTER_CREDITS_DOCUMENTATION: &str =
    "https://openrouter.ai/docs/api/reference/credits/get-credits";
/// OpenRouter's page of limits and budgets, read for when each starts over.
pub const OPENROUTER_LIMITS_DOCUMENTATION: &str = "https://openrouter.ai/docs/api-reference/limits";
/// These small reads must finish inside an interactive provider refresh, including both requests.
const READ_TIMEOUT: Duration = orch_provider_contract::wire::READ_DEADLINE;
/// Known replies are a few KiB; one MiB bounds an unexpected response.
const REPLY_BYTES: usize = orch_provider_contract::limits::REPORT_MAX_BYTES;
/// The documented vendor endpoint uses the ordinary HTTPS port.
const VENDOR_TLS_PORT: u16 = 443;
/// The unit of every amount OpenRouter tells.
const UNIT: &str = "USD";
/// The scope of the budget that is the key's own limit.
const KEY_LIMIT_SCOPE: &str = "key_limit";
/// The window of the key's own limit, where it starts over.
pub const KEY_WINDOW: &str = "key";
/// The balance of the key's own limit, where it never starts over.
pub const KEY_BALANCE: &str = "key";
/// The window of the free models' requests of the day.
pub const FREE_REQUESTS_WINDOW: &str = "free_requests";
/// A day, in minutes.
const DAY_MINUTES: i64 = 24 * 60;
/// A week, in minutes.
const WEEK_MINUTES: i64 = 7 * DAY_MINUTES;
/// All of a limit, in percent.
const WHOLE: f64 = 100.0;

/// The vendor told by the host of a provider's upstream, without suffix or substring matching.
#[must_use]
pub fn known_vendor(base: &Url) -> Option<KnownVendor> {
    KNOWN_HOSTS
        .iter()
        .find(|(host, _, _)| {
            base.host_str() == Some(*host)
                && base.scheme() == "https"
                && base.port_or_known_default() == Some(VENDOR_TLS_PORT)
                && matches!(base.path().trim_end_matches('/'), "" | "/api/v1" | "/api")
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none()
        })
        .map(|(_, vendor, _)| *vendor)
}

fn amount(value: Option<&Json>) -> Option<f64> {
    value?
        .as_f64()
        .filter(|amount| amount.is_finite() && *amount >= 0.0)
}

/// A number the vendor told, below zero too (what is left of a limit that was overrun).
fn finite(value: Option<&Json>) -> Option<f64> {
    value?.as_f64().filter(|number| number.is_finite())
}

/// What part of `limit` `used` is, in percent, from 0 to 100; all of it where nothing may be spent.
#[must_use]
pub fn percent(used: f64, limit: f64) -> f64 {
    if limit <= 0.0 {
        return WHOLE;
    }
    (used / limit * WHOLE).clamp(0.0, WHOLE)
}

/// An amount as decimal text, as a balance keeps it.
fn decimal(amount: f64) -> Option<String> {
    let text = amount.to_string();
    (amount.is_finite() && orch_provider_contract::limits::is_decimal(&text)).then_some(text)
}

/// How often a limit or a budget starts over, as OpenRouter's limits page says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interval {
    /// At 00:00 UTC every day.
    Daily,
    /// At 00:00 UTC every Monday.
    Weekly,
    /// At 00:00 UTC on the first of every month.
    Monthly,
}

impl Interval {
    /// The interval a vendor's word names.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        match word {
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            _ => None,
        }
    }

    /// The vendor's word for it, as a window's label says it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    /// How long it is, in minutes; none for a month, which has no one length.
    #[must_use]
    pub const fn minutes(self) -> Option<i64> {
        match self {
            Self::Daily => Some(DAY_MINUTES),
            Self::Weekly => Some(WEEK_MINUTES),
            Self::Monthly => None,
        }
    }

    /// The first moment after `now` at which it starts over: 00:00 UTC of the next day, of the next Monday (a
    /// week on, on a Monday), or of the first of the next month.
    #[must_use]
    pub fn next_reset(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let today = now.date_naive();
        let day = match self {
            Self::Daily => today.succ_opt()?,
            Self::Weekly => today.checked_add_days(Days::new(u64::from(
                7 - today.weekday().num_days_from_monday(),
            )))?,
            Self::Monthly => today.with_day(1)?.checked_add_months(Months::new(1))?,
        };
        Some(day.and_hms_opt(0, 0, 0)?.and_utc())
    }
}

/// How often a limit starts over: one of the vendor's intervals, or a word of its own that names none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cadence<'a> {
    /// One of the intervals of the limits page.
    Every(Interval),
    /// A word the limits page does not name: shown as it is, with no length and no reset worked out.
    Named(&'a str),
}

/// The cadence a member names; none where it is `null` or absent, which is a limit that never starts over.
fn cadence(value: Option<&Json>) -> Option<Cadence<'_>> {
    let word = value?.as_str()?;
    Some(Interval::of(word).map_or(Cadence::Named(word), Cadence::Every))
}

/// A budget enforced on a key, as `budgets` and `effective_budget` tell it.
#[derive(Debug, Clone, PartialEq)]
struct Budget<'a> {
    scope: &'a str,
    budget_id: Option<&'a str>,
    guardrail_id: Option<&'a str>,
    limit: f64,
    spend: f64,
    remaining: Option<f64>,
    cadence: Option<Cadence<'a>>,
    resets_at: Option<Time>,
}

impl<'a> Budget<'a> {
    fn of(value: &'a Json) -> Option<Self> {
        Some(Self {
            scope: value.get("scope")?.as_str()?,
            budget_id: value.get("budget_id").and_then(Json::as_str),
            guardrail_id: value.get("guardrail_id").and_then(Json::as_str),
            limit: finite(value.get("limit_usd"))?,
            spend: finite(value.get("spend_usd"))?,
            remaining: finite(value.get("remaining_usd")),
            cadence: cadence(value.get("reset_interval")),
            resets_at: value
                .get("resets_at")
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Time>().ok()),
        })
    }

    /// Whether `other` is the same budget.
    fn is(&self, other: &Budget<'_>) -> bool {
        self.scope == other.scope
            && self.budget_id == other.budget_id
            && self.guardrail_id == other.guardrail_id
    }

    /// Its id among the windows and the balances: its scope, and its own id where it has one.
    fn id(&self) -> String {
        match self.budget_id.or(self.guardrail_id) {
            Some(own) => format!("budget:{}:{own}", self.scope),
            None => format!("budget:{}", self.scope),
        }
    }

    /// What it is called: who it is set for, in words.
    fn name(&self) -> String {
        let who = match self.scope {
            "workspace_default_member" => "Workspace member default",
            "workspace_default_key" => "Workspace key default",
            "member" => "Member",
            "api_key" => "Key",
            "workspace" => "Workspace",
            other => other,
        };
        format!("{who} budget")
    }
}

/// A window of a limit that starts over: `what` it is and how often, how much of it is used, and when it starts
/// over (the vendor's moment where it told one, else worked out from `now`).
fn window(
    id: String,
    what: &str,
    cadence: Cadence<'_>,
    used_percent: f64,
    told_reset: Option<Time>,
    now: DateTime<Utc>,
    is_active: Option<bool>,
) -> UsageWindow {
    let (word, duration_minutes, next) = match cadence {
        Cadence::Every(interval) => (
            interval.word(),
            interval.minutes(),
            interval.next_reset(now),
        ),
        Cadence::Named(word) => (word, None, None),
    };
    UsageWindow {
        id,
        label: format!("{what}, {word}"),
        duration_minutes,
        used_percent,
        resets_at: told_reset.or_else(|| next.map(Time::new)),
        model_family: None,
        is_active,
        reset_not_started: None,
        warning_percent: None,
        severity: None,
    }
}

/// The windows of a key's limits that start over (its own limit, each budget, the free models' requests of the
/// day), and the balances of those that never do (its own limit, a budget).
fn limits(data: &Json, now: DateTime<Utc>) -> (Vec<UsageWindow>, Vec<Balance>) {
    let budgets: Vec<Budget<'_>> = data
        .get("budgets")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Budget::of)
        .collect();
    let effective = data
        .get("effective_budget")
        .filter(|value| value.is_object())
        .and_then(Budget::of);
    let binding = |scope: &str, which: Option<&Budget<'_>>| {
        effective.as_ref().map(|effective| match which {
            Some(budget) => effective.is(budget),
            None => effective.scope == scope,
        })
    };
    let (mut windows, mut balances) = (Vec::new(), Vec::new());
    let own = budgets
        .iter()
        .find(|budget| budget.scope == KEY_LIMIT_SCOPE && budget.cadence.is_some());
    let remaining = finite(data.get("limit_remaining"));
    if let Some(budget) = own
        && let Some(cadence) = budget.cadence
    {
        windows.push(window(
            KEY_WINDOW.to_owned(),
            "Key limit",
            cadence,
            percent(budget.spend, budget.limit),
            budget.resets_at,
            now,
            binding(KEY_LIMIT_SCOPE, Some(budget)),
        ));
    } else if let Some(limit) = finite(data.get("limit")) {
        match cadence(data.get("limit_reset")) {
            Some(cadence) => {
                if let Some(remaining) = remaining {
                    windows.push(window(
                        KEY_WINDOW.to_owned(),
                        "Key limit",
                        cadence,
                        percent(limit - remaining, limit),
                        None,
                        now,
                        binding(KEY_LIMIT_SCOPE, None),
                    ));
                }
            }
            // A limit that never starts over is what is left to spend of it: a balance.
            None => balances.push(Balance {
                id: KEY_BALANCE.to_owned(),
                label: Some("Key limit".to_owned()),
                unit: UNIT.to_owned(),
                remaining: remaining.and_then(decimal),
                total: decimal(limit),
                used: None,
                unlimited: None,
                parts: None,
                expires_at: None,
                extensions: Extensions::new(),
            }),
        }
    }
    for budget in budgets
        .iter()
        .filter(|budget| budget.scope != KEY_LIMIT_SCOPE)
    {
        match budget.cadence {
            Some(cadence) => windows.push(window(
                budget.id(),
                &budget.name(),
                cadence,
                percent(budget.spend, budget.limit),
                budget.resets_at,
                now,
                binding(budget.scope, Some(budget)),
            )),
            None => balances.push(Balance {
                id: budget.id(),
                label: Some(budget.name()),
                unit: UNIT.to_owned(),
                remaining: budget.remaining.and_then(decimal),
                total: decimal(budget.limit),
                used: decimal(budget.spend),
                unlimited: None,
                parts: None,
                expires_at: None,
                extensions: Extensions::new(),
            }),
        }
    }
    let free = data.get("free_model_daily_requests");
    if let (Some(used), Some(limit)) = (
        free.and_then(|free| finite(free.get("used"))),
        free.and_then(|free| finite(free.get("limit"))),
    ) {
        windows.push(window(
            FREE_REQUESTS_WINDOW.to_owned(),
            "Free model requests",
            Cadence::Every(Interval::Daily),
            percent(used, limit),
            None,
            now,
            None,
        ));
    }
    (windows, balances)
}

/// Reads OpenRouter's ordinary-key reply at `now`: its spending, its limits that start over as windows, those
/// that never do as balances, and its own expiry; members unrelated to those are ignored.
#[must_use]
pub fn openrouter_key(value: &Json, now: Time) -> Option<ProviderFacts> {
    let data = value.get("data")?;
    let expires_at = match data.get("expires_at") {
        None | Some(Json::Null) => None,
        Some(value) => Some(value.as_str()?.parse::<Time>().ok()?),
    };
    let usage = KeyUsage {
        vendor: "OpenRouter".to_owned(),
        unit: UNIT.to_owned(),
        limit: amount(data.get("limit")),
        remaining: amount(data.get("limit_remaining")),
        used: amount(data.get("usage"))?,
        used_daily: amount(data.get("usage_daily"))?,
        used_weekly: amount(data.get("usage_weekly"))?,
        used_monthly: amount(data.get("usage_monthly"))?,
    };
    let (windows, balances) = limits(data, now.get());
    Some(ProviderFacts {
        credential_verified: Some(true),
        key_usage_supported: Some(true),
        key_usage: Some(usage),
        windows,
        balances,
        credential: Some(CredentialFacts {
            method: None,
            yields: CredentialYield::Key,
            expires_at,
            issued_at: None,
        }),
        ..ProviderFacts::default()
    })
}

/// Reads account credits only when the vendor actually supplied both amounts.
#[must_use]
pub fn openrouter_credits(value: &Json) -> Option<KeyCredits> {
    let data = value.get("data")?;
    Some(KeyCredits {
        unit: UNIT.to_owned(),
        total_credits: amount(data.get("total_credits"))?,
        total_usage: amount(data.get("total_usage"))?,
    })
}

/// Reads one known vendor with the provider's key, through the keeping machine's transport; the caller
/// determines the vendor before dispatch.
pub async fn read(
    client: &dyn VendorClient,
    base: &Url,
    key: &Secret,
    vendor: KnownVendor,
    clock: &dyn Clock,
) -> Option<ProviderFacts> {
    read_details(client, base, key, vendor, clock)
        .await
        .map(|(facts, _)| facts)
}

/// Retains the successful key answer beside its facts for source-specific spend metadata;
/// no second key request or secret is introduced by the normalized Source projection.
pub(crate) async fn read_details(
    client: &dyn VendorClient,
    base: &Url,
    key: &Secret,
    vendor: KnownVendor,
    clock: &dyn Clock,
) -> Option<(ProviderFacts, Json)> {
    let deadline = tokio::time::Instant::now() + READ_TIMEOUT;
    match vendor {
        KnownVendor::OpenRouter => {
            let answer = tokio::time::timeout_at(
                deadline,
                client.get(base, "/api/v1/key", key, REPLY_BYTES),
            )
            .await
            .ok()
            .flatten()?;
            let mut facts = openrouter_key(&answer, clock.time())?;
            for field in [
                "key_usage",
                "key_usage_supported",
                "credential",
                "credential_verified",
                "windows",
            ] {
                facts.field_times.insert(field.to_owned(), clock.time());
            }
            // A limit that never starts over is told as a balance; nothing else of the key is.
            if !facts.balances.is_empty() {
                facts
                    .field_times
                    .insert("balances".to_owned(), clock.time());
            }
            // A slow or refused account read does not discard the key's successful observation: OpenRouter
            // answers it only for a management key.
            let credits = tokio::time::timeout_at(
                deadline,
                client.get(base, "/api/v1/credits", key, REPLY_BYTES),
            )
            .await
            .ok()
            .flatten();
            facts.key_credits = credits.as_ref().and_then(openrouter_credits);
            if facts.key_credits.is_some() {
                facts
                    .field_times
                    .insert("key_credits".to_owned(), clock.time());
            }
            Some((facts, answer.get("data")?.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse::<Time>().unwrap().get()
    }

    #[test]
    fn each_interval_starts_over_at_its_next_utc_midnight() {
        // A Thursday afternoon.
        let now = at("2026-10-15T15:30:00.000Z");
        assert_eq!(
            Interval::Daily.next_reset(now),
            Some(at("2026-10-16T00:00:00.000Z"))
        );
        assert_eq!(
            Interval::Weekly.next_reset(now),
            Some(at("2026-10-19T00:00:00.000Z"))
        );
        assert_eq!(
            Interval::Monthly.next_reset(now),
            Some(at("2026-11-01T00:00:00.000Z"))
        );
        // At the very moment a week begins, the next one is a week on.
        let monday = at("2026-10-19T00:00:00.000Z");
        assert_eq!(
            Interval::Weekly.next_reset(monday),
            Some(at("2026-10-26T00:00:00.000Z"))
        );
        assert_eq!(
            Interval::Daily.next_reset(monday),
            Some(at("2026-10-20T00:00:00.000Z"))
        );
        // The last day of a year.
        let eve = at("2026-12-31T23:59:59.000Z");
        assert_eq!(
            Interval::Monthly.next_reset(eve),
            Some(at("2027-01-01T00:00:00.000Z"))
        );
        assert_eq!(
            Interval::Daily.next_reset(eve),
            Some(at("2027-01-01T00:00:00.000Z"))
        );
        assert_eq!(
            (
                Interval::Daily.minutes(),
                Interval::Weekly.minutes(),
                Interval::Monthly.minutes()
            ),
            (Some(1440), Some(10_080), None)
        );
    }

    #[test]
    fn a_share_of_a_limit_is_a_percent_from_zero_to_a_hundred() {
        assert_eq!(percent(3.0, 20.0), 15.0);
        assert_eq!(percent(25.0, 20.0), 100.0, "an overrun limit is full");
        assert_eq!(
            percent(0.0, 0.0),
            100.0,
            "nothing may be spent of a zero limit"
        );
        assert_eq!(percent(-1.0, 20.0), 0.0);
    }

    #[test]
    fn a_key_without_a_limit_or_budgets_has_only_the_free_models_window() {
        let facts = openrouter_key(
            &json!({"data":{"limit":null,"limit_remaining":null,"limit_reset":null,"usage":0.5,"usage_daily":0.1,
                "usage_weekly":0.2,"usage_monthly":0.5,"budgets":[],"effective_budget":null,
                "free_model_daily_requests":{"used":10,"limit":50,"remaining":40}}}),
            "2026-10-15T15:30:00.000Z".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(facts.windows.len(), 1);
        assert_eq!(facts.windows[0].id, FREE_REQUESTS_WINDOW);
        assert_eq!(facts.windows[0].used_percent, 20.0);
        assert!(facts.balances.is_empty());
    }
}
