//! The rate a turn is counted at: a provider's own, the rate card, or the shipped catalogue of estimates, in that order; one rule for every pattern that names models; the credits counted from it, marked where they are an estimate; and the tokens that have no rate.
//!
//! A **rate** says what a million tokens of a model cost in credits: the input, the cached input, the output,
//! and where its vendor prices differently also the tokens written to a prompt cache, a service tier at a
//! multiple, and a prompt above a size at other prices. For one use the rate is the first of these that has
//! one, and the row of the ledger says which (`rate_source`):
//!
//! | Source | Is |
//! |--------|----|
//! | `provider` | The `rate` of the provider's own entry for the model |
//! | `rate_card` | The entry of `usage.rates` that matches the model, as resolved at the thread's place |
//! | `catalogue` | The entry of the shipped catalogue that matches, times `usage.catalogue_scale`, where `usage.catalogue` is on: an estimate |
//! | `reported` | An actual vendor total in the catalogue's matching unit, converted by the same scale in place of the estimate; explicit provider/card prices stay in force |
//!
//! Patterns of the rate card and the catalogue name models by the one rule of `models`; of the patterns that
//! match, the one with the most characters that are not `*` counts. When none has a rate, the row has none and
//! its credits are not zero but unsaid: the tokens are counted, and a figure in credits can say how many it
//! leaves out.
//!
//! **The credits of a use** are `((input − cached − cache written) × input price + cached × cached price +
//! cache written × cache price + output × output price) ÷ 1 000 000`, times the multiple of the service tier the
//! use ran with where the rate lists one, kept to six decimal places (half up). The prices are the rate's own,
//! or those of the step of `above` with the largest prompt size that the use's prompt exceeds. The arithmetic
//! is exact: a price is a decimal number and is multiplied as one, never as a floating-point number, so a rate
//! of `2.50` and a million tokens come to exactly `2.500000` and the rows of a day add up to the figure of
//! the day. A close snapshot is priced once for each factual aggregate group, following the same
//! six-place row convention; that can differ by millionths from adding individually rounded requests.
//! These are Orch credits at the recorded rate, not a vendor invoice.

use crate::error::EngineError;
use orch_agent_contract::discovery::ModelPattern;
use orch_agent_contract::parameters::{Rate, RateStep};
use orch_db::Conn;
use orch_db::queries::resources::keeper;
use orch_db::usage::ledger::{self, Tokens};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use std::collections::BTreeMap;

/// How many places of a credit a row keeps: six, a millionth of a credit, the precision the ledger keeps. A row
/// is rounded once, half up, so it is off by at most half a millionth, which no figure shown to a person reaches.
pub const CREDIT_PLACES: u32 = 6;
/// How many tokens a price is for: every rate names the price of a million, which is six places of ten.
const PRICE_PLACES: u32 = 6;
/// The longest a decimal number may be, in characters: far more than any price or scale, and short enough
/// that its digits cannot overflow the integer they are held in.
const MAX_DECIMAL_CHARS: usize = 30;

/// Where the rate of a use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The provider's own entry for the model.
    Provider,
    /// The rate card.
    RateCard,
    /// The shipped catalogue: an estimate.
    Catalogue,
    /// A vendor-reported total converted in the matching catalogue unit, without quoted fees added.
    Reported,
}

impl Source {
    /// The word the ledger writes (`rate_source`).
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Provider => ledger::rate_source::PROVIDER,
            Self::RateCard => ledger::rate_source::RATE_CARD,
            Self::Catalogue => ledger::rate_source::CATALOGUE,
            Self::Reported => ledger::rate_source::REPORTED,
        }
    }

    /// Whether credits counted from this source are an estimate.
    #[must_use]
    pub const fn is_estimate(self) -> bool {
        matches!(self, Self::Catalogue)
    }
}

// ---------------------------------------------------------------- exact decimals

/// A decimal number: `mantissa` times ten to the minus `scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dec {
    mantissa: i128,
    scale: u32,
}

impl Dec {
    const ONE: Self = Self {
        mantissa: 1,
        scale: 0,
    };

    const fn whole(count: i128) -> Self {
        Self {
            mantissa: count,
            scale: 0,
        }
    }

    /// A plain decimal such as `3.125`: digits, then optionally a point and more digits. No sign, no
    /// exponent.
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_DECIMAL_CHARS {
            return None;
        }
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        if !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        Some(Self {
            mantissa: format!("{whole}{fraction}").parse().ok()?,
            scale: u32::try_from(fraction.len()).ok()?,
        })
    }

    fn mul(self, other: Self) -> Option<Self> {
        Some(Self {
            mantissa: self.mantissa.checked_mul(other.mantissa)?,
            scale: self.scale.checked_add(other.scale)?,
        })
    }

    fn at_scale(self, scale: u32) -> Option<i128> {
        let steps = scale.checked_sub(self.scale)?;
        self.mantissa.checked_mul(10_i128.checked_pow(steps)?)
    }

    fn add(self, other: Self) -> Option<Self> {
        let scale = self.scale.max(other.scale);
        Some(Self {
            mantissa: self.at_scale(scale)?.checked_add(other.at_scale(scale)?)?,
            scale,
        })
    }

    /// Divided by a power of ten.
    fn shifted(self, places: u32) -> Option<Self> {
        Some(Self {
            mantissa: self.mantissa,
            scale: self.scale.checked_add(places)?,
        })
    }

    /// Kept to `places` fractional digits, a half going up.
    fn rounded(self, places: u32) -> Option<Self> {
        let Some(extra) = self.scale.checked_sub(places).filter(|extra| *extra > 0) else {
            return Some(self);
        };
        let divisor = 10_i128.checked_pow(extra)?;
        let quotient = self.mantissa / divisor;
        let remainder = self.mantissa % divisor;
        let up = i128::from(remainder * 2 >= divisor);
        Some(Self {
            mantissa: quotient.checked_add(up)?,
            scale: places,
        })
    }

    /// The number with exactly `places` fractional digits (more digits than that are cut off: round first).
    fn text(self, places: u32) -> Option<String> {
        let mantissa = self.at_scale(places).or_else(|| {
            let extra = self.scale - places;
            Some(self.mantissa / 10_i128.checked_pow(extra)?)
        })?;
        let divisor = 10_i128.checked_pow(places)?;
        Some(format!(
            "{}.{:0width$}",
            mantissa / divisor,
            mantissa % divisor,
            width = places as usize
        ))
    }

    /// The number with no trailing zeros, as a price is written.
    fn trimmed(self) -> String {
        let mut mantissa = self.mantissa;
        let mut scale = self.scale;
        while scale > 0 && mantissa % 10 == 0 {
            mantissa /= 10;
            scale -= 1;
        }
        if scale == 0 {
            return mantissa.to_string();
        }
        let digits = format!("{mantissa:0>width$}", width = scale as usize + 1);
        let (whole, fraction) = digits.split_at(digits.len() - scale as usize);
        format!("{whole}.{fraction}")
    }
}

// ---------------------------------------------------------------- credits

/// What a use is charged per million tokens of each kind, in decimals.
#[derive(Debug, Clone, Copy)]
struct Prices {
    input: Dec,
    cached: Dec,
    cache_write: Dec,
    cache_write_1h: Option<Dec>,
    cache_write_12h: Option<Dec>,
    output: Dec,
}

/// The existing strict context-band selector, shared by pricing and aggregate
/// sufficiency checks. Comparing prices alone would miss intermediate bands.
fn context_step(rate: &Rate, context: Option<i64>) -> Option<&RateStep> {
    context.and_then(|context| {
        rate.above
            .iter()
            .flatten()
            .filter(|step| context > step.context_tokens)
            .max_by_key(|step| step.context_tokens)
    })
}

fn same_context_band(rate: &Rate, low: i64, high: i64) -> bool {
    low <= high
        && match (
            context_step(rate, Some(low)),
            context_step(rate, Some(high)),
        ) {
            (None, None) => true,
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            _ => false,
        }
}

impl Prices {
    /// The prices of `rate` for a use whose prompt was `context_tokens` large.
    fn of(rate: &Rate, context_tokens: Option<i64>) -> Option<Self> {
        let input = Dec::parse(&rate.input_per_mtok)?;
        let cached = Dec::parse(&rate.cached_input_per_mtok)?;
        let output = Dec::parse(&rate.output_per_mtok)?;
        let cache_write = match &rate.cache_write_per_mtok {
            Some(price) => Dec::parse(price)?,
            None => input,
        };
        let cache_write_1h = match &rate.cache_write_1h_per_mtok {
            Some(price) => Some(Dec::parse(price)?),
            None => None,
        };
        let cache_write_12h = match &rate.cache_write_12h_per_mtok {
            Some(price) => Some(Dec::parse(price)?),
            None => None,
        };
        let step = context_step(rate, context_tokens);
        let Some(step) = step else {
            return Some(Self {
                input,
                cached,
                cache_write,
                cache_write_1h,
                cache_write_12h,
                output,
            });
        };
        let step_input = Dec::parse(&step.input_per_mtok)?;
        Some(Self {
            input: step_input,
            cached: match &step.cached_input_per_mtok {
                Some(price) => Dec::parse(price)?,
                None => cached,
            },
            cache_write: match &step.cache_write_per_mtok {
                Some(price) => Dec::parse(price)?,
                None => step_input,
            },
            cache_write_1h: match &step.cache_write_1h_per_mtok {
                Some(price) => Some(Dec::parse(price)?),
                None => cache_write_1h,
            },
            cache_write_12h: match &step.cache_write_12h_per_mtok {
                Some(price) => Some(Dec::parse(price)?),
                None => cache_write_12h,
            },
            output: Dec::parse(&step.output_per_mtok)?,
        })
    }
}

/// The credits `tokens` come to at `rate`, to six places, as a decimal number. `context_tokens` is how large
/// the prompt was and `tier` the service tier the use ran with. None where a price of the rate is not a
/// decimal the arithmetic can hold.
#[must_use]
pub fn credits(
    rate: &Rate,
    tokens: &Tokens,
    context_tokens: Option<i64>,
    tier: Option<&str>,
) -> Option<String> {
    token_charge(rate, tokens, context_tokens, tier)?
        .rounded(CREDIT_PLACES)?
        .text(CREDIT_PLACES)
}

fn token_charge(
    rate: &Rate,
    tokens: &Tokens,
    context_tokens: Option<i64>,
    tier: Option<&str>,
) -> Option<Dec> {
    token_charge_with_lifetimes(rate, tokens, context_tokens, tier, 0, 0)
}
fn token_charge_with_lifetimes(
    rate: &Rate,
    tokens: &Tokens,
    context_tokens: Option<i64>,
    tier: Option<&str>,
    hourly: i64,
    twelve_hour: i64,
) -> Option<Dec> {
    let prices = Prices::of(rate, context_tokens)?;
    let cached = tokens.cached_input.max(0);
    let written = tokens.cache_write.max(0);
    if hourly < 0 || twelve_hour < 0 || hourly.checked_add(twelve_hour)? > written {
        return None;
    }
    let fresh = (tokens.input - cached - written).max(0);
    let terms = [
        (fresh, prices.input),
        (cached, prices.cached),
        (written - hourly - twelve_hour, prices.cache_write),
        (tokens.output.max(0), prices.output),
    ];
    let mut sum = Dec::whole(0);
    for (count, price) in terms {
        sum = sum.add(price.mul(Dec::whole(i128::from(count)))?)?;
    }
    if hourly > 0 {
        sum = sum.add(prices.cache_write_1h?.mul(Dec::whole(i128::from(hourly)))?)?;
    }
    if twelve_hour > 0 {
        sum = sum.add(
            prices
                .cache_write_12h?
                .mul(Dec::whole(i128::from(twelve_hour)))?,
        )?;
    }
    let mut credits = sum.shifted(PRICE_PLACES)?;
    let multiple = match tier.and_then(|tier| rate.tiers.as_ref()?.get(tier)) {
        Some(multiple) => Dec::parse(multiple)?,
        None => Dec::ONE,
    };
    credits = credits.mul(multiple)?;
    Some(credits)
}

/// The rate a use at service tier `tier` is counted at: every price times the tier's multiple where
/// the rate lists one, with no multiple left to apply again. What a forwarder holds a turn's allowance
/// at, since it does not know the tier the turn runs with. None where a price or the multiple is not a
/// decimal the arithmetic can hold.
#[must_use]
pub fn for_tier(rate: &Rate, tier: Option<&str>) -> Option<Rate> {
    let multiple = match tier.and_then(|tier| rate.tiers.as_ref()?.get(tier)) {
        Some(multiple) => Dec::parse(multiple)?,
        None => Dec::ONE,
    };
    let mut applied = scaled(rate, multiple)?;
    applied.web_search_per_call = rate.web_search_per_call.clone();
    applied.web_fetch_per_call = rate.web_fetch_per_call.clone();
    applied.tiers = None;
    Some(applied)
}

/// The rate with every price multiplied by `scale`: what one unit of the catalogue is worth in a place's
/// credits. The multiples of the service tiers and the sizes of the steps are not prices and stay.
fn scaled(rate: &Rate, scale: Dec) -> Option<Rate> {
    let price = |text: &str| -> Option<String> { Some(Dec::parse(text)?.mul(scale)?.trimmed()) };
    let optional = |text: &Option<String>| -> Option<Option<String>> {
        match text {
            Some(text) => Some(Some(price(text)?)),
            None => Some(None),
        }
    };
    let above = match &rate.above {
        Some(steps) => Some(
            steps
                .iter()
                .map(|step| {
                    Some(RateStep {
                        context_tokens: step.context_tokens,
                        input_per_mtok: price(&step.input_per_mtok)?,
                        cached_input_per_mtok: optional(&step.cached_input_per_mtok)?,
                        output_per_mtok: price(&step.output_per_mtok)?,
                        cache_write_per_mtok: optional(&step.cache_write_per_mtok)?,
                        cache_write_1h_per_mtok: optional(&step.cache_write_1h_per_mtok)?,
                        cache_write_12h_per_mtok: optional(&step.cache_write_12h_per_mtok)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        None => None,
    };
    Some(Rate {
        web_search_per_call: optional(&rate.web_search_per_call)?,
        web_fetch_per_call: optional(&rate.web_fetch_per_call)?,
        input_per_mtok: price(&rate.input_per_mtok)?,
        cached_input_per_mtok: price(&rate.cached_input_per_mtok)?,
        output_per_mtok: price(&rate.output_per_mtok)?,
        cache_write_per_mtok: optional(&rate.cache_write_per_mtok)?,
        cache_write_1h_per_mtok: optional(&rate.cache_write_1h_per_mtok)?,
        cache_write_12h_per_mtok: optional(&rate.cache_write_12h_per_mtok)?,
        tiers: rate.tiers.clone(),
        capacity_tiers: rate.capacity_tiers.clone(),
        above,
    })
}

// ---------------------------------------------------------------- finding the rate

/// A rate that was found, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The rate.
    pub rate: Rate,
    /// Where it came from.
    pub source: Source,
}

/// What a use is counted at, as it goes into the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Priced {
    /// The rate and its source; none when no rate names the model.
    pub found: Option<Found>,
    /// The credits it comes to; none when the rate or the actual processing tier is unavailable.
    pub credits: Option<String>,
}

/// The entry of `candidates` that names `model` with the most characters that are not `*`.
fn best<'a>(candidates: &'a [(String, Rate)], model: &str) -> Option<&'a Rate> {
    ModelPattern::best(
        candidates
            .iter()
            .map(|(pattern, rate)| (pattern.as_str(), rate)),
        model,
    )
}

/// Exact existing-ledger pricing of a service quantity, with an immutable applied unit quote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PricedService {
    /// Unknown quantity, quote or currency leaves the total unknown.
    pub credits: Option<String>,
    /// Full-precision applied credits per unit; rounding occurs only on the final row total.
    pub credits_per_unit: Option<String>,
}

/// The rates of a place that are not the provider's: its rate card, and the settings that govern the
/// catalogue. The catalogue itself is read only when nothing before it has a rate.
#[derive(Debug, Clone)]
pub struct RateBook {
    card: Vec<(String, Rate)>,
    use_catalogue: bool,
    scale: Dec,
}

/// Actual request measurements and the native caller's wire-format fallback for pricing.
pub(crate) struct Observation<'a> {
    pub timing: &'a orch_api_contract::usage::RequestTiming,
    pub protocol: Option<&'a str>,
}

impl RateBook {
    /// The rate card, the catalogue switch and the catalogue scale as they resolve at `node`.
    pub async fn load(conn: &mut Conn, node: Id) -> Result<Self, EngineError> {
        let card: BTreeMap<String, Rate> =
            orch_settings::get(conn, Place::Node(node), keys::USAGE_RATES)
                .await?
                .unwrap_or_default();
        let use_catalogue: bool =
            orch_settings::get(conn, Place::Node(node), keys::USAGE_CATALOGUE)
                .await?
                .unwrap_or(true);
        let scale_text: Option<String> =
            orch_settings::get(conn, Place::Node(node), keys::USAGE_CATALOGUE_SCALE).await?;
        let scale = match scale_text.as_deref() {
            None => Dec::ONE,
            Some(text) => Dec::parse(text).ok_or_else(|| {
                EngineError::unreadable(format!("usage.catalogue_scale is {text:?}, not a decimal"))
            })?,
        };
        Ok(Self {
            card: card.into_iter().collect(),
            use_catalogue,
            scale,
        })
    }

    /// A book with only the given rate card and no catalogue: what a test or a caller without settings uses.
    #[must_use]
    pub fn with_card(card: Vec<(String, Rate)>) -> Self {
        Self {
            card,
            use_catalogue: false,
            scale: Dec::ONE,
        }
    }

    /// Price one selected service quote with the same exact decimal and catalogue policy.
    /// A declaration supplies no currency conversion or provider-price override.
    pub async fn price_service(
        &self,
        conn: &mut Conn,
        quote: Option<&orch_provider_contract::capabilities::Price>,
        source: Source,
        unit: &str,
        quantity: &orch_api_contract::usage::UsageMeasurement<i64>,
    ) -> Result<PricedService, EngineError> {
        let Some(quote) = quote.filter(|quote| quote.unit == unit) else {
            return Ok(PricedService::default());
        };
        let scale = if source == Source::Catalogue {
            if !self.use_catalogue {
                return Ok(PricedService::default());
            }
            let catalogue = keeper::rate_catalogue(conn).await?;
            if catalogue
                .as_ref()
                .and_then(|doc| doc.get("unit"))
                .and_then(Json::as_str)
                != Some(quote.currency.as_str())
            {
                return Ok(PricedService::default());
            }
            self.scale
        } else if quote.currency.eq_ignore_ascii_case("credits") {
            Dec::ONE
        } else {
            return Ok(PricedService::default());
        };
        let applied = Dec::parse(&quote.amount).and_then(|value| value.mul(scale));
        let credits_per_unit = applied.and_then(|value| value.text(value.scale));
        let credits = quantity
            .value()
            .filter(|count| **count >= 0)
            .and_then(|count| {
                applied?
                    .mul(Dec::whole(i128::from(*count)))?
                    .rounded(CREDIT_PLACES)?
                    .text(CREDIT_PLACES)
            });
        Ok(PricedService {
            credits,
            credits_per_unit,
        })
    }

    /// The rate for `model`: the provider's own entry (`own`), else the rate card, else the shipped
    /// catalogue's estimate. None where no rate names it.
    pub async fn find(
        &self,
        conn: &mut Conn,
        own: Option<&Rate>,
        model: &str,
    ) -> Result<Option<Found>, EngineError> {
        if let Some(rate) = own {
            return Ok(Some(Found {
                rate: rate.clone(),
                source: Source::Provider,
            }));
        }
        if let Some(rate) = best(&self.card, model) {
            return Ok(Some(Found {
                rate: rate.clone(),
                source: Source::RateCard,
            }));
        }
        if !self.use_catalogue {
            return Ok(None);
        }
        let Some(document) = keeper::rate_catalogue(conn).await? else {
            return Ok(None);
        };
        let families = families_of(&document);
        let Some(rate) = best(&families, model) else {
            return Ok(None);
        };
        Ok(scaled(rate, self.scale).map(|rate| Found {
            rate,
            source: Source::Catalogue,
        }))
    }

    /// What `tokens` of `model` are counted at: the rate, and the credits it comes to.
    pub async fn price(
        &self,
        conn: &mut Conn,
        own: Option<&Rate>,
        model: &str,
        tokens: &Tokens,
        context_tokens: Option<i64>,
        tier: Option<&str>,
    ) -> Result<Priced, EngineError> {
        self.price_with_alias(conn, own, model, tokens, context_tokens, (tier, None))
            .await
    }

    /// As `price`, with one protocol-proven alias when no multiplier is set for the actual tier.
    /// Retain the resolved multiplier in the event's copied rate; the provider's rate is unchanged.
    pub(crate) async fn price_with_alias(
        &self,
        conn: &mut Conn,
        own: Option<&Rate>,
        model: &str,
        tokens: &Tokens,
        context_tokens: Option<i64>,
        processing_tier: (Option<&str>, Option<&str>),
    ) -> Result<Priced, EngineError> {
        let (tier, alias) = processing_tier;
        let Some(mut found) = self.find(conn, own, model).await? else {
            return Ok(Priced::default());
        };
        materialize_alias(&mut found.rate, tier, alias);
        Ok(price_found(found, model, tokens, context_tokens, tier))
    }

    /// The same actual-tier decision used by structured OwnUse and raw observers.
    /// Old callers without an observation retain their explicit legacy tier separately.
    pub(crate) async fn price_observed(
        &self,
        conn: &mut Conn,
        own: Option<&Rate>,
        model: &str,
        tokens: &Tokens,
        context: (i64, i64),
        observation: Observation<'_>,
    ) -> Result<(Option<String>, Priced), EngineError> {
        use orch_api_contract::usage::UsageMeasurement;
        let Observation { timing, protocol } = observation;
        let tier = timing
            .reported_service_tier
            .value()
            .map(String::as_str)
            .filter(|tier| !matches!(*tier, "default" | "standard"));
        let unavailable = matches!(
            timing.reported_service_tier,
            UsageMeasurement::Unread | UsageMeasurement::Empty
        ) || (timing
            .requested_service_tier
            .as_deref()
            .is_some_and(|tier| !matches!(tier, "default" | "standard"))
            && timing.reported_service_tier.value().is_none());
        let alias = timing
            .wire_protocol
            .as_deref()
            .or(protocol)
            .and_then(|protocol| {
                tier.and_then(|tier| {
                    orch_model_contract::parameters::service_tier_price_alias(protocol, tier)
                })
            });
        let Some(mut found) = self.find(conn, own, model).await? else {
            return Ok((tier.map(str::to_owned), Priced::default()));
        };
        materialize_alias(&mut found.rate, tier, alias);
        if found.source == Source::Catalogue {
            if timing.native_prices.contains_key("web_search") {
                found.rate.web_search_per_call = None;
            }
            if timing.native_prices.contains_key("page_fetch") {
                found.rate.web_fetch_per_call = None;
            }
        }
        let capacity = for_capacity_observation(&found.rate, timing, protocol);
        let mut priced = if unavailable
            || capacity.is_none()
            || !same_context_band(&found.rate, context.0, context.1)
        {
            Priced {
                found: Some(found),
                credits: None,
            }
        } else {
            found.rate = capacity
                .ok_or_else(|| EngineError::unreadable("the capacity price disappeared"))?;
            let mut priced = price_found(found, model, tokens, Some(context.1), tier);
            if let Some(rate) = priced.found.as_ref() {
                priced.credits =
                    if rate.source == Source::Catalogue && !timing.native_prices.is_empty() {
                        let document = keeper::rate_catalogue(conn).await?;
                        let unit = document
                            .as_ref()
                            .and_then(|doc| doc.get("unit"))
                            .and_then(Json::as_str);
                        credits_with_native_prices(
                            &rate.rate,
                            tokens,
                            Some(context.1),
                            tier,
                            timing,
                            unit,
                            self.scale,
                        )
                    } else {
                        credits_with_timing(&rate.rate, tokens, Some(context.1), tier, timing)
                    };
            }
            priced
        };
        self.apply_reported_total(conn, timing, &mut priced).await?;
        Ok((tier.map(str::to_owned), priced))
    }

    /// Original vendor totals replace catalogue estimates only. Explicit manager/card credit rates remain authoritative.
    pub(crate) async fn apply_reported_total(
        &self,
        conn: &mut Conn,
        timing: &orch_api_contract::usage::RequestTiming,
        priced: &mut Priced,
    ) -> Result<(), EngineError> {
        let source = priced.found.as_ref().map(|found| found.source);
        if let Some((actual, unit)) = self.reported_cost(conn, timing, source, None).await? {
            apply_reported_total(priced, actual, Some(&unit), self.scale);
        }
        Ok(())
    }

    /// The same total-conversion policy for a selected service's original catalogue quote.
    pub(crate) async fn reported_total(
        &self,
        conn: &mut Conn,
        timing: &orch_api_contract::usage::RequestTiming,
        source: Option<Source>,
        original_currency: &str,
    ) -> Result<Option<String>, EngineError> {
        let Some((actual, unit)) = self
            .reported_cost(conn, timing, source, Some(original_currency))
            .await?
        else {
            return Ok(None);
        };
        Ok(converted_reported_total(
            source,
            actual,
            Some(&unit),
            self.scale,
        ))
    }

    async fn reported_cost<'t>(
        &self,
        conn: &mut Conn,
        timing: &'t orch_api_contract::usage::RequestTiming,
        source: Option<Source>,
        original_currency: Option<&str>,
    ) -> Result<Option<(&'t orch_api_contract::usage::ReportedCost, String)>, EngineError> {
        if !self.use_catalogue || source != Some(Source::Catalogue) {
            return Ok(None);
        }
        if timing
            .reported_tokens
            .as_ref()
            .is_none_or(|tokens| tokens.is_empty())
            && timing
                .outcome
                .value()
                .is_some_and(|outcome| outcome != "done")
        {
            return Ok(None);
        }
        let Some(actual) = timing.reported_cost.value() else {
            return Ok(None);
        };
        let Some(document) = keeper::rate_catalogue(conn).await? else {
            return Ok(None);
        };
        let Some(unit) = document
            .get("unit")
            .and_then(Json::as_str)
            .filter(|unit| !unit.is_empty())
        else {
            return Ok(None);
        };
        if original_currency.is_some_and(|currency| currency != unit) {
            return Ok(None);
        }
        Ok(Some((actual, unit.to_owned())))
    }
}

fn apply_reported_total(
    priced: &mut Priced,
    actual: &orch_api_contract::usage::ReportedCost,
    catalogue_unit: Option<&str>,
    scale: Dec,
) {
    let Some(credits) = converted_reported_total(
        priced.found.as_ref().map(|found| found.source),
        actual,
        catalogue_unit,
        scale,
    ) else {
        return;
    };
    priced.credits = Some(credits);
    if let Some(found) = &mut priced.found {
        found.source = Source::Reported;
    }
}

fn converted_reported_total(
    source: Option<Source>,
    actual: &orch_api_contract::usage::ReportedCost,
    catalogue_unit: Option<&str>,
    scale: Dec,
) -> Option<String> {
    if source != Some(Source::Catalogue) {
        return None;
    }
    let unit = actual.unit.as_deref().filter(|unit| !unit.is_empty())?;
    if Some(unit) != catalogue_unit.filter(|unit| !unit.is_empty()) {
        return None;
    }
    Dec::parse(&actual.amount)?
        .mul(scale)?
        .rounded(CREDIT_PLACES)?
        .text(CREDIT_PLACES)
}

/// Apply only an actual reported capacity multiplier. Legacy/unaware sources keep their prior rate.
fn for_capacity_observation(
    rate: &Rate,
    timing: &orch_api_contract::usage::RequestTiming,
    protocol: Option<&str>,
) -> Option<Rate> {
    use orch_api_contract::usage::UsageMeasurement;
    if timing.reported_capacity_tier == UsageMeasurement::Unsupported
        && timing.requested_capacity_tier.is_none()
    {
        return Some(rate.clone());
    }
    let reported = timing.reported_capacity_tier.value()?;
    let ordinary = timing
        .wire_protocol
        .as_deref()
        .or(protocol)
        .is_some_and(|protocol| {
            orch_model_contract::parameters::ordinary_capacity_tier(protocol, reported)
        });
    let multiple = if ordinary {
        Dec::ONE
    } else {
        Dec::parse(rate.capacity_tiers.as_ref()?.get(reported)?)?
    };
    let mut applied = scaled(rate, multiple)?;
    // Native tool fees are independent of both inference speed and capacity.
    applied.web_search_per_call = rate.web_search_per_call.clone();
    applied.web_fetch_per_call = rate.web_fetch_per_call.clone();
    applied.capacity_tiers = None;
    Some(applied)
}

/// Search fees are additional and are not multiplied by a model's processing tier.
#[must_use]
pub fn credits_with_searches(
    rate: &Rate,
    tokens: &Tokens,
    context: Option<i64>,
    tier: Option<&str>,
    searches: &orch_api_contract::usage::UsageMeasurement<i64>,
) -> Option<String> {
    credits_with_web_tools(
        rate,
        tokens,
        context,
        tier,
        searches,
        &orch_api_contract::usage::UsageMeasurement::Unsupported,
    )
}
/// Native tool fees share the same decimal kernel and retain independent quantities.
#[must_use]
pub fn credits_with_web_tools(
    rate: &Rate,
    tokens: &Tokens,
    context: Option<i64>,
    tier: Option<&str>,
    searches: &orch_api_contract::usage::UsageMeasurement<i64>,
    fetches: &orch_api_contract::usage::UsageMeasurement<i64>,
) -> Option<String> {
    finish_with_web_tools(
        rate,
        token_charge(rate, tokens, context, tier)?,
        searches,
        fetches,
    )
}
/// Measured longer-lived subsets are never charged again as ordinary cache writes.
pub(crate) fn credits_with_timing(
    rate: &Rate,
    tokens: &Tokens,
    context: Option<i64>,
    tier: Option<&str>,
    timing: &orch_api_contract::usage::RequestTiming,
) -> Option<String> {
    finish_with_web_tools(
        rate,
        token_charge_observed(rate, tokens, context, tier, timing)?,
        &timing.web_searches,
        &timing.web_fetches,
    )
}

fn token_charge_observed(
    rate: &Rate,
    tokens: &Tokens,
    context: Option<i64>,
    tier: Option<&str>,
    timing: &orch_api_contract::usage::RequestTiming,
) -> Option<Dec> {
    use orch_api_contract::usage::UsageMeasurement;
    let hourly = match timing.cache_write_1h_tokens.as_ref() {
        None | Some(UsageMeasurement::Unsupported) => 0,
        Some(UsageMeasurement::Told { value }) => *value,
        _ if tokens.cache_write == 0 => 0,
        _ => return None,
    };
    let twelve_hour = match timing.cache_write_12h_tokens.as_ref() {
        Some(UsageMeasurement::Told { value }) => *value,
        Some(UsageMeasurement::Unsupported) => 0,
        _ if tokens.cache_write == 0 => 0,
        None if rate.cache_write_12h_per_mtok.is_none()
            && context_step(rate, context)
                .is_none_or(|step| step.cache_write_12h_per_mtok.is_none()) =>
        {
            0
        }
        _ => return None,
    };
    token_charge_with_lifetimes(rate, tokens, context, tier, hourly, twelve_hour)
}

fn measured_counter(timing: &orch_api_contract::usage::RequestTiming, name: &str) -> Option<i128> {
    let measured = match name {
        "web_searches" => &timing.web_searches,
        "web_fetches" => &timing.web_fetches,
        _ => timing.declared_counters.get(name)?,
    };
    measured
        .value()
        .copied()
        .filter(|value| *value >= 0)
        .map(i128::from)
}

fn native_charge(
    quote: &orch_provider_contract::capabilities::pricing::NativePrice,
    timing: &orch_api_contract::usage::RequestTiming,
    unit: Option<&str>,
    scale: Dec,
) -> Option<Dec> {
    if quote.currency.is_empty() || Some(quote.currency.as_str()) != unit || quote.terms.is_empty()
    {
        return None;
    }
    let mut total = Dec::whole(0);
    for term in &quote.terms {
        let mut quantity = measured_counter(timing, &term.counter)?;
        if let Some(included) = &term.included {
            let allowance = measured_counter(timing, &included.counter)?
                .checked_mul(i128::from(included.each))?;
            quantity = quantity.checked_sub(allowance)?.max(0);
        }
        total = total.add(
            Dec::parse(&term.amount)?
                .mul(scale)?
                .mul(Dec::whole(quantity))?,
        )?;
    }
    Some(total)
}

/// Only catalogue pricing uses these request-selected terms. Explicit credit rates use the ordinary path.
fn credits_with_native_prices(
    rate: &Rate,
    tokens: &Tokens,
    context: Option<i64>,
    tier: Option<&str>,
    timing: &orch_api_contract::usage::RequestTiming,
    unit: Option<&str>,
    scale: Dec,
) -> Option<String> {
    use orch_api_contract::usage::UsageMeasurement;
    let mut total = token_charge_observed(rate, tokens, context, tier, timing)?;
    for (capability, count, price) in [
        (
            "web_search",
            &timing.web_searches,
            &rate.web_search_per_call,
        ),
        ("page_fetch", &timing.web_fetches, &rate.web_fetch_per_call),
    ] {
        let count = match count {
            UsageMeasurement::Told { value } if *value >= 0 => *value,
            UsageMeasurement::Unsupported if !timing.native_prices.contains_key(capability) => 0,
            _ => return None,
        };
        if count == 0 {
            continue;
        }
        let charge = match timing.native_prices.get(capability) {
            Some(quote) => native_charge(quote.as_ref()?, timing, unit, scale)?,
            None => Dec::parse(price.as_deref()?)?.mul(Dec::whole(i128::from(count)))?,
        };
        total = total.add(charge)?;
    }
    total.rounded(CREDIT_PLACES)?.text(CREDIT_PLACES)
}
fn finish_with_web_tools(
    rate: &Rate,
    mut total: Dec,
    searches: &orch_api_contract::usage::UsageMeasurement<i64>,
    fetches: &orch_api_contract::usage::UsageMeasurement<i64>,
) -> Option<String> {
    use orch_api_contract::usage::UsageMeasurement;
    for (count, price) in [
        (searches, &rate.web_search_per_call),
        (fetches, &rate.web_fetch_per_call),
    ] {
        let count = match count {
            UsageMeasurement::Told { value } if *value >= 0 => *value,
            UsageMeasurement::Unsupported => 0,
            _ => return None,
        };
        if count > 0 {
            total =
                total.add(Dec::parse(price.as_deref()?)?.mul(Dec::whole(i128::from(count)))?)?;
        }
    }
    total.rounded(CREDIT_PLACES)?.text(CREDIT_PLACES)
}

/// A declared native unit uses the same exact decimal kernel and row rounding, without a token-tier multiplier.
#[must_use]
pub fn credits_for_counter(price: &str, count: i64) -> Option<String> {
    if count < 0 {
        return None;
    }
    Dec::parse(price)?
        .mul(Dec::whole(i128::from(count)))?
        .rounded(CREDIT_PLACES)?
        .text(CREDIT_PLACES)
}

fn materialize_alias(rate: &mut Rate, tier: Option<&str>, alias: Option<&str>) {
    if let (Some(tier), Some(alias), Some(tiers)) = (tier, alias, rate.tiers.as_mut())
        && !tiers.contains_key(tier)
        && let Some(multiplier) = tiers.get(alias).cloned()
    {
        tiers.insert(tier.to_owned(), multiplier);
    }
}

/// The single existing exact decimal/kernel/rounding path for every priced row.
fn price_found(
    found: Found,
    model: &str,
    tokens: &Tokens,
    context: Option<i64>,
    tier: Option<&str>,
) -> Priced {
    match credits(&found.rate, tokens, context, tier) {
        Some(credits) => Priced {
            found: Some(found),
            credits: Some(credits),
        },
        None => {
            tracing::warn!(
                model,
                "a rate has a price that is not a decimal; the use is counted without one"
            );
            Priced::default()
        }
    }
}

/// The families of a catalogue document, each a pattern and a rate; an entry that does not read is left out.
fn families_of(document: &Json) -> Vec<(String, Rate)> {
    document
        .get("families")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|family| {
            let pattern = family.get("match")?.as_str()?.to_owned();
            let rate: Rate = serde_json::from_value(family.get("rate")?.clone()).ok()?;
            Some((pattern, rate))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_counter_quotes_use_exact_unit_prices_without_token_tier_multipliers() {
        assert_eq!(credits_for_counter("0.01", 3).as_deref(), Some("0.030000"));
        assert_eq!(
            credits_for_counter("0.0000005", 1).as_deref(),
            Some("0.000001")
        );
        assert_eq!(credits_for_counter("0.01", 0).as_deref(), Some("0.000000"));
        assert_eq!(credits_for_counter("bad", 2), None);
        assert_eq!(credits_for_counter("-1", 2), None);
        assert_eq!(credits_for_counter("1", -1), None);
    }
    #[test]
    fn separately_counted_rows_keep_their_original_rounding_instead_of_becoming_one_aggregate() {
        let one = credits_for_counter("0.0000005", 1).unwrap();
        assert_eq!(one, "0.000001");
        let rows = Dec::parse(&one)
            .unwrap()
            .mul(Dec::whole(3))
            .unwrap()
            .text(CREDIT_PLACES)
            .unwrap();
        assert_eq!(rows, "0.000003");
        assert_eq!(
            credits_for_counter("0.0000005", 3).as_deref(),
            Some("0.000002")
        );
    }
    use serde_json::json;

    fn rate(value: &Json) -> Rate {
        serde_json::from_value(value.clone()).unwrap()
    }

    fn tokens(input: i64, cached: i64, output: i64) -> Tokens {
        Tokens {
            input,
            cached_input: cached,
            output,
            total: input + output,
            ..Tokens::default()
        }
    }

    #[test]
    fn hourly_cache_is_a_subset_not_a_second_write_and_missing_hourly_prices_stay_unknown() {
        use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
        let quoted = rate(
            &json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","cache_write_per_mtok":"1.25",
            "cache_write_1h_per_mtok":"2","output_per_mtok":"2","web_search_per_call":"0.01","web_fetch_per_call":"0.02"}),
        );
        let mut used = tokens(1000, 100, 200);
        used.cache_write = 300;
        let mut timing = RequestTiming {
            cache_write_1h_tokens: Some(UsageMeasurement::Told { value: 200 }),
            ..Default::default()
        };
        assert_eq!(
            credits_with_timing(&quoted, &used, None, None, &timing),
            Some("0.001535".into())
        );
        timing.web_searches = UsageMeasurement::Told { value: 1 };
        timing.web_fetches = UsageMeasurement::Told { value: 2 };
        assert_eq!(
            credits_with_timing(&quoted, &used, None, None, &timing),
            Some("0.051535".into())
        );
        timing.web_searches = UsageMeasurement::Unsupported;
        timing.web_fetches = UsageMeasurement::Unsupported;
        let mut unquoted = quoted.clone();
        unquoted.cache_write_1h_per_mtok = None;
        assert!(credits_with_timing(&unquoted, &used, None, None, &timing).is_none());
        for state in [
            UsageMeasurement::Empty,
            UsageMeasurement::Unread,
            UsageMeasurement::Told { value: -1 },
            UsageMeasurement::Told { value: 301 },
        ] {
            timing.cache_write_1h_tokens = Some(state);
            assert!(credits_with_timing(&quoted, &used, None, None, &timing).is_none());
        }
        timing.cache_write_1h_tokens = Some(UsageMeasurement::Unsupported);
        assert_eq!(
            credits_with_timing(&quoted, &used, None, None, &timing),
            Some("0.001385".into())
        );
        timing.cache_write_1h_tokens = None;
        assert_eq!(
            credits_with_timing(&quoted, &used, None, None, &timing),
            credits(&quoted, &used, None, None)
        );
    }

    #[test]
    fn default_cache_writes_use_the_write_premium_and_twelve_hour_parts_are_disjoint() {
        use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
        let quoted = rate(&json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1",
            "output_per_mtok":"2","cache_write_per_mtok":"1.25","cache_write_12h_per_mtok":"2.5",
            "tiers":{"fast":"2"},"above":[{"context_tokens":272000,"input_per_mtok":"2",
            "cached_input_per_mtok":"0.2","output_per_mtok":"3","cache_write_per_mtok":"2.5",
            "cache_write_12h_per_mtok":"5"}]}));
        let mut used = tokens(1_000_000, 0, 0);
        used.cache_write = 1_000_000;
        let mut timing = RequestTiming {
            cache_write_12h_tokens: Some(UsageMeasurement::Told { value: 0 }),
            ..Default::default()
        };
        assert_eq!(
            credits_with_timing(&quoted, &used, None, None, &timing).as_deref(),
            Some("1.250000")
        );
        used = tokens(12000, 4000, 0);
        used.cache_write = 7000;
        timing.cache_write_12h_tokens = Some(UsageMeasurement::Told { value: 4000 });
        assert_eq!(
            credits_with_timing(&quoted, &used, None, None, &timing).as_deref(),
            Some("0.015150")
        );
        assert_eq!(
            credits_with_timing(&quoted, &used, None, Some("fast"), &timing).as_deref(),
            Some("0.030300")
        );
        assert_eq!(
            credits_with_timing(&quoted, &used, Some(272001), None, &timing).as_deref(),
            Some("0.030300")
        );
        let mut missing_quote = quoted.clone();
        missing_quote.cache_write_12h_per_mtok = None;
        assert!(credits_with_timing(&missing_quote, &used, None, None, &timing).is_none());
        for state in [
            None,
            Some(UsageMeasurement::Unread),
            Some(UsageMeasurement::Empty),
            Some(UsageMeasurement::Told { value: -1 }),
            Some(UsageMeasurement::Told { value: 7001 }),
        ] {
            timing.cache_write_12h_tokens = state;
            assert!(credits_with_timing(&quoted, &used, None, None, &timing).is_none());
        }
        timing.cache_write_1h_tokens = Some(UsageMeasurement::Told { value: 4000 });
        timing.cache_write_12h_tokens = Some(UsageMeasurement::Told { value: 4000 });
        assert!(credits_with_timing(&quoted, &used, None, None, &timing).is_none());
    }

    #[test]
    fn actual_capacity_and_inference_speed_price_independently_without_scaling_native_tool_fees() {
        use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
        let original = rate(
            &json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"2",
            "web_search_per_call":"0.01","tiers":{"fast":"2"},"capacity_tiers":{"priority":"3"}}),
        );
        let mut timing = RequestTiming {
            wire_protocol: Some("anthropic-messages".into()),
            requested_capacity_tier: Some("auto".into()),
            reported_capacity_tier: UsageMeasurement::Told {
                value: "priority".into(),
            },
            ..Default::default()
        };
        let applied = for_capacity_observation(&original, &timing, None).unwrap();
        assert_eq!(
            credits_with_searches(
                &applied,
                &tokens(1_000_000, 0, 0),
                None,
                Some("fast"),
                &UsageMeasurement::Told { value: 2 }
            ),
            Some("6.020000".into())
        );
        assert_eq!(original.input_per_mtok, "1");
        assert_eq!(original.capacity_tiers.as_ref().unwrap()["priority"], "3");
        timing.reported_capacity_tier = UsageMeasurement::Told {
            value: "standard".into(),
        };
        let standard = for_capacity_observation(&original, &timing, None).unwrap();
        assert_eq!(
            credits(&standard, &tokens(1_000_000, 0, 0), None, Some("fast")),
            Some("2.000000".into())
        );
        for state in [
            UsageMeasurement::Empty,
            UsageMeasurement::Unread,
            UsageMeasurement::Told {
                value: "unpriced".into(),
            },
        ] {
            timing.reported_capacity_tier = state;
            assert!(for_capacity_observation(&original, &timing, None).is_none());
        }
        timing.reported_capacity_tier = UsageMeasurement::Told {
            value: "priority".into(),
        };
        let mut unpriced = original.clone();
        unpriced.capacity_tiers = None;
        assert!(for_capacity_observation(&unpriced, &timing, None).is_none());
        assert_eq!(
            for_capacity_observation(&original, &RequestTiming::default(), None),
            Some(original)
        );
    }

    fn plain() -> Rate {
        rate(
            &json!({ "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15" }),
        )
    }

    #[test]
    fn reported_totals_replace_only_matching_catalogue_estimates_without_adding_search_quotes() {
        use orch_api_contract::usage::ReportedCost;
        let make = |source| Priced {
            found: Some(Found {
                rate: plain(),
                source,
            }),
            credits: Some("9.000000".into()),
        };
        let actual = ReportedCost {
            amount: "0.0071".into(),
            unit: Some("USD".into()),
        };
        let scale = Dec::parse("2").unwrap();
        let mut catalogue = make(Source::Catalogue);
        apply_reported_total(&mut catalogue, &actual, Some("USD"), scale);
        assert_eq!(catalogue.credits.as_deref(), Some("0.014200"));
        assert_eq!(catalogue.found.as_ref().unwrap().source, Source::Reported);
        assert!(!Source::Reported.is_estimate());
        for source in [Source::Provider, Source::RateCard] {
            let mut explicit = make(source);
            apply_reported_total(&mut explicit, &actual, Some("USD"), scale);
            assert_eq!(explicit, make(source));
        }
        for (unit, catalogue_unit) in [
            (Some("USD"), Some("EUR")),
            (None, Some("USD")),
            (Some("USD"), None),
            (Some(""), Some("")),
        ] {
            let mut unknown = make(Source::Catalogue);
            apply_reported_total(
                &mut unknown,
                &ReportedCost {
                    amount: "0.0071".into(),
                    unit: unit.map(str::to_owned),
                },
                catalogue_unit,
                scale,
            );
            assert_eq!(unknown, make(Source::Catalogue));
        }
        for amount in ["-1", "NaN", "not-a-cost"] {
            let mut invalid = make(Source::Catalogue);
            apply_reported_total(
                &mut invalid,
                &ReportedCost {
                    amount: amount.into(),
                    ..actual.clone()
                },
                Some("USD"),
                scale,
            );
            assert_eq!(invalid, make(Source::Catalogue));
        }
        let mut free = make(Source::Catalogue);
        apply_reported_total(
            &mut free,
            &ReportedCost {
                amount: "0".into(),
                ..actual.clone()
            },
            Some("USD"),
            scale,
        );
        assert_eq!(free.credits.as_deref(), Some("0.000000"));
    }

    #[test]
    fn native_fetches_and_searches_have_independent_exact_prices_without_tier_multiplication() {
        use orch_api_contract::usage::UsageMeasurement as Measured;
        let rate = rate(
            &json!({"input_per_mtok":"2.50","cached_input_per_mtok":"0.25","output_per_mtok":"15","tiers":{"fast":"2"},"web_search_per_call":"0.01","web_fetch_per_call":"0.00075"}),
        );
        let used = tokens(1_000_000, 0, 0);
        assert_eq!(
            credits_with_web_tools(
                &rate,
                &used,
                None,
                Some("fast"),
                &Measured::Told { value: 2 },
                &Measured::Told { value: 3 }
            )
            .as_deref(),
            Some("5.022250")
        );
        assert_eq!(
            credits_with_web_tools(
                &rate,
                &used,
                None,
                Some("fast"),
                &Measured::Told { value: 0 },
                &Measured::Told { value: 0 }
            )
            .as_deref(),
            Some("5.000000")
        );
        assert!(
            credits_with_web_tools(
                &rate,
                &used,
                None,
                None,
                &Measured::Told { value: 0 },
                &Measured::Unread
            )
            .is_none()
        );
        assert!(
            credits_with_web_tools(
                &rate,
                &used,
                None,
                None,
                &Measured::Told { value: 0 },
                &Measured::Told { value: -1 }
            )
            .is_none()
        );
        let mut missing = rate.clone();
        missing.web_fetch_per_call = None;
        assert!(
            credits_with_web_tools(
                &missing,
                &used,
                None,
                None,
                &Measured::Told { value: 0 },
                &Measured::Told { value: 1 }
            )
            .is_none()
        );
        assert_eq!(
            credits_with_searches(&rate, &used, None, None, &Measured::Told { value: 2 })
                .as_deref(),
            Some("2.520000")
        );
    }

    #[test]
    fn a_million_tokens_cost_exactly_what_the_rate_says() {
        assert_eq!(
            credits(&plain(), &tokens(1_000_000, 0, 0), None, None).unwrap(),
            "2.500000"
        );
        assert_eq!(
            credits(&plain(), &tokens(0, 0, 1_000_000), None, None).unwrap(),
            "15.000000"
        );
    }

    #[test]
    fn cached_input_is_charged_at_its_own_price_and_is_part_of_the_input() {
        // 800 fresh at 2.5, 200 cached at 0.25, 100 out at 15: (2000 + 50 + 1500) / 1e6.
        assert_eq!(
            credits(&plain(), &tokens(1_000, 200, 100), None, None).unwrap(),
            "0.003550"
        );
    }

    #[test]
    fn tokens_written_to_a_cache_are_charged_at_the_cache_price_or_as_input_when_it_has_none() {
        let with_price = rate(&json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10",
            "cache_write_per_mtok": "4"
        }));
        let counted = Tokens {
            input: 1_000_000,
            cached_input: 0,
            cache_write: 500_000,
            ..Tokens::default()
        };
        // 500 000 fresh at 2 and 500 000 written at 4.
        assert_eq!(
            credits(&with_price, &counted, None, None).unwrap(),
            "3.000000"
        );
        let without = rate(
            &json!({ "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10" }),
        );
        assert_eq!(credits(&without, &counted, None, None).unwrap(), "2.000000");
    }

    #[test]
    fn a_tier_is_a_multiple_and_a_tier_the_rate_does_not_list_is_the_ordinary_price() {
        let tiered = rate(&json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10",
            "tiers": { "fast": "2", "batch": "0.5" }
        }));
        let used = tokens(1_000_000, 0, 0);
        assert_eq!(
            credits(&tiered, &used, None, Some("fast")).unwrap(),
            "4.000000"
        );
        assert_eq!(
            credits(&tiered, &used, None, Some("batch")).unwrap(),
            "1.000000"
        );
        assert_eq!(
            credits(&tiered, &used, None, Some("flex")).unwrap(),
            "2.000000"
        );
        assert_eq!(credits(&tiered, &used, None, None).unwrap(), "2.000000");
    }

    #[test]
    fn a_prompt_above_a_size_pays_the_prices_of_the_largest_step_it_exceeds() {
        let stepped = rate(&json!({
            "input_per_mtok": "2.5", "cached_input_per_mtok": "0.25", "output_per_mtok": "15",
            "above": [
                { "context_tokens": 272000, "input_per_mtok": "5", "output_per_mtok": "22.5" },
                { "context_tokens": 1000000, "input_per_mtok": "9", "cached_input_per_mtok": "1", "output_per_mtok": "30" }
            ]
        }));
        let used = tokens(300_000, 0, 0);
        assert_eq!(
            credits(&stepped, &used, Some(100_000), None).unwrap(),
            "0.750000"
        );
        assert_eq!(
            credits(&stepped, &used, Some(272_000), None).unwrap(),
            "0.750000",
            "not above"
        );
        assert_eq!(
            credits(&stepped, &used, Some(272_001), None).unwrap(),
            "1.500000"
        );
        assert_eq!(
            credits(&stepped, &used, Some(1_000_001), None).unwrap(),
            "2.700000"
        );
        assert_eq!(
            credits(&stepped, &used, None, None).unwrap(),
            "0.750000",
            "unknown size: ordinary"
        );
    }

    #[test]
    fn equal_prices_do_not_make_distinct_context_bands_interchangeable() {
        let stepped = rate(&json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10",
            "above": [
                { "context_tokens": 100, "input_per_mtok": "4", "output_per_mtok": "20" },
                { "context_tokens": 200, "input_per_mtok": "2", "output_per_mtok": "10" }
            ]
        }));
        let used = tokens(1_000_000, 0, 0);
        assert_eq!(
            credits(&stepped, &used, Some(100), None),
            Some("2.000000".to_owned())
        );
        assert_eq!(
            credits(&stepped, &used, Some(150), None),
            Some("4.000000".to_owned())
        );
        assert_eq!(
            credits(&stepped, &used, Some(201), None),
            Some("2.000000".to_owned())
        );
        assert!(
            !same_context_band(&stepped, 100, 201),
            "equal endpoint prices hide an intermediate band"
        );
        assert!(same_context_band(&stepped, 0, 100));
        assert!(same_context_band(&stepped, 101, 200));
        assert!(same_context_band(&stepped, 201, 300));
        assert!(
            !same_context_band(&stepped, 100, 101),
            "threshold is strict"
        );
        assert!(!same_context_band(&stepped, 200, 201));
        assert!(
            !same_context_band(&stepped, 201, 100),
            "reversed bounds are not evidence"
        );
    }

    #[test]
    fn a_documented_tier_alias_fills_only_an_absent_actual_multiplier() {
        let alias = orch_model_contract::parameters::service_tier_price_alias(
            "openai-responses",
            "priority",
        );
        assert_eq!(alias, Some("fast"));
        let mut explicit = rate(&json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10",
            "tiers": { "fast": "2", "priority": "3" }
        }));
        let used = tokens(1_000_000, 0, 0);
        materialize_alias(&mut explicit, Some("priority"), alias);
        assert_eq!(
            credits(&explicit, &used, None, Some("priority")),
            Some("6.000000".to_owned())
        );
        assert_eq!(explicit.tiers.as_ref().unwrap()["priority"], "3");
        let mut absent = rate(&json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10",
            "tiers": { "fast": "2" }
        }));
        materialize_alias(&mut absent, Some("priority"), alias);
        assert_eq!(
            credits(&absent, &used, None, Some("priority")),
            Some("4.000000".to_owned())
        );
        assert_eq!(absent.tiers.as_ref().unwrap()["fast"], "2");
        assert_eq!(absent.tiers.as_ref().unwrap()["priority"], "2");
    }

    #[test]
    fn a_half_of_the_sixth_place_goes_up() {
        // One token at 2.5 per million is 0.0000025 credits.
        assert_eq!(
            credits(&plain(), &tokens(1, 0, 0), None, None).unwrap(),
            "0.000003"
        );
        // One token at 0.25 per million is 0.00000025, which rounds down.
        let cached = Tokens {
            input: 1,
            cached_input: 1,
            ..Tokens::default()
        };
        assert_eq!(credits(&plain(), &cached, None, None).unwrap(), "0.000000");
    }

    #[test]
    fn no_use_costs_nothing_and_a_price_that_is_not_a_decimal_costs_nothing_that_is_said() {
        assert_eq!(
            credits(&plain(), &Tokens::default(), None, None).unwrap(),
            "0.000000"
        );
        let broken = rate(
            &json!({ "input_per_mtok": "two", "cached_input_per_mtok": "1", "output_per_mtok": "1" }),
        );
        assert_eq!(credits(&broken, &tokens(10, 0, 0), None, None), None);
    }

    #[test]
    fn the_catalogues_prices_are_multiplied_by_the_scale_exactly() {
        let catalogue = rate(&json!({
            "input_per_mtok": "10", "cached_input_per_mtok": "0.25", "output_per_mtok": "50",
            "cache_write_per_mtok": "12.5", "tiers": { "batch": "0.5" },
            "above": [ { "context_tokens": 200000, "input_per_mtok": "20", "output_per_mtok": "75" } ]
        }));
        let scale = Dec::parse("0.92").unwrap();
        let scaled = scaled(&catalogue, scale).unwrap();
        assert_eq!(scaled.input_per_mtok, "9.2");
        assert_eq!(scaled.cached_input_per_mtok, "0.23");
        assert_eq!(scaled.output_per_mtok, "46");
        assert_eq!(scaled.cache_write_per_mtok.as_deref(), Some("11.5"));
        assert_eq!(scaled.tiers, catalogue.tiers, "a multiple is not a price");
        let step = &scaled.above.as_ref().unwrap()[0];
        assert_eq!(
            (step.context_tokens, step.input_per_mtok.as_str()),
            (200_000, "18.4")
        );
        assert_eq!(step.output_per_mtok, "69");
    }

    #[test]
    fn the_rate_a_forwarder_is_given_is_the_one_the_turns_tier_comes_to_and_counts_the_same() {
        let tiered = rate(&json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.5", "output_per_mtok": "8",
            "cache_write_per_mtok": "2.5", "tiers": { "fast": "2" }
        }));
        let fast = for_tier(&tiered, Some("fast")).unwrap();
        assert_eq!(
            (fast.input_per_mtok.as_str(), fast.output_per_mtok.as_str()),
            ("4", "16")
        );
        assert_eq!(fast.cache_write_per_mtok.as_deref(), Some("5"));
        assert_eq!(fast.tiers, None, "the multiple is not applied twice");
        let used = Tokens {
            input: 1_000,
            cached_input: 200,
            cache_write: 100,
            output: 50,
            ..Tokens::default()
        };
        assert_eq!(
            credits(&fast, &used, None, None),
            credits(&tiered, &used, None, Some("fast")),
            "the forwarder and the ledger count a turn at a tier alike"
        );
        let ordinary = for_tier(&tiered, None).unwrap();
        assert_eq!(ordinary.input_per_mtok, "2");
        assert_eq!(for_tier(&tiered, Some("slow")).unwrap().input_per_mtok, "2");
    }

    #[test]
    fn of_the_patterns_that_match_the_one_with_the_most_literal_characters_counts() {
        let card = vec![
            (
                "gpt-*".to_owned(),
                rate(
                    &json!({ "input_per_mtok": "1", "cached_input_per_mtok": "1", "output_per_mtok": "1" }),
                ),
            ),
            (
                "gpt-6-sol".to_owned(),
                rate(
                    &json!({ "input_per_mtok": "7", "cached_input_per_mtok": "7", "output_per_mtok": "7" }),
                ),
            ),
        ];
        assert_eq!(best(&card, "openai/gpt-6-sol").unwrap().input_per_mtok, "7");
        assert_eq!(best(&card, "gpt-5").unwrap().input_per_mtok, "1");
        assert!(best(&card, "claude-opus-5").is_none());
    }

    #[test]
    fn the_catalogue_file_is_read_by_family_and_an_entry_that_does_not_read_is_left_out() {
        let document = json!({ "format": 1, "families": [
            { "match": "claude-opus-5*", "rate": { "input_per_mtok": "5", "cached_input_per_mtok": "0.5", "output_per_mtok": "25" } },
            { "match": "broken*", "rate": { "input_per_mtok": 5 } },
            { "rate": { "input_per_mtok": "1", "cached_input_per_mtok": "1", "output_per_mtok": "1" } }
        ] });
        let families = families_of(&document);
        assert_eq!(families.len(), 1);
        assert_eq!(families[0].0, "claude-opus-5*");
    }
    #[test]
    fn native_search_fees_are_additive_exact_and_not_tier_multipliers() {
        use orch_api_contract::usage::UsageMeasurement;
        let rate = rate(
            &json!({"input_per_mtok":"2","cached_input_per_mtok":"0.5","output_per_mtok":"8",
            "tiers":{"fast":"2"},"web_search_per_call":"0.01"}),
        );
        let tokens = Tokens {
            input: 1000,
            total: 1000,
            ..Tokens::default()
        };
        assert_eq!(
            credits_with_searches(
                &rate,
                &tokens,
                Some(1000),
                Some("fast"),
                &UsageMeasurement::Told { value: 2 }
            )
            .as_deref(),
            Some("0.024000")
        );
        assert_eq!(
            credits_with_searches(&rate, &tokens, Some(1000), None, &UsageMeasurement::Unread),
            None
        );
        assert_eq!(
            for_tier(&rate, Some("fast"))
                .unwrap()
                .web_search_per_call
                .as_deref(),
            Some("0.01")
        );
        let mut missing = rate.clone();
        missing.web_search_per_call = None;
        assert_eq!(
            credits_with_searches(
                &missing,
                &tokens,
                Some(1000),
                None,
                &UsageMeasurement::Told { value: 1 }
            ),
            None
        );
    }

    #[test]
    fn conditional_native_quotes_need_actual_counts_and_use_one_exact_total_without_a_tier_multiplier()
     {
        use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
        let rate:Rate=serde_json::from_value(json!({"input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"99","tiers":{"fast":"9"}})).unwrap();
        let mut timing = RequestTiming {
            web_searches: UsageMeasurement::Told { value: 2 },
            ..Default::default()
        };
        timing.native_prices=serde_json::from_value(json!({"web_search":{"currency":"USD","terms":[
            {"amount":"0.007","counter":"web_searches"},
            {"amount":"0.001","counter":"web_search_results","included":{"counter":"web_searches","each":10}}
        ]}})).unwrap();
        assert_eq!(
            credits_with_native_prices(
                &rate,
                &Tokens::default(),
                None,
                Some("fast"),
                &timing,
                Some("USD"),
                Dec::ONE
            ),
            None,
            "result count is missing, not the requested maximum or number of citations"
        );
        timing.declared_counters.insert(
            "web_search_results".into(),
            UsageMeasurement::Told { value: 27 },
        );
        assert_eq!(
            credits_with_native_prices(
                &rate,
                &Tokens::default(),
                None,
                Some("fast"),
                &timing,
                Some("USD"),
                Dec::whole(2)
            )
            .as_deref(),
            Some("0.042000")
        );
        assert_eq!(
            credits_with_native_prices(
                &rate,
                &Tokens::default(),
                None,
                None,
                &timing,
                Some("EUR"),
                Dec::ONE
            ),
            None
        );
        assert_eq!(
            credits_with_timing(&rate, &Tokens::default(), None, None, &timing).as_deref(),
            Some("198.000000"),
            "the ordinary explicit-credit path ignores catalogue quote terms"
        );
        timing.native_prices.insert("web_search".into(), None);
        assert_eq!(
            credits_with_native_prices(
                &rate,
                &Tokens::default(),
                None,
                None,
                &timing,
                Some("USD"),
                Dec::ONE
            ),
            None,
            "automatic/native has no inferred engine price"
        );
        timing.web_searches = UsageMeasurement::Told { value: 0 };
        assert_eq!(
            credits_with_native_prices(
                &rate,
                &Tokens::default(),
                None,
                None,
                &timing,
                Some("USD"),
                Dec::ONE
            )
            .as_deref(),
            Some("0.000000")
        );
    }
}
