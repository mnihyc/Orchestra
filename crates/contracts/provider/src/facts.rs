//! What a provider's sources report of it: the plan, how full its windows are, what is left to spend and what was spent, the resets it offers, and who its credential belongs to.
//!
//! These are the facts of a provider as every source tells them and every reader keeps them: the harness's
//! adapter, a vendor's built-in reader, a program and an address alike. The agent contract and the API's
//! contract re-export them, so that each type is written once. Every member is in one of four states
//! ([`MemberState`], [`member_state`]); the inputs of the built-in readers (a plan's credits, a key's usage,
//! an account's credits) become `balances` and `spend` by [`normalize`].

use std::collections::BTreeMap;

use orch_plugin_contract::Extensions;
use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// The kinds of fact a harness reports of its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFactKind {
    /// The plan the sign-in is on.
    Plan,
    /// The usage windows of the plan and how full they are.
    Windows,
    /// What is left of the plan's own credits.
    PlanCredits,
    /// The resets the vendor makes available.
    AvailableResets,
    /// Spending beyond the included plan.
    ExtraUsage,
    /// A read without a turn can authenticate the provider's credential with its vendor.
    CredentialVerification,
    /// A limit of the plan that has been reached, with when it lifts.
    LimitReached,
    /// Whether the vendor admits ordinary use now.
    OrdinaryUsageAllowed,
}

/// How much of a usage window of the vendor's plan is used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageWindow {
    /// The vendor's id for the window.
    pub id: String,
    /// What it is called: `5h`, `weekly`.
    pub label: String,
    /// How long it is, in minutes; `null` where it has no fixed length.
    #[schemars(range(min = 1))]
    pub duration_minutes: Option<i64>,
    /// How much of it is used, from 0 to 100. A vendor may report a fraction.
    #[schemars(range(min = 0))]
    pub used_percent: f64,
    /// When it starts over.
    pub resets_at: Option<Time>,
    /// The family of models this window covers; null means the whole plan.
    #[serde(default)]
    pub model_family: Option<String>,
    /// Whether the vendor marks this as the binding window.
    #[serde(default)]
    pub is_active: Option<bool>,
    /// The source reports an unstarted or not yet fixed reset, distinct from an unknown reset.
    #[serde(default)]
    pub reset_not_started: Option<bool>,
    /// The vendor's warning mark, in percent, where it names one.
    #[serde(default)]
    #[schemars(range(min = 0, max = 100))]
    pub warning_percent: Option<f64>,
    /// The vendor's severity for display, without a notification.
    #[serde(default)]
    pub severity: Option<String>,
}

/// What the vendor's plan still holds, in the vendor's own unit: not Orch's credits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanCredits {
    /// What is left, as the vendor states it.
    pub balance: Option<String>,
    /// The vendor's unit.
    pub unit: Option<String>,
    /// Whether the plan has no limit.
    pub unlimited: bool,
}

/// A limit of the plan that has been reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LimitReached {
    /// Which limit, as the vendor names it.
    #[serde(rename = "type")]
    pub kind: String,
    /// When it is lifted.
    pub resets_at: Option<Time>,
}

/// What a credential yields, as a provider's facts say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialYield {
    /// A sign-in the harness renews.
    Login,
    /// A token used as it is.
    Token,
    /// An API key.
    Key,
    /// None.
    None,
}

/// How a provider was given its credential, and when it lapses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialFacts {
    /// A sign-in method's id; `null` for a key.
    pub method: Option<String>,
    /// What the credential is.
    pub yields: CredentialYield,
    /// When a token lapses, where that is known.
    pub expires_at: Option<Time>,
    /// When the credential was issued, where its own claims say so.
    #[serde(default)]
    pub issued_at: Option<Time>,
}

/// One reset credit as the vendor lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResetCredit {
    /// The vendor's opaque identifier.
    pub id: String,
    /// The vendor's title, where supplied.
    pub title: Option<String>,
    /// What the vendor says the reset begins anew.
    pub description: Option<String>,
    /// The type of reset, in the vendor's vocabulary.
    pub reset_type: String,
    /// Whether the vendor says it is available, redeeming, redeemed or unknown.
    pub status: String,
    /// When it was granted.
    pub granted_at: Option<Time>,
    /// When it lapses, or null when it has no known end.
    pub expires_at: Option<Time>,
}

/// How many resets are available and the detail rows the vendor supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AvailableResets {
    /// The count, which may be greater than the detail list's length.
    pub available_count: i64,
    /// Null means only the count is known; an empty list means no details were returned.
    pub credits: Option<Vec<ResetCredit>>,
}

/// Spending beyond a subscription's included plan, in the vendor's unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExtraUsage {
    /// Whether extra spending is enabled.
    pub enabled: bool,
    /// The spending limit, where known.
    pub limit: Option<f64>,
    /// What has been used, where known.
    pub used: Option<f64>,
    /// How much is used, in percent.
    pub used_percent: Option<f64>,
    /// The vendor's currency or unit, where supplied.
    pub unit: Option<String>,
    /// The scale the vendor uses for its amounts.
    pub decimal_places: Option<u32>,
    /// Why it is disabled, when the vendor says.
    pub disabled_reason: Option<String>,
    /// Whether its spending limit is reached.
    pub limit_reached: Option<bool>,
}

/// What a vendor tells an ordinary API key about its own spending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KeyUsage {
    /// The vendor's name.
    pub vendor: String,
    /// The vendor's unit, distinct from Orch's ledger credits.
    pub unit: String,
    /// The key's limit; null means no limit was stated.
    pub limit: Option<f64>,
    /// What the key has left of its limit.
    pub remaining: Option<f64>,
    /// What the key has used in all.
    pub used: f64,
    /// What it used today, by the vendor's clock.
    pub used_daily: f64,
    /// What it used this week, by the vendor's clock.
    pub used_weekly: f64,
    /// What it used this month, by the vendor's clock.
    pub used_monthly: f64,
}

/// The whole account's credits, where its ordinary key may read them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KeyCredits {
    /// The vendor's unit.
    pub unit: String,
    /// The credits bought.
    pub total_credits: f64,
    /// The credits used.
    pub total_usage: f64,
}

/// One part of a balance, where the vendor splits it, such as what was granted and what was topped up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BalancePart {
    /// What the vendor calls the part.
    pub label: String,
    /// How much it is, as decimal text in the balance's unit.
    pub amount: String,
}

/// What an account, a plan or a key still holds to be spent, in the vendor's own unit: not Orch's credits.
///
/// Amounts are decimal text (`"12.50"`), so that the vendor's precision is kept as it was told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Balance {
    /// The source's name for it, one of a kind among the balances it tells: `plan`, `account`, a currency.
    pub id: String,
    /// What it is called for people, where the source names it.
    #[serde(default)]
    pub label: Option<String>,
    /// The vendor's unit: a currency such as `USD` or `CNY`, or `credits`.
    pub unit: String,
    /// What is left.
    #[serde(default)]
    pub remaining: Option<String>,
    /// What there was in all.
    #[serde(default)]
    pub total: Option<String>,
    /// What has been spent of it.
    #[serde(default)]
    pub used: Option<String>,
    /// Whether it has no limit.
    #[serde(default)]
    pub unlimited: Option<bool>,
    /// What it is made of, where the vendor splits it.
    #[serde(default)]
    pub parts: Option<Vec<BalancePart>>,
    /// When it lapses, where it does.
    #[serde(default)]
    pub expires_at: Option<Time>,
    /// Members of the source's own (`x_…`), kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// The period a row of spending covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpendPeriod {
    /// A day, by the vendor's clock.
    Day,
    /// A week, by the vendor's clock.
    Week,
    /// A month, by the vendor's clock.
    Month,
    /// Everything so far.
    Total,
    /// A window of its own length, given as `period_minutes`.
    Window,
}

/// What has been spent over one period, in the vendor's own unit, and the limit of that period where there
/// is one. Amounts are decimal text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Spend {
    /// The source's name for the row, one of a kind among the rows it tells.
    pub id: String,
    /// What it is called for people, where the source names it.
    #[serde(default)]
    pub label: Option<String>,
    /// The vendor's unit.
    pub unit: String,
    /// The period it covers.
    pub period: SpendPeriod,
    /// For a period of its own length, how long it is, in minutes.
    #[serde(default)]
    pub period_minutes: Option<i64>,
    /// What has been spent in the period.
    #[serde(default)]
    pub used: Option<String>,
    /// The most that may be spent in it.
    #[serde(default)]
    pub limit: Option<String>,
    /// What is left of the limit.
    #[serde(default)]
    pub remaining: Option<String>,
    /// When the period starts over.
    #[serde(default)]
    pub resets_at: Option<Time>,
    /// Members of the source's own (`x_…`), kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Reads a list member that a source may tell as `null` as well as `[]`: both say that it holds nothing.
fn null_as_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// One confirmed reset attempt, with an identity reused for retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetRequest {
    /// The exact credit shown to the person.
    pub reset_id: String,
    /// The key Orch made for this logical request.
    pub idempotency_key: String,
}

/// What the vendor answered to a reset, with freshly read facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResetResult {
    /// The outcome in the vendor's own vocabulary.
    pub outcome: String,
    /// What the provider knows after the request.
    pub facts: ProviderFacts,
}

/// What a provider's sources report of it: the plan, how full its windows are, what is left to spend and what
/// was spent, and who is signed in.
///
/// `plan_credits`, `key_usage`, `key_credits` and `key_usage_supported` are inputs of the built-in readers
/// only: [`normalize`] turns the first three into `balances` and `spend`, and the last says whether `spend`
/// is supported at all ([`member_state`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ProviderFacts {
    /// The plan, as the harness reports it.
    pub plan: Option<String>,
    /// The usage windows of the plan.
    #[serde(deserialize_with = "null_as_empty")]
    pub windows: Vec<UsageWindow>,
    /// What the plan's own credits have left.
    pub plan_credits: Option<PlanCredits>,
    /// A limit of the plan that has been reached.
    pub limit_reached: Option<LimitReached>,
    /// Whom the harness is signed in as, such as an email, when it reports one.
    pub identity: Option<String>,
    /// How the provider was given its credential.
    pub credential: Option<CredentialFacts>,
    /// Resets still available, where the vendor tells them.
    pub available_resets: Option<AvailableResets>,
    /// Spending beyond the included plan.
    pub extra_usage: Option<ExtraUsage>,
    /// What the account, the plan or the key still holds to be spent, each in its own unit.
    #[serde(deserialize_with = "null_as_empty")]
    pub balances: Vec<Balance>,
    /// What has been spent, each row over its own period.
    #[serde(deserialize_with = "null_as_empty")]
    pub spend: Vec<Spend>,
    /// Whether an ordinary key can read usage; null for a subscription or an unknown answer.
    pub key_usage_supported: Option<bool>,
    /// The key's usage in the vendor's units.
    pub key_usage: Option<KeyUsage>,
    /// The credits of the whole account, where told.
    pub key_credits: Option<KeyCredits>,
    /// The vendor's explicit permission for ordinary included usage; null is no proof of recovery.
    pub ordinary_usage_allowed: Option<bool>,
    /// True only when the vendor authenticated this observation with the provider's credential.
    /// False or null is local knowledge or no proof; a cached earlier observation proves no new recovery.
    pub credential_verified: Option<bool>,
    /// The last successful observation of each source group; absent groups were never observed.
    #[serde(default)]
    pub field_times: std::collections::BTreeMap<String, Time>,
}

impl ProviderFacts {
    /// Whether the vendor has refused included usage; a percentage alone is no admission decision.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.ordinary_usage_allowed == Some(false) || self.limit_reached.is_some()
    }

    /// Whether `member` holds a value: anything but `null` or an empty list. The inputs that [`normalize`]
    /// turns into `balances` and `spend` count for them.
    #[must_use]
    pub fn holds(&self, member: FactMember) -> bool {
        match member {
            FactMember::Plan => self.plan.is_some(),
            FactMember::Windows => !self.windows.is_empty(),
            FactMember::LimitReached => self.limit_reached.is_some(),
            FactMember::OrdinaryUsageAllowed => self.ordinary_usage_allowed.is_some(),
            FactMember::Balances => {
                !self.balances.is_empty()
                    || self.plan_credits.is_some()
                    || self.key_credits.is_some()
            }
            FactMember::Spend => !self.spend.is_empty() || self.key_usage.is_some(),
            FactMember::ExtraUsage => self.extra_usage.is_some(),
            FactMember::AvailableResets => self.available_resets.is_some(),
            FactMember::Identity => self.identity.is_some(),
            FactMember::Credential => self.credential.is_some(),
            FactMember::CredentialVerified => self.credential_verified.is_some(),
        }
    }

    /// Whether `member` was ever told: it, or one of its items (`<member>.<id>`), has a time in
    /// `field_times`. The inputs that [`normalize`] turns into `balances` and `spend` count for them.
    #[must_use]
    pub fn was_told(&self, member: FactMember) -> bool {
        let stamped = |name: &str| {
            self.field_times.keys().any(|key| {
                key == name
                    || key
                        .strip_prefix(name)
                        .is_some_and(|rest| rest.starts_with('.'))
            })
        };
        stamped(member.as_str())
            || match member {
                FactMember::Balances => stamped("plan_credits") || stamped("key_credits"),
                FactMember::Spend => stamped("key_usage"),
                _ => false,
            }
    }
}

/// A member of [`ProviderFacts`] that a source can tell, as its descriptor lists it and `facts_state`
/// answers it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FactMember {
    /// The plan's id.
    Plan,
    /// The usage windows of the plan.
    Windows,
    /// A limit that has been reached.
    LimitReached,
    /// Whether the vendor admits ordinary use now.
    OrdinaryUsageAllowed,
    /// What is left to be spent.
    Balances,
    /// What has been spent, over periods.
    Spend,
    /// Spending beyond the plan.
    ExtraUsage,
    /// Resets that can be spent.
    AvailableResets,
    /// Who the credential belongs to.
    Identity,
    /// How the credential was given and when it lapses.
    Credential,
    /// Whether this read authenticated with the provider's credential.
    CredentialVerified,
}

impl FactMember {
    /// Every member, in the order a provider's details show them.
    pub const ALL: [Self; 11] = [
        Self::Identity,
        Self::Credential,
        Self::CredentialVerified,
        Self::Plan,
        Self::Windows,
        Self::LimitReached,
        Self::OrdinaryUsageAllowed,
        Self::Balances,
        Self::Spend,
        Self::ExtraUsage,
        Self::AvailableResets,
    ];

    /// The member's name, as a report and `field_times` write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Windows => "windows",
            Self::LimitReached => "limit_reached",
            Self::OrdinaryUsageAllowed => "ordinary_usage_allowed",
            Self::Balances => "balances",
            Self::Spend => "spend",
            Self::ExtraUsage => "extra_usage",
            Self::AvailableResets => "available_resets",
            Self::Identity => "identity",
            Self::Credential => "credential",
            Self::CredentialVerified => "credential_verified",
        }
    }

    /// The member an adapter's declaration of a kind of fact stands for: its plan's credits are a balance, a
    /// read that can authenticate the credential tells `credential_verified`, the rest by name.
    #[must_use]
    pub const fn of_kind(kind: ProviderFactKind) -> Self {
        match kind {
            ProviderFactKind::Plan => Self::Plan,
            ProviderFactKind::Windows => Self::Windows,
            ProviderFactKind::PlanCredits => Self::Balances,
            ProviderFactKind::AvailableResets => Self::AvailableResets,
            ProviderFactKind::ExtraUsage => Self::ExtraUsage,
            ProviderFactKind::CredentialVerification => Self::CredentialVerified,
            ProviderFactKind::LimitReached => Self::LimitReached,
            ProviderFactKind::OrdinaryUsageAllowed => Self::OrdinaryUsageAllowed,
        }
    }

    /// The members an adapter's declarations stand for, each once.
    #[must_use]
    pub fn of_kinds(kinds: &[ProviderFactKind]) -> Vec<Self> {
        let mut members: Vec<Self> = kinds.iter().map(|kind| Self::of_kind(*kind)).collect();
        members.sort_unstable();
        members.dedup();
        members
    }
}

impl std::fmt::Display for FactMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a member of a provider's facts stands: one of four, each with its own behaviour of the screens and
/// the engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemberState {
    /// The source does not declare it, and it was never told.
    #[default]
    Unsupported,
    /// The source declares it, and it was never told.
    NotReadYet,
    /// It was told as `null` or `[]`: none offered.
    Empty,
    /// It was told with a value.
    Told,
}

impl MemberState {
    /// The state of a member that holds a value or not, was told or not, and is supported or not.
    #[must_use]
    pub const fn of(holds: bool, told: bool, supported: bool) -> Self {
        if holds {
            Self::Told
        } else if told {
            Self::Empty
        } else if supported {
            Self::NotReadYet
        } else {
            Self::Unsupported
        }
    }
}

/// Where `member` of `facts` stands, for a source that declares `declared`.
///
/// A member that holds a value is told, whatever was declared: a fact proves that it is supported. One told as
/// `null` or `[]` is empty. Otherwise it is not read yet where it is declared, and unsupported where not. The
/// credential is always supported (its keeper always reports it), and `spend` is supported where the key's
/// built-in reader said so (`key_usage_supported`).
#[must_use]
pub fn member_state(
    member: FactMember,
    facts: &ProviderFacts,
    declared: &[FactMember],
) -> MemberState {
    let supported = declared.contains(&member)
        || match member {
            FactMember::Credential => true,
            FactMember::Spend => facts.key_usage_supported == Some(true),
            _ => false,
        };
    MemberState::of(facts.holds(member), facts.was_told(member), supported)
}

/// Where every member of `facts` stands, for a source that declares `declared`.
#[must_use]
pub fn facts_state(
    facts: &ProviderFacts,
    declared: &[FactMember],
) -> BTreeMap<FactMember, MemberState> {
    FactMember::ALL
        .iter()
        .map(|member| (*member, member_state(*member, facts, declared)))
        .collect()
}

/// The `balances` item that a plan's credits become.
pub const PLAN_BALANCE_ID: &str = "plan";
/// The `balances` item that an account's credits become.
pub const ACCOUNT_BALANCE_ID: &str = "account";
/// The `spend` items that a key's usage becomes: in all, today, this week, this month.
pub const KEY_SPEND_IDS: [&str; 4] = ["key:total", "key:day", "key:week", "key:month"];
/// The unit of a plan's credits whose vendor names none.
const PLAN_CREDITS_UNIT: &str = "credits";
/// The most characters of an amount as decimal text; a longer one is not kept.
const DECIMAL_TEXT_MAX: usize = 40;

/// Turns the inputs of the built-in readers into the general members, wherever facts enter: a plan's credits
/// into the `balances` item `plan`, an account's credits into the item `account` (with what is left worked
/// out), a key's usage into the `spend` items `key:total`, `key:day`, `key:week` and `key:month`.
///
/// Each item takes the time of the member it came from (`balances.plan`, `balances.account`, `spend.key:…`),
/// and the list is told then too. An input told as `null` takes its items out. An item, or the whole list,
/// that was told later than the input is newer and stays as it is. The inputs are then cleared with their
/// times; `key_usage_supported` stays, as what says whether `spend` is supported. Applying it twice changes
/// nothing more.
pub fn normalize(facts: &mut ProviderFacts) {
    let before = facts.field_times.clone();
    let plan = facts.plan_credits.take().map(|credits| {
        vec![Balance {
            id: PLAN_BALANCE_ID.to_owned(),
            label: Some("Plan credits".to_owned()),
            unit: credits.unit.unwrap_or_else(|| PLAN_CREDITS_UNIT.to_owned()),
            remaining: credits.balance,
            total: None,
            used: None,
            unlimited: Some(credits.unlimited),
            parts: None,
            expires_at: None,
            extensions: Extensions::new(),
        }]
    });
    let told = facts.field_times.remove("plan_credits");
    absorb(
        &mut facts.balances,
        &mut facts.field_times,
        &before,
        Absorbed {
            member: "balances",
            told,
            items: plan,
            ids: &[PLAN_BALANCE_ID],
        },
        balance_id,
    );
    let account = facts.key_credits.take().map(|credits| {
        let total = decimal_text(credits.total_credits);
        let used = decimal_text(credits.total_usage);
        let remaining = total
            .as_deref()
            .zip(used.as_deref())
            .and_then(|(total, used)| decimal_difference(total, used));
        vec![Balance {
            id: ACCOUNT_BALANCE_ID.to_owned(),
            label: Some("Credits".to_owned()),
            unit: credits.unit,
            remaining,
            total,
            used,
            unlimited: None,
            parts: None,
            expires_at: None,
            extensions: Extensions::new(),
        }]
    });
    let told = facts.field_times.remove("key_credits");
    absorb(
        &mut facts.balances,
        &mut facts.field_times,
        &before,
        Absorbed {
            member: "balances",
            told,
            items: account,
            ids: &[ACCOUNT_BALANCE_ID],
        },
        balance_id,
    );
    let key = facts.key_usage.take().map(|usage| {
        let row = |id: &str, period: SpendPeriod, used: f64| Spend {
            id: id.to_owned(),
            label: None,
            unit: usage.unit.clone(),
            period,
            period_minutes: None,
            used: decimal_text(used),
            limit: None,
            remaining: None,
            resets_at: None,
            extensions: Extensions::new(),
        };
        let [total, day, week, month] = KEY_SPEND_IDS;
        let mut all = row(total, SpendPeriod::Total, usage.used);
        all.limit = usage.limit.and_then(decimal_text);
        all.remaining = usage.remaining.and_then(decimal_text);
        vec![
            all,
            row(day, SpendPeriod::Day, usage.used_daily),
            row(week, SpendPeriod::Week, usage.used_weekly),
            row(month, SpendPeriod::Month, usage.used_monthly),
        ]
    });
    let told = facts.field_times.remove("key_usage");
    absorb(
        &mut facts.spend,
        &mut facts.field_times,
        &before,
        Absorbed {
            member: "spend",
            told,
            items: key,
            ids: &KEY_SPEND_IDS,
        },
        spend_id,
    );
}

/// The id of a balance.
fn balance_id(balance: &Balance) -> &String {
    &balance.id
}

/// The id of a row of spending.
fn spend_id(spend: &Spend) -> &String {
    &spend.id
}

/// What one input of a built-in reader brings into a list member.
struct Absorbed<'a, T> {
    /// The list member: `balances` or `spend`.
    member: &'a str,
    /// When the input was told, where that is known.
    told: Option<Time>,
    /// What it holds, or `None` where it was told as `null` or not at all.
    items: Option<Vec<T>>,
    /// The ids of every item it stands for.
    ids: &'a [&'a str],
}

/// Brings `absorbed` into `list`, by the rules of [`normalize`]; `before` are the times as they were before
/// anything was brought in, so that the order of the inputs does not matter.
fn absorb<T>(
    list: &mut Vec<T>,
    times: &mut BTreeMap<String, Time>,
    before: &BTreeMap<String, Time>,
    absorbed: Absorbed<'_, T>,
    id_of: fn(&T) -> &String,
) {
    let Absorbed {
        member,
        told,
        items,
        ids,
    } = absorbed;
    let later = |key: &str| match (before.get(key), told) {
        (Some(then), Some(told)) => *then > told,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let newer = |id: &str| later(&format!("{member}.{id}")) || later(member);
    match items {
        Some(items) => {
            for item in items {
                let id = id_of(&item).clone();
                if newer(&id) {
                    continue;
                }
                match list.iter().position(|kept| *id_of(kept) == id) {
                    Some(index) => list[index] = item,
                    None => list.push(item),
                }
                if let Some(at) = told {
                    times.insert(format!("{member}.{id}"), at);
                }
            }
        }
        None if told.is_some() => {
            for id in ids.iter().copied() {
                if newer(id) {
                    continue;
                }
                list.retain(|kept| id_of(kept) != id);
                times.remove(&format!("{member}.{id}"));
            }
        }
        None => return,
    }
    if let Some(at) = told {
        let whole = times.entry(member.to_owned()).or_insert(at);
        if *whole < at {
            *whole = at;
        }
    }
}

/// An amount as decimal text: its shortest form, which never has an exponent, or `None` for one that is not
/// finite or too long to keep.
fn decimal_text(amount: f64) -> Option<String> {
    if !amount.is_finite() {
        return None;
    }
    let text = amount.to_string();
    (text.len() <= DECIMAL_TEXT_MAX).then_some(text)
}

/// `minuend − subtrahend` of two amounts as decimal text, worked out exactly, so that `10.3 − 0.1` is `10.2`.
fn decimal_difference(minuend: &str, subtrahend: &str) -> Option<String> {
    let (left, left_scale) = decimal_parts(minuend)?;
    let (right, right_scale) = decimal_parts(subtrahend)?;
    let scale = left_scale.max(right_scale);
    let widen = |value: i128, from: u32| value.checked_mul(10i128.checked_pow(scale - from)?);
    let difference = widen(left, left_scale)?.checked_sub(widen(right, right_scale)?)?;
    let digits = difference.unsigned_abs().to_string();
    let sign = if difference < 0 { "-" } else { "" };
    let width = usize::try_from(scale).ok()?;
    let text = if width == 0 {
        format!("{sign}{digits}")
    } else {
        let padded = format!("{digits:0>width$}", width = width + 1);
        let (whole, fraction) = padded.split_at(padded.len() - width);
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            format!("{sign}{whole}")
        } else {
            format!("{sign}{whole}.{fraction}")
        }
    };
    (text.len() <= DECIMAL_TEXT_MAX).then_some(text)
}

/// Decimal text as a whole number and how many of its digits are after the point.
fn decimal_parts(text: &str) -> Option<(i128, u32)> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (unsigned.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let value: i128 = format!("{whole}{fraction}").parse().ok()?;
    let scale = u32::try_from(fraction.len()).ok()?;
    Some((if negative { -value } else { value }, scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::Json;
    use serde_json::json;

    #[test]
    fn provider_facts_keep_reset_details_scopes_and_vendor_credit_units() {
        let facts: ProviderFacts = serde_json::from_value(json!({
            "windows": [{"id":"secondary", "label":"weekly", "duration_minutes":10080,
                "used_percent":79, "resets_at":null, "model_family":"Sonnet", "is_active":true,
                "warning_percent":75, "severity":"warning"}],
            "credential":{"method":"access_token","yields":"token","expires_at":"2026-10-23T00:00:00.000Z",
                "issued_at":"2026-10-13T00:00:00.000Z"},
            "available_resets":{"available_count":3,"credits":[{"id":"credit-1","title":"Full reset",
                "description":null,"reset_type":"codexRateLimits","status":"available","granted_at":null,"expires_at":null}]},
            "key_usage_supported":true,
            "key_usage":{"vendor":"OpenRouter","unit":"USD","limit":10,"remaining":8,"used":2,
                "used_daily":0.1,"used_weekly":0.5,"used_monthly":1},
            "key_credits":{"unit":"USD","total_credits":20,"total_usage":2}
        })).unwrap();
        assert_eq!(facts.available_resets.as_ref().unwrap().available_count, 3);
        assert_eq!(facts.windows[0].model_family.as_deref(), Some("Sonnet"));
        let written = serde_json::to_value(&facts).unwrap();
        assert_eq!(written["key_usage"]["unit"], "USD");
        assert!(written["extra_usage"].is_null());
        assert_eq!(
            serde_json::from_value::<ProviderFacts>(written).unwrap(),
            facts
        );
        assert_eq!(
            serde_json::from_value::<ProviderFacts>(json!({})).unwrap(),
            ProviderFacts::default()
        );
    }

    #[test]
    fn what_a_harness_reports_of_its_provider_reads_as_the_spec_writes_it() {
        let facts: ProviderFacts = serde_json::from_value(json!({
            "plan": "pro",
            "windows": [ { "id": "primary", "label": "5h", "duration_minutes": 300, "used_percent": 37.5,
                           "resets_at": "2026-10-11T15:00:00.000Z" },
                         { "id": "secondary", "label": "weekly", "duration_minutes": 10080, "used_percent": 12,
                           "resets_at": null } ],
            "plan_credits": { "balance": "1250", "unit": "credits", "unlimited": false },
            "limit_reached": null, "identity": "dana@example.com",
            "credential": { "method": "device_code", "yields": "login", "expires_at": null }
        }))
        .unwrap();
        assert!(!facts.is_exhausted());
        assert_eq!(facts.windows[0].used_percent, 37.5);
        assert_eq!(
            facts.credential.as_ref().unwrap().yields,
            CredentialYield::Login
        );
        let mut full = facts.clone();
        full.windows[1].used_percent = 100.0;
        assert!(
            !full.is_exhausted(),
            "a full window does not prove vendor refusal"
        );
        full.ordinary_usage_allowed = Some(false);
        assert!(full.is_exhausted());
        let mut limited = facts;
        limited.limit_reached = Some(LimitReached {
            kind: "primary".into(),
            resets_at: None,
        });
        assert!(limited.is_exhausted());
        assert_eq!(
            serde_json::to_value(&limited).unwrap()["limit_reached"]["type"],
            json!("primary")
        );
    }

    fn at(text: &str) -> Time {
        text.parse().unwrap()
    }

    /// A value of `member` that is not nothing.
    fn value_of(member: FactMember) -> Json {
        match member {
            FactMember::Plan => json!("pro"),
            FactMember::Windows => json!([{ "id": "5h", "label": "5h", "duration_minutes": 300,
                                           "used_percent": 12, "resets_at": null }]),
            FactMember::LimitReached => json!({ "type": "weekly", "resets_at": null }),
            FactMember::OrdinaryUsageAllowed | FactMember::CredentialVerified => json!(false),
            FactMember::Balances => json!([{ "id": "USD", "unit": "USD", "remaining": "4.20" }]),
            FactMember::Spend => {
                json!([{ "id": "month", "unit": "USD", "period": "month", "used": "1.5" }])
            }
            FactMember::ExtraUsage => {
                json!({ "enabled": false, "limit": null, "used": null, "used_percent": null,
                                              "unit": null, "decimal_places": null, "disabled_reason": null,
                                              "limit_reached": null })
            }
            FactMember::AvailableResets => json!({ "available_count": 0, "credits": [] }),
            FactMember::Identity => json!("dana@example.com"),
            FactMember::Credential => {
                json!({ "method": null, "yields": "key", "expires_at": null })
            }
        }
    }

    /// Facts where `member` was told `value` (absent: not told) at a moment, or held without a time.
    fn facts_with(member: FactMember, value: Option<Json>, timed: bool) -> ProviderFacts {
        let mut report = serde_json::Map::new();
        if let Some(value) = value {
            report.insert(member.as_str().to_owned(), value);
        }
        if timed {
            report.insert(
                "field_times".to_owned(),
                json!({ member.as_str(): "2026-10-15T10:00:00.000Z" }),
            );
        }
        serde_json::from_value(Json::Object(report)).unwrap()
    }

    #[test]
    fn every_member_is_in_exactly_one_of_four_states() {
        for member in FactMember::ALL {
            let told = facts_with(member, Some(value_of(member)), true);
            assert_eq!(
                member_state(member, &told, &[]),
                MemberState::Told,
                "{member}"
            );
            assert_eq!(
                member_state(member, &told, &[member]),
                MemberState::Told,
                "{member}: a fact proves support"
            );
            let held = facts_with(member, Some(value_of(member)), false);
            assert_eq!(
                member_state(member, &held, &[]),
                MemberState::Told,
                "{member}"
            );
            let empty = facts_with(member, Some(Json::Null), true);
            assert_eq!(
                member_state(member, &empty, &[member]),
                MemberState::Empty,
                "{member}"
            );
            assert_eq!(
                member_state(member, &empty, &[]),
                MemberState::Empty,
                "{member}"
            );
            let never = ProviderFacts::default();
            assert_eq!(
                member_state(member, &never, &[member]),
                MemberState::NotReadYet,
                "{member}"
            );
            let expected = if member == FactMember::Credential {
                MemberState::NotReadYet
            } else {
                MemberState::Unsupported
            };
            assert_eq!(member_state(member, &never, &[]), expected, "{member}");
        }
        let empty_list = facts_with(FactMember::Balances, Some(json!([])), true);
        assert_eq!(
            member_state(FactMember::Balances, &empty_list, &[]),
            MemberState::Empty,
            "[] is told as none, like null"
        );
        let mut one_window = ProviderFacts::default();
        one_window
            .field_times
            .insert("windows.5h".to_owned(), at("2026-10-15T10:00:00.000Z"));
        assert_eq!(
            member_state(FactMember::Windows, &one_window, &[]),
            MemberState::Empty,
            "an item told alone tells its list"
        );
        let mut key_read = ProviderFacts {
            key_usage_supported: Some(true),
            ..ProviderFacts::default()
        };
        assert_eq!(
            member_state(FactMember::Spend, &key_read, &[]),
            MemberState::NotReadYet
        );
        key_read.key_usage_supported = Some(false);
        assert_eq!(
            member_state(FactMember::Spend, &key_read, &[]),
            MemberState::Unsupported
        );
        let states = facts_state(&key_read, &[FactMember::Plan]);
        assert_eq!(states.len(), FactMember::ALL.len());
        assert_eq!(states[&FactMember::Plan], MemberState::NotReadYet);
    }

    #[test]
    fn an_adapters_declarations_stand_for_members() {
        assert_eq!(
            FactMember::of_kinds(&[
                ProviderFactKind::PlanCredits,
                ProviderFactKind::Windows,
                ProviderFactKind::CredentialVerification,
                ProviderFactKind::LimitReached,
                ProviderFactKind::OrdinaryUsageAllowed,
                ProviderFactKind::Windows,
            ]),
            [
                FactMember::Windows,
                FactMember::LimitReached,
                FactMember::OrdinaryUsageAllowed,
                FactMember::Balances,
                FactMember::CredentialVerified
            ]
        );
        for member in FactMember::ALL {
            assert_eq!(
                serde_json::to_value(member).unwrap(),
                json!(member.as_str())
            );
        }
    }

    #[test]
    fn a_plans_credits_become_the_balance_plan_with_their_time() {
        let mut facts: ProviderFacts = serde_json::from_value(json!({
            "plan_credits": { "balance": "1250", "unit": null, "unlimited": false },
            "field_times": { "plan_credits": "2026-10-15T10:00:00.000Z" }
        }))
        .unwrap();
        normalize(&mut facts);
        assert_eq!(facts.plan_credits, None, "the input is cleared");
        assert_eq!(facts.balances.len(), 1);
        let plan = &facts.balances[0];
        assert_eq!(plan.id, PLAN_BALANCE_ID);
        assert_eq!(plan.label.as_deref(), Some("Plan credits"));
        assert_eq!(
            plan.unit, "credits",
            "a plan's credits without a unit are credits"
        );
        assert_eq!(plan.remaining.as_deref(), Some("1250"));
        assert_eq!(plan.unlimited, Some(false));
        assert_eq!(
            facts.field_times["balances.plan"],
            at("2026-10-15T10:00:00.000Z")
        );
        assert_eq!(
            facts.field_times["balances"],
            at("2026-10-15T10:00:00.000Z")
        );
        assert!(!facts.field_times.contains_key("plan_credits"));
        let once = facts.clone();
        normalize(&mut facts);
        assert_eq!(facts, once, "normalizing twice changes nothing more");
    }

    #[test]
    fn an_accounts_credits_become_the_balance_account_with_what_is_left_worked_out_exactly() {
        let mut facts: ProviderFacts = serde_json::from_value(json!({
            "key_credits": { "unit": "USD", "total_credits": 10.3, "total_usage": 0.1 },
            "field_times": { "key_credits": "2026-10-15T09:00:00.000Z" }
        }))
        .unwrap();
        normalize(&mut facts);
        let account = &facts.balances[0];
        assert_eq!(account.id, ACCOUNT_BALANCE_ID);
        assert_eq!(account.label.as_deref(), Some("Credits"));
        assert_eq!(
            (
                account.total.as_deref(),
                account.used.as_deref(),
                account.remaining.as_deref()
            ),
            (Some("10.3"), Some("0.1"), Some("10.2"))
        );
        assert_eq!(
            facts.field_times["balances.account"],
            at("2026-10-15T09:00:00.000Z")
        );
        assert_eq!(facts.key_credits, None);
        let mut whole: ProviderFacts = serde_json::from_value(
            json!({ "key_credits": { "unit": "USD", "total_credits": 20, "total_usage": 23.5 } }),
        )
        .unwrap();
        normalize(&mut whole);
        assert_eq!(whole.balances[0].remaining.as_deref(), Some("-3.5"));
        assert!(
            whole.field_times.is_empty(),
            "an input without a time gives none"
        );
    }

    #[test]
    fn a_keys_usage_becomes_four_rows_of_spending_with_its_time() {
        let mut facts: ProviderFacts = serde_json::from_value(json!({
            "key_usage_supported": true,
            "key_usage": { "vendor": "OpenRouter", "unit": "USD", "limit": 10, "remaining": 8.25, "used": 1.75,
                           "used_daily": 0.1, "used_weekly": 0.5, "used_monthly": 1.0 },
            "field_times": { "key_usage": "2026-10-15T10:00:00.000Z", "key_usage_supported": "2026-10-15T10:00:00.000Z" }
        }))
        .unwrap();
        normalize(&mut facts);
        let ids: Vec<&str> = facts.spend.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, KEY_SPEND_IDS);
        let total = &facts.spend[0];
        assert_eq!(total.period, SpendPeriod::Total);
        assert_eq!(
            (
                total.used.as_deref(),
                total.limit.as_deref(),
                total.remaining.as_deref()
            ),
            (Some("1.75"), Some("10"), Some("8.25"))
        );
        assert_eq!(facts.spend[1].period, SpendPeriod::Day);
        assert_eq!(facts.spend[1].used.as_deref(), Some("0.1"));
        assert_eq!(facts.spend[3].used.as_deref(), Some("1"));
        assert_eq!(
            facts.spend[2].limit, None,
            "only the key's whole use has its limit"
        );
        for id in KEY_SPEND_IDS {
            assert_eq!(
                facts.field_times[&format!("spend.{id}")],
                at("2026-10-15T10:00:00.000Z")
            );
        }
        assert_eq!(facts.key_usage, None);
        assert_eq!(
            facts.key_usage_supported,
            Some(true),
            "spend's support stays"
        );
        assert!(facts.field_times.contains_key("key_usage_supported"));
        assert_eq!(
            member_state(FactMember::Spend, &facts, &[]),
            MemberState::Told
        );
    }

    #[test]
    fn what_was_told_later_wins_over_an_older_input_and_an_input_told_as_nothing_takes_its_items_out()
     {
        let mut newer: ProviderFacts = serde_json::from_value(json!({
            "plan_credits": { "balance": "1", "unit": "credits", "unlimited": false },
            "balances": [{ "id": "plan", "unit": "credits", "remaining": "900" }],
            "field_times": { "plan_credits": "2026-10-15T09:00:00.000Z",
                             "balances": "2026-10-15T10:00:00.000Z", "balances.plan": "2026-10-15T10:00:00.000Z" }
        }))
        .unwrap();
        normalize(&mut newer);
        assert_eq!(newer.balances[0].remaining.as_deref(), Some("900"));
        assert_eq!(
            newer.field_times["balances.plan"],
            at("2026-10-15T10:00:00.000Z")
        );
        let mut list_later: ProviderFacts = serde_json::from_value(json!({
            "key_credits": { "unit": "USD", "total_credits": 5, "total_usage": 1 },
            "balances": [],
            "field_times": { "key_credits": "2026-10-15T09:00:00.000Z", "balances": "2026-10-15T10:00:00.000Z" }
        }))
        .unwrap();
        normalize(&mut list_later);
        assert!(
            list_later.balances.is_empty(),
            "a later whole list did not hold the account"
        );
        let mut gone: ProviderFacts = serde_json::from_value(json!({
            "plan_credits": null,
            "balances": [{ "id": "plan", "unit": "credits", "remaining": "5" },
                         { "id": "USD", "unit": "USD", "remaining": "2" }],
            "field_times": { "plan_credits": "2026-10-15T11:00:00.000Z",
                             "balances.plan": "2026-10-15T10:00:00.000Z", "balances.USD": "2026-10-15T10:00:00.000Z" }
        }))
        .unwrap();
        normalize(&mut gone);
        assert_eq!(gone.balances.len(), 1);
        assert_eq!(gone.balances[0].id, "USD");
        assert!(!gone.field_times.contains_key("balances.plan"));
        assert_eq!(gone.field_times["balances"], at("2026-10-15T11:00:00.000Z"));
        let mut both: ProviderFacts = serde_json::from_value(json!({
            "plan_credits": { "balance": "7", "unit": "credits", "unlimited": false },
            "key_credits": { "unit": "USD", "total_credits": 5, "total_usage": 1 },
            "field_times": { "plan_credits": "2026-10-15T10:00:00.000Z", "key_credits": "2026-10-15T09:00:00.000Z" }
        }))
        .unwrap();
        normalize(&mut both);
        assert_eq!(both.balances.len(), 2, "one input does not hide the other");
        assert_eq!(both.field_times["balances"], at("2026-10-15T10:00:00.000Z"));
    }

    #[test]
    fn a_list_told_as_null_reads_as_none() {
        let facts: ProviderFacts =
            serde_json::from_value(json!({ "windows": null, "balances": null, "spend": null }))
                .unwrap();
        assert!(facts.windows.is_empty() && facts.balances.is_empty() && facts.spend.is_empty());
        let balance: Balance = serde_json::from_value(
            json!({ "id": "CNY", "unit": "CNY", "remaining": "110.00", "x_region": "cn",
                    "parts": [{ "label": "Granted", "amount": "10.00" }] }),
        )
        .unwrap();
        assert_eq!(balance.extensions.get("x_region"), Some(&json!("cn")));
        assert_eq!(serde_json::to_value(&balance).unwrap()["x_region"], "cn");
    }
}
