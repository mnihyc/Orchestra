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
//! the day.

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
}

impl Source {
    /// The word the ledger writes (`rate_source`).
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Provider => ledger::rate_source::PROVIDER,
            Self::RateCard => ledger::rate_source::RATE_CARD,
            Self::Catalogue => ledger::rate_source::CATALOGUE,
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
    output: Dec,
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
        let step: Option<&RateStep> = context_tokens.and_then(|context| {
            rate.above
                .iter()
                .flatten()
                .filter(|step| context > step.context_tokens)
                .max_by_key(|step| step.context_tokens)
        });
        let Some(step) = step else {
            return Some(Self {
                input,
                cached,
                cache_write,
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
    let prices = Prices::of(rate, context_tokens)?;
    let cached = tokens.cached_input.max(0);
    let written = tokens.cache_write.max(0);
    let fresh = (tokens.input - cached - written).max(0);
    let terms = [
        (fresh, prices.input),
        (cached, prices.cached),
        (written, prices.cache_write),
        (tokens.output.max(0), prices.output),
    ];
    let mut sum = Dec::whole(0);
    for (count, price) in terms {
        sum = sum.add(price.mul(Dec::whole(i128::from(count)))?)?;
    }
    let mut credits = sum.shifted(PRICE_PLACES)?;
    let multiple = match tier.and_then(|tier| rate.tiers.as_ref()?.get(tier)) {
        Some(multiple) => Dec::parse(multiple)?,
        None => Dec::ONE,
    };
    credits = credits.mul(multiple)?;
    credits.rounded(CREDIT_PLACES)?.text(CREDIT_PLACES)
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
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        None => None,
    };
    Some(Rate {
        input_per_mtok: price(&rate.input_per_mtok)?,
        cached_input_per_mtok: price(&rate.cached_input_per_mtok)?,
        output_per_mtok: price(&rate.output_per_mtok)?,
        cache_write_per_mtok: optional(&rate.cache_write_per_mtok)?,
        tiers: rate.tiers.clone(),
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
    /// The credits it comes to; none exactly when there is no rate.
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

/// The rates of a place that are not the provider's: its rate card, and the settings that govern the
/// catalogue. The catalogue itself is read only when nothing before it has a rate.
#[derive(Debug, Clone)]
pub struct RateBook {
    card: Vec<(String, Rate)>,
    use_catalogue: bool,
    scale: Dec,
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
        let Some(found) = self.find(conn, own, model).await? else {
            return Ok(Priced::default());
        };
        match credits(&found.rate, tokens, context_tokens, tier) {
            Some(credits) => Ok(Priced {
                found: Some(found),
                credits: Some(credits),
            }),
            None => {
                tracing::warn!(
                    model,
                    "a rate has a price that is not a decimal; the use is counted without one"
                );
                Ok(Priced::default())
            }
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

    fn plain() -> Rate {
        rate(
            &json!({ "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15" }),
        )
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
}
