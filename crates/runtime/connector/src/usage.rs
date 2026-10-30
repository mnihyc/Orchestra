//! Provider facts and per-request usage reporting.
//!
//! What a forwarded model request used is read from its answer as it streams by (the wire protocol's
//! observer) and goes three places: into the running totals of the lease it ran under, where the allowance of
//! the turn is held against it; into a `lease.usage` event, one for each request, for the system's ledger;
//! and, summed, into the answer to `lease.close`.
//!
//! **The allowance.** A lease may carry what the turn can still use: a figure in tokens, a figure in credits
//! with the rate they are counted at, or both. The forwarder refuses the next request once a figure is
//! reached ([`Allowance::reached`]), so a cap holds within one request even while the link to the system is
//! down. Tokens are the total of the observed usage. Credits follow the rule of the rate card: the input that
//! was not read from a cache at the input price, the cached input at its own, the output at the output price,
//! a million tokens to the price, and the prices of the largest step of `above` that the prompt exceeds. The
//! arithmetic is exact: prices and amounts are decimals with at most nine places, kept as whole billionths,
//! and a request adds `tokens × price` without dividing, so nothing is rounded until a figure is shown.
//! Input written to a cache, where the upstream counts it apart, is charged at the rate's price for it
//! (the input price where the rate gives none), as the ledger counts it. A service tier's multiple is in
//! the rate the lease carries: the system gives the forwarder the rate the turn's tier comes to, since the
//! forwarder does not know the tier.

use orch_agent_contract::events::TokenUsage;
use orch_agent_contract::facts::{CredentialFacts, CredentialYield, ProviderFacts};
use orch_agent_contract::parameters::Rate;
use orch_link_contract::commands::LeaseOpenAllowance;
use orch_link_contract::events::{Event, LeaseDenied, LeaseUsage};
use orch_model_contract::usage::ObservedUsage;
use orch_types::Id;

/// How many fractional digits a price or an amount may have: nine, so that the prices of a rate card (a
/// handful of decimals) and the sums of them are whole numbers of billionths.
pub const DECIMAL_PLACES: u32 = 9;
/// One, as a count of billionths.
const ONE: i128 = 1_000_000_000;
/// How many tokens a price is for: a million, as every rate says.
const TOKENS_PER_PRICE: i128 = 1_000_000;
/// The most digits before the point: enough for any amount of credits, and keeps the arithmetic far from
/// the limits of the integer.
const MAX_WHOLE_DIGITS: usize = 20;
/// The places of the credits shown: six, as the rule of the rate card keeps them.
const SHOWN_PLACES: u32 = 6;
/// How much accrued charge makes one shown unit, a millionth of a credit: a thousand billionths, each a
/// million tokens' worth of price.
const ACCRUED_PER_SHOWN_UNIT: i128 = TOKENS_PER_PRICE * 1_000;

/// Reads a decimal such as `3.125` as billionths: `None` for anything that is not digits with an optional
/// fraction of at most [`DECIMAL_PLACES`] digits.
#[must_use]
pub fn parse_billionths(text: &str) -> Option<i128> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || !digits(fraction) || whole.len() > MAX_WHOLE_DIGITS {
        return None;
    }
    if fraction.len() > DECIMAL_PLACES as usize {
        return None;
    }
    let whole_value: i128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let padded = format!("{fraction:0<width$}", width = DECIMAL_PLACES as usize);
    let fraction_value: i128 = padded.parse().ok()?;
    whole_value.checked_mul(ONE)?.checked_add(fraction_value)
}

/// What a request is charged for each kind of token, in billionths of a credit for a million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prices {
    /// For input that was not read from a cache.
    pub input: i128,
    /// For input that was read from a cache.
    pub cached: i128,
    /// For input that was written to a cache.
    pub cache_write: i128,
    /// For output, the reasoning among it.
    pub output: i128,
}

impl Prices {
    /// The prices of `rate` for a request whose prompt is `prompt_tokens`: the rate's own, or those of the
    /// entry of `above` with the largest `context_tokens` that the prompt exceeds (a price that entry leaves
    /// out is the rate's own). `None` when a price is not a decimal the arithmetic can hold.
    #[must_use]
    pub fn of(rate: &Rate, prompt_tokens: i64) -> Option<Self> {
        let input = parse_billionths(&rate.input_per_mtok)?;
        let base = Self {
            input,
            cached: parse_billionths(&rate.cached_input_per_mtok)?,
            cache_write: match &rate.cache_write_per_mtok {
                Some(price) => parse_billionths(price)?,
                None => input,
            },
            output: parse_billionths(&rate.output_per_mtok)?,
        };
        let step = rate
            .above
            .iter()
            .flatten()
            .filter(|step| prompt_tokens > step.context_tokens)
            .max_by_key(|step| step.context_tokens);
        let Some(step) = step else {
            return Some(base);
        };
        let step_input = parse_billionths(&step.input_per_mtok)?;
        Some(Self {
            input: step_input,
            cached: match &step.cached_input_per_mtok {
                Some(price) => parse_billionths(price)?,
                None => base.cached,
            },
            cache_write: match &step.cache_write_per_mtok {
                Some(price) => parse_billionths(price)?,
                None => step_input,
            },
            output: parse_billionths(&step.output_per_mtok)?,
        })
    }

    /// What `usage` is charged: `tokens × price` summed, not yet divided by the million tokens a price is
    /// for (see [`credits_text`]).
    #[must_use]
    pub fn accrue(&self, usage: &TokenUsage) -> i128 {
        let cached = i128::from(usage.cached_input_tokens.max(0));
        let written = i128::from(usage.cache_write_tokens.max(0));
        let fresh = (i128::from(usage.input_tokens) - cached - written).max(0);
        let output = i128::from(usage.output_tokens.max(0));
        fresh * self.input
            + cached * self.cached
            + written * self.cache_write
            + output * self.output
    }
}

/// An accrued charge as credits, to the six places the rule of the rate card keeps (half up).
#[must_use]
pub fn credits_text(accrued: i128) -> String {
    let shown_units = (accrued + ACCRUED_PER_SHOWN_UNIT / 2) / ACCRUED_PER_SHOWN_UNIT;
    let one = 10_i128.pow(SHOWN_PLACES);
    format!(
        "{}.{:0width$}",
        shown_units / one,
        shown_units % one,
        width = SHOWN_PLACES as usize
    )
}

/// What a figure of the allowance in credits holds: the limit, and the rate the tokens are counted at.
#[derive(Debug, Clone, PartialEq)]
pub struct CreditLimit {
    /// The limit, as the accrued charge that reaches it (billionths of a credit times the million tokens
    /// a price is for).
    limit: i128,
    /// The rate.
    rate: Rate,
}

/// What a turn may still use before the forwarder refuses its requests.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Allowance {
    /// The room in tokens, where a cap in tokens covers the turn.
    tokens: Option<i64>,
    /// The room in credits, where a cap in credits covers it.
    credits: Option<CreditLimit>,
}

/// Why an allowance in a `lease.open` cannot be held.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AllowanceError {
    /// The amount is not a decimal the arithmetic holds.
    #[error("the allowance in credits is {0:?}, which is not a decimal with at most nine places")]
    Amount(String),
    /// The rate cannot be read.
    #[error("the rate of the allowance cannot be read: {0}")]
    Rate(String),
    /// A price of the rate is not a decimal the arithmetic holds.
    #[error("a price of the allowance's rate is not a decimal with at most nine places")]
    Price,
}

impl Allowance {
    /// No allowance: nothing is held back.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// An allowance of `tokens` tokens.
    #[must_use]
    pub fn of_tokens(tokens: i64) -> Self {
        Self {
            tokens: Some(tokens),
            credits: None,
        }
    }

    /// The allowance a `lease.open` carries.
    pub fn from_open(open: Option<&LeaseOpenAllowance>) -> Result<Self, AllowanceError> {
        let Some(open) = open else {
            return Ok(Self::unlimited());
        };
        let credits = match &open.credits {
            Some(credits) => {
                let amount = parse_billionths(&credits.amount)
                    .ok_or_else(|| AllowanceError::Amount(credits.amount.clone()))?;
                let rate: Rate = credits.rate.clone();
                // Every price must be one the arithmetic holds, so that a request is never uncounted.
                Prices::of(&rate, 0).ok_or(AllowanceError::Price)?;
                for step in rate.above.iter().flatten() {
                    Prices::of(&rate, step.context_tokens.saturating_add(1))
                        .ok_or(AllowanceError::Price)?;
                }
                Some(CreditLimit {
                    limit: amount.saturating_mul(TOKENS_PER_PRICE),
                    rate,
                })
            }
            None => None,
        };
        Ok(Self {
            tokens: open.tokens,
            credits,
        })
    }

    /// Whether any figure is held.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.tokens.is_some() || self.credits.is_some()
    }

    /// What `usage` adds to the credits of the lease, as an accrued charge; nothing where no figure in credits
    /// is held.
    #[must_use]
    pub fn accrue(&self, usage: &TokenUsage) -> i128 {
        self.credits
            .as_ref()
            .and_then(|credits| Prices::of(&credits.rate, usage.input_tokens))
            .map_or(0, |prices| prices.accrue(usage))
    }

    /// Whether what has been used reaches a figure of the allowance: then the next request is refused.
    #[must_use]
    pub fn reached(&self, used: &Totals) -> bool {
        self.tokens
            .is_some_and(|tokens| used.usage.total_tokens >= tokens)
            || self
                .credits
                .as_ref()
                .is_some_and(|credits| used.accrued >= credits.limit)
    }
}

/// What the requests of a lease have used so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Totals {
    /// How many requests ran.
    pub requests: i64,
    /// The sum of the usage that was read.
    pub usage: TokenUsage,
    /// Whether any request told what it used.
    pub known: bool,
    /// The charge accrued at the allowance's rate, where it has one.
    pub accrued: i128,
}

impl Totals {
    /// Counts one request that ran.
    pub fn count_request(&mut self) {
        self.requests += 1;
    }

    /// Adds what a request used, in whole or in the part that was read since the last time.
    pub fn add_usage(&mut self, usage: &TokenUsage, accrued: i128) {
        self.usage = self.usage.plus(*usage);
        self.known = true;
        self.accrued += accrued;
    }

    /// The usage as `lease.close` answers it: `None` where no request said what it used.
    #[must_use]
    pub fn reported(&self) -> Option<TokenUsage> {
        self.known.then_some(self.usage)
    }
}

/// What `now` has that `before` does not, member by member; a member that did not grow adds nothing.
#[must_use]
pub fn growth(now: &TokenUsage, before: &TokenUsage) -> TokenUsage {
    let grew = |now: i64, before: i64| (now - before).max(0);
    TokenUsage {
        input_tokens: grew(now.input_tokens, before.input_tokens),
        cached_input_tokens: grew(now.cached_input_tokens, before.cached_input_tokens),
        cache_write_tokens: grew(now.cache_write_tokens, before.cache_write_tokens),
        output_tokens: grew(now.output_tokens, before.output_tokens),
        reasoning_tokens: grew(now.reasoning_tokens, before.reasoning_tokens),
        total_tokens: grew(now.total_tokens, before.total_tokens),
    }
}

/// The `lease.usage` event of one request.
#[must_use]
pub fn lease_usage(lease_id: Id, model: &str, observed: &ObservedUsage) -> Option<Event> {
    let usage = observed.tokens;
    Some(Event::LeaseUsage(LeaseUsage {
        lease_id,
        model: model.to_owned(),
        usage,
        request_ref: observed.request_ref.clone(),
    }))
}

/// The `lease.denied` event of a request the forwarder refused for a reason the system should see.
#[must_use]
pub fn lease_denied(lease_id: Id, model: &str, reason: &str) -> Event {
    Event::LeaseDenied(LeaseDenied {
        lease_id,
        model: model.to_owned(),
        reason: reason.to_owned(),
    })
}

/// What an API provider reports of itself: it has a key (or needs none) and no plan, windows or limits of a
/// vendor's subscription.
#[must_use]
pub fn api_provider_facts(needs_key: bool) -> ProviderFacts {
    ProviderFacts {
        credential: Some(CredentialFacts {
            method: None,
            yields: if needs_key {
                CredentialYield::Key
            } else {
                CredentialYield::None
            },
            expires_at: None,
            issued_at: None,
        }),
        ..ProviderFacts::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::events::TokensReportedApart;
    use serde_json::json;

    fn rate(value: serde_json::Value) -> Rate {
        serde_json::from_value(value).unwrap()
    }

    fn usage(input: i64, cached: i64, output: i64) -> TokenUsage {
        TokenUsage::from_parts(TokensReportedApart {
            input: input - cached,
            cache_read: cached,
            cache_write: 0,
            output,
            reasoning: 0,
        })
    }

    fn standard() -> Rate {
        rate(json!({
            "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15",
            "above": [ { "context_tokens": 272000, "input_per_mtok": "5", "cached_input_per_mtok": "0.5", "output_per_mtok": "22.5" } ]
        }))
    }

    #[test]
    fn a_price_is_read_exactly_or_not_at_all() {
        assert_eq!(parse_billionths("3.125"), Some(3_125_000_000));
        assert_eq!(parse_billionths("15"), Some(15_000_000_000));
        assert_eq!(parse_billionths(".5"), Some(500_000_000));
        assert_eq!(parse_billionths("0.000000001"), Some(1));
        for refused in ["", ".", "-1", "1e3", "1,5", "0.1234567891", "abc", "1.2.3"] {
            assert_eq!(parse_billionths(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn a_request_is_charged_by_kind_of_token_at_the_price_of_a_million() {
        let prices = Prices::of(&standard(), 1000).unwrap();
        // 800 fresh input tokens, 200 cached, 100 of output: (800 x 2.5 + 200 x 0.25 + 100 x 15) / 1e6.
        let accrued = prices.accrue(&usage(1000, 200, 100));
        assert_eq!(credits_text(accrued), "0.003550");
    }

    #[test]
    fn input_written_to_a_cache_is_charged_at_its_own_price_and_at_the_input_price_without_one() {
        let written = TokenUsage::from_parts(TokensReportedApart {
            input: 600,
            cache_read: 200,
            cache_write: 200,
            output: 100,
            reasoning: 0,
        });
        let priced = rate(json!({
            "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15",
            "cache_write_per_mtok": "3.125"
        }));
        // (600 x 2.5 + 200 x 0.25 + 200 x 3.125 + 100 x 15) / 1e6.
        let accrued = Prices::of(&priced, 1000).unwrap().accrue(&written);
        assert_eq!(credits_text(accrued), "0.003675");
        // A rate without the price counts what was written as input.
        let as_input = Prices::of(&standard(), 1000).unwrap().accrue(&written);
        assert_eq!(credits_text(as_input), "0.003550");
    }

    #[test]
    fn a_prompt_above_a_step_pays_the_prices_of_the_step() {
        let below = Prices::of(&standard(), 272_000).unwrap();
        let above = Prices::of(&standard(), 272_001).unwrap();
        assert_eq!(below.input, 2_500_000_000);
        assert_eq!(above.input, 5_000_000_000);
        assert_eq!(above.cached, 500_000_000);
        assert_eq!(above.output, 22_500_000_000);
        let partial = rate(json!({
            "input_per_mtok": "1", "cached_input_per_mtok": "0.1", "output_per_mtok": "2",
            "above": [ { "context_tokens": 10, "input_per_mtok": "3", "output_per_mtok": "4" } ]
        }));
        assert_eq!(
            Prices::of(&partial, 11).unwrap().cached,
            100_000_000,
            "a price a step leaves out is the rate's own"
        );
    }

    #[test]
    fn credits_are_shown_to_six_places_rounded_half_up() {
        // One token at 0.5 credits for a million is 0.0000005: half of the last shown place.
        let prices = Prices::of(&rate(json!({"input_per_mtok": "0.5", "cached_input_per_mtok": "0", "output_per_mtok": "0"})), 1).unwrap();
        assert_eq!(credits_text(prices.accrue(&usage(1, 0, 0))), "0.000001");
        assert_eq!(credits_text(0), "0.000000");
        assert_eq!(
            credits_text(prices.accrue(&usage(1_000_000_000, 0, 0))),
            "500.000000"
        );
    }

    #[test]
    fn an_allowance_in_tokens_is_reached_at_its_figure_and_not_before() {
        let allowance = Allowance::of_tokens(100);
        let mut totals = Totals::default();
        totals.add_usage(&usage(60, 0, 39), 0);
        assert!(!allowance.reached(&totals), "99 of 100 is room");
        totals.add_usage(&usage(1, 0, 0), 0);
        assert!(allowance.reached(&totals), "100 of 100 is none");
        assert!(!Allowance::unlimited().reached(&totals));
    }

    #[test]
    fn an_allowance_in_credits_is_held_at_the_rate_it_came_with() {
        let open: LeaseOpenAllowance = serde_json::from_value(json!({
            "credits": { "amount": "0.005", "rate": { "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15" } }
        }))
        .unwrap();
        let allowance = Allowance::from_open(Some(&open)).unwrap();
        assert!(allowance.is_held());
        let mut totals = Totals::default();
        // 0.00355 credits: below 0.005.
        let first = usage(1000, 200, 100);
        totals.add_usage(&first, allowance.accrue(&first));
        assert!(!allowance.reached(&totals));
        // Another 0.00355 passes it.
        totals.add_usage(&first, allowance.accrue(&first));
        assert!(allowance.reached(&totals));
    }

    #[test]
    fn a_request_that_did_not_say_what_it_used_counts_as_a_request_and_as_nothing_else() {
        let mut totals = Totals::default();
        totals.count_request();
        assert_eq!(totals.requests, 1);
        assert_eq!(totals.reported(), None);
        totals.count_request();
        totals.add_usage(&usage(10, 0, 5), 0);
        assert_eq!(totals.reported().map(|usage| usage.total_tokens), Some(15));
        assert_eq!(totals.requests, 2);
    }

    #[test]
    fn an_allowance_that_cannot_be_held_is_refused_when_it_is_opened() {
        let bad_amount: LeaseOpenAllowance = serde_json::from_value(json!({
            "credits": { "amount": "lots", "rate": { "input_per_mtok": "1", "cached_input_per_mtok": "1", "output_per_mtok": "1" } }
        }))
        .unwrap();
        assert!(matches!(
            Allowance::from_open(Some(&bad_amount)),
            Err(AllowanceError::Amount(_))
        ));
        let bad_rate = serde_json::from_value::<LeaseOpenAllowance>(
            json!({ "credits": { "amount": "1", "rate": { "input_per_mtok": "1" } } }),
        );
        assert!(bad_rate.is_err());
        let bad_price: LeaseOpenAllowance = serde_json::from_value(json!({
            "credits": { "amount": "1", "rate": { "input_per_mtok": "a lot", "cached_input_per_mtok": "1", "output_per_mtok": "1" } }
        }))
        .unwrap();
        assert!(matches!(
            Allowance::from_open(Some(&bad_price)),
            Err(AllowanceError::Price)
        ));
    }

    #[test]
    fn what_grew_is_what_is_added_and_a_member_that_did_not_grow_adds_nothing() {
        let before = usage(100, 20, 10);
        let now = usage(150, 20, 40);
        let added = growth(&now, &before);
        assert_eq!(
            (
                added.input_tokens,
                added.cached_input_tokens,
                added.output_tokens
            ),
            (50, 0, 30)
        );
        assert_eq!(added.total_tokens, 80);
        assert_eq!(growth(&before, &now).total_tokens, 0);
    }

    #[test]
    fn what_an_api_provider_reports_of_itself_says_whether_it_has_a_key() {
        let keyed = api_provider_facts(true);
        assert!(!keyed.is_exhausted());
        assert_eq!(keyed.credential.unwrap().yields, CredentialYield::Key);
        let open = api_provider_facts(false);
        assert_eq!(open.credential.unwrap().yields, CredentialYield::None);
    }
}
