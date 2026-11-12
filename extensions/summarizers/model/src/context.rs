//! Fair, token-budgeted material and stable cuts for consecutive summary prompts.
//!
//! Counts are planning estimates, never financial usage. Source owners provide immutable item
//! keys and already-redacted words; a stored cut supplies neither words nor read authority.
use crate::prompts;
use orch_summarizer_contract::{
    activity::{ActivityItem, SummaryInputFacts},
    summarizer::{SummarizeError, SummaryRequest},
};
use orch_types::Hash;
use serde::{Deserialize, Serialize};

/// A few sentences of detail for a long item before adding another older one.
/// Short items need only their actual tokens; a smaller total budget lowers this default.
pub const DETAIL_TOKENS: u32 = 64;
/// Evict an eighth-window at once, leaving append room instead of evicting one old item per refresh.
pub const TRIM_CHUNKS: u32 = 8;

/// Full input allowance and the actual rendered instruction/frame reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// Effective input tokens, at most the selected model window less its output.
    pub tokens: u32,
    /// Estimated tokens of the unchanged instruction, prior material and empty activity frame.
    pub frame: u32,
}
impl Budget {
    /// Keep all frame material; it is never silently cut to fit.
    pub fn new(tokens: u32, frame: u32) -> Result<Self, SummarizeError> {
        if tokens <= frame {
            return Err(SummarizeError::Config(
                "the context token budget cannot hold the summary instruction and frame".into(),
            ));
        }
        Ok(Self { tokens, frame })
    }
    /// Available activity tokens after framing.
    #[must_use]
    pub fn available(self) -> u32 {
        self.tokens.saturating_sub(self.frame)
    }
    /// Weight used while reading recent source pages; no older item can fit below this floor.
    #[must_use]
    pub fn detail(self, item: &ActivityItem) -> u32 {
        item_tokens(item).min(DETAIL_TOKENS.min(self.available()))
    }
}

/// One native source item. The key names the actual item, never a synthetic actor or governor.
pub struct InputItem {
    /// Native stable item identity, local to this selected resource history.
    pub key: String,
    /// Current redacted source text.
    pub item: ActivityItem,
}

/// A retained cut, without reusable source text or permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cut {
    key: String,
    hash: Hash,
    chars: u64,
    shortened: bool,
}
/// Immutable item cuts in oldest-to-newest prompt order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The actual input/frame allowance under which these cuts were chosen.
    pub budget: Budget,
    cuts: Vec<Cut>,
}
impl Plan {
    /// Reading through the oldest retained item permits a stable append without loading all history.
    #[must_use]
    pub fn oldest_key(&self) -> Option<&str> {
        self.cuts.first().map(|cut| cut.key.as_str())
    }
}
/// Current source words and factual coverage selected under a token budget.
pub struct Filled {
    /// Already cut activity, oldest to newest, ready for the ordinary escaped prompt renderer.
    pub activity: Vec<ActivityItem>,
    /// Cut positions for the next authorized refresh.
    pub plan: Plan,
    /// How much of the actual eligible source was represented.
    pub facts: SummaryInputFacts,
}

/// Text estimate shared by summary framing, fair allocation and final checks.
/// Like semantic text planning, four ASCII characters or one other character count as a token.
#[must_use]
pub fn estimated_tokens(text: &str) -> u32 {
    let (ascii, other) = text.chars().fold((0_u64, 0_u64), |(ascii, other), ch| {
        if ch.is_ascii() {
            (ascii + 1, other)
        } else {
            (ascii, other + 1)
        }
    });
    u32::try_from(ascii.div_ceil(4).saturating_add(other)).unwrap_or(u32::MAX)
}

/// Count the actual escaped item line, including its label and a conservative separator.
#[must_use]
pub fn item_tokens(item: &ActivityItem) -> u32 {
    estimated_tokens(&format!(
        "{}{}\n",
        prompts::activity_label(item.kind),
        prompts::material(&item.text)
    ))
}

/// Reserve the real configured prompt, prior material and tag choices before selecting activity.
pub fn request_budget(request: &SummaryRequest, tokens: u32) -> Result<Budget, SummarizeError> {
    let config = crate::descriptor::Config::read(&request.config)?;
    let (system, input) = prompts::request_frame(request, &config);
    Budget::new(
        tokens,
        estimated_tokens(&system).saturating_add(estimated_tokens(&input)),
    )
}

/// Ordinary resource summaries deliberately have no previous paragraph or tag context.
pub fn resource_budget(
    config: &crate::descriptor::Config,
    field: orch_summarizer_contract::descriptor::SummaryField,
    limits: &orch_summarizer_contract::summarizer::SummaryLimits,
) -> Result<Budget, SummarizeError> {
    let (system, input) = prompts::resource_frame(field, limits, config);
    Budget::new(
        config.context_tokens,
        estimated_tokens(&system).saturating_add(estimated_tokens(&input)),
    )
}

/// Use the configured input budget up to the model's declared window after output is reserved.
pub fn fit_window(tokens: u32, output: i64, window: Option<i64>) -> Result<u32, SummarizeError> {
    let Some(window) = window.filter(|window| *window > 0) else {
        return Ok(tokens);
    };
    let available = window.saturating_sub(output);
    if available <= 0 {
        return Err(SummarizeError::Config(
            "the model window has no input room after reserving output".into(),
        ));
    }
    Ok(tokens.min(u32::try_from(available).unwrap_or(u32::MAX)))
}

struct Item {
    source: InputItem,
    hash: Hash,
    tokens: u32,
}

fn hash(item: &ActivityItem) -> Result<Hash, SummarizeError> {
    serde_json::to_vec(item)
        .map(|body| Hash::of(&body))
        .map_err(|error| SummarizeError::Internal(error.to_string()))
}

fn retained(item: &Item, cut: &Cut) -> Option<ActivityItem> {
    if cut.key != item.source.key || cut.hash != item.hash {
        return None;
    }
    let chars = usize::try_from(cut.chars).ok()?;
    let mut text: String = item.source.item.text.chars().take(chars).collect();
    if text.chars().count() != chars {
        return None;
    }
    if cut.shortened {
        text.push('…');
    }
    Some(ActivityItem {
        text,
        ..item.source.item.clone()
    })
}

fn cut(item: &Item, tokens: u32) -> Option<(ActivityItem, Cut)> {
    if item.tokens <= tokens {
        return Some((
            item.source.item.clone(),
            Cut {
                key: item.source.key.clone(),
                hash: item.hash,
                chars: u64::try_from(item.source.item.text.chars().count()).unwrap_or(u64::MAX),
                shortened: false,
            },
        ));
    }
    // Leave room for the explicit cut mark; escaping is counted before choosing a UTF-8 boundary.
    let mut ascii = u64::try_from(prompts::activity_label(item.source.item.kind).len())
        .ok()?
        .saturating_add(1);
    let mut other = 1_u64;
    let mut text = String::new();
    let mut chars = 0_u64;
    for ch in item.source.item.text.chars() {
        let (a, o) = match ch {
            '&' => (5, 0),
            '<' | '>' => (4, 0),
            ch if ch.is_ascii() => (1, 0),
            _ => (0, 1),
        };
        if ascii
            .saturating_add(a)
            .div_ceil(4)
            .saturating_add(other)
            .saturating_add(o)
            > u64::from(tokens)
        {
            break;
        }
        ascii += a;
        other += o;
        chars += 1;
        text.push(ch);
    }
    if text.is_empty() {
        return None;
    }
    text.push('…');
    Some((
        ActivityItem {
            text,
            ..item.source.item.clone()
        },
        Cut {
            key: item.source.key.clone(),
            hash: item.hash,
            chars,
            shortened: true,
        },
    ))
}

/// Largest common water level, then one spare token per recent long item.
fn levels(costs: &[u32], budget: u32) -> Vec<u32> {
    let mut low = 0_u32;
    let mut high = costs.iter().copied().max().unwrap_or(0).min(budget);
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        let cost: u64 = costs
            .iter()
            .map(|tokens| u64::from((*tokens).min(middle)))
            .sum();
        if cost <= u64::from(budget) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let mut out: Vec<_> = costs.iter().map(|tokens| (*tokens).min(low)).collect();
    let used: u64 = out.iter().map(|tokens| u64::from(*tokens)).sum();
    let mut spare = u64::from(budget).saturating_sub(used);
    for (level, tokens) in out.iter_mut().zip(costs) {
        if spare == 0 {
            break;
        }
        if *level < *tokens {
            *level += 1;
            spare -= 1;
        }
    }
    out
}

fn fresh(items: &[Item], budget: Budget) -> Result<Vec<(ActivityItem, Cut)>, SummarizeError> {
    let mut used = 0_u64;
    let floor = DETAIL_TOKENS.min(budget.available());
    let count = items
        .iter()
        .take_while(|item| {
            let next = used + u64::from(item.tokens.min(floor));
            if next > u64::from(budget.available()) {
                return false;
            }
            used = next;
            true
        })
        .count();
    let selected = &items[..count];
    let mut out = Vec::new();
    for (item, level) in selected.iter().zip(levels(
        &selected.iter().map(|item| item.tokens).collect::<Vec<_>>(),
        budget.available(),
    )) {
        out.push(cut(item, level).ok_or_else(|| {
            SummarizeError::Config("the context token budget cannot hold an activity item".into())
        })?);
    }
    out.reverse();
    Ok(out)
}

fn append(items: &[Item], budget: Budget, previous: &Plan) -> Option<Vec<(ActivityItem, Cut)>> {
    if previous.budget != budget || previous.cuts.is_empty() {
        return None;
    }
    let cuts: std::collections::BTreeMap<_, _> = previous
        .cuts
        .iter()
        .map(|cut| (cut.key.as_str(), cut))
        .collect();
    let mut known: Vec<_> = items
        .iter()
        .map(|item| {
            cuts.get(item.source.key.as_str())
                .and_then(|cut| retained(item, cut).map(|text| (text, (*cut).clone())))
        })
        .collect();
    // The current newest prefix through the last surviving old item; older omitted material stays omitted.
    let mut end = known.iter().rposition(Option::is_some)? + 1;
    let minimum: Vec<u64> = items[..end]
        .iter()
        .zip(&known)
        .map(|(item, old)| {
            old.as_ref().map_or(
                u64::from(item.tokens.min(DETAIL_TOKENS.min(budget.available()))),
                |(text, _)| u64::from(item_tokens(text)),
            )
        })
        .collect();
    let used: u64 = minimum.iter().sum();
    let available = u64::from(budget.available());
    if used > available {
        let release = (used - available).max(available.div_ceil(u64::from(TRIM_CHUNKS)));
        let mut freed = 0_u64;
        while end > 0 && freed < release {
            end -= 1;
            freed = freed.saturating_add(minimum[end]);
        }
    }
    let kept_tokens: u64 = known[..end]
        .iter()
        .flatten()
        .map(|(text, _)| u64::from(item_tokens(text)))
        .sum();
    if kept_tokens > available {
        return None;
    }
    let costs: Vec<_> = items[..end]
        .iter()
        .zip(&known)
        .filter(|(_, old)| old.is_none())
        .map(|(item, _)| item.tokens)
        .collect();
    let mut assigned = levels(&costs, u32::try_from(available - kept_tokens).ok()?).into_iter();
    let mut out = Vec::new();
    for (index, item) in items[..end].iter().enumerate() {
        out.push(match known[index].take() {
            Some(old) => old,
            None => cut(item, assigned.next()?)?,
        });
    }
    out.reverse();
    Some(out)
}

/// Select newest material fairly, then render oldest first. Unchanged retained items reuse
/// their cuts; changed item versions are filled again without reallocating surviving items.
pub fn fill(
    items: Vec<InputItem>,
    budget: Budget,
    previous: Option<&Plan>,
    total_items: u64,
) -> Result<Filled, SummarizeError> {
    Budget::new(budget.tokens, budget.frame)?;
    let mut current = Vec::new();
    for mut source in items {
        source.item.text = source.item.text.trim().to_owned();
        if source.item.text.is_empty() {
            continue;
        }
        current.push(Item {
            hash: hash(&source.item)?,
            tokens: item_tokens(&source.item),
            source,
        });
    }
    current.reverse();
    let selected = match previous.and_then(|plan| append(&current, budget, plan)) {
        Some(selected) => selected,
        None => fresh(&current, budget)?,
    };
    let facts = SummaryInputFacts {
        included_items: u64::try_from(selected.len()).unwrap_or(u64::MAX),
        shortened_items: u64::try_from(selected.iter().filter(|(_, cut)| cut.shortened).count())
            .unwrap_or(u64::MAX),
        omitted_items: total_items
            .saturating_sub(u64::try_from(selected.len()).unwrap_or(u64::MAX)),
    };
    let (activity, cuts) = selected.into_iter().unzip();
    Ok(Filled {
        activity,
        plan: Plan { budget, cuts },
        facts,
    })
}

#[cfg(test)]
mod tests;
