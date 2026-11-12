//! Target defaults from the ordinary settings resolver and a document's own typed overlay.
//! No permission, provider choice admission, paid work, retry policy or cap is decided here.
use super::sources::Identity;
use crate::EngineError;
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::summaries::{
    SummaryKind, SummaryTarget, SummaryTargetOptions, SummaryTargets, SummaryTrigger,
};
use orch_db::{
    Conn,
    queries::{settings as stored, summaries},
};
use orch_settings::{
    Place, Snapshot, Wanted,
    locks::{Here, lock_above},
};
use orch_settings_contract::{
    provenance::{LockedBy, SourceScope},
    registry::{self, SettingDef, keys},
};
use orch_summarizer_contract::{
    descriptor::split_setting,
    summarizer::{SUMMARY_CHARS, SummaryLimits},
};
use orch_summarizer_model::descriptor::CONTEXT_TOKENS;
use orch_types::{Hash, Id, Json};
use serde::Deserialize;
use serde_json::json;
use std::{collections::BTreeSet, num::NonZeroU32};

/// Thread activity keeps the ordinary shared trigger default.
pub const THREAD_MIN_INTERVAL_SECS: u32 = 120;
/// A topic aggregates many changing threads; fifteen minutes avoids repeating every child event.
pub const TOPIC_MIN_INTERVAL_SECS: u32 = 900;
/// Document edits arrive in bursts; five minutes lets a group of edits settle.
pub const DOCUMENT_MIN_INTERVAL_SECS: u32 = 300;
/// Chat messages arrive in runs; catch-up and title automation share a five-minute default.
pub const CHAT_MIN_INTERVAL_SECS: u32 = 300;
/// Eligible/running work follows at five seconds by default; future work waits until its due time.
pub const FOLLOW_INTERVAL_SECS: u32 = 5;

/// Shipped pacing only. Explicit per-kind and shared settings always take precedence.
#[must_use]
pub const fn default_min_interval(kind: SummaryKind) -> u32 {
    match kind {
        SummaryKind::Thread => THREAD_MIN_INTERVAL_SECS,
        SummaryKind::Topic => TOPIC_MIN_INTERVAL_SECS,
        SummaryKind::Document => DOCUMENT_MIN_INTERVAL_SECS,
        SummaryKind::Chat | SummaryKind::ChatTitle => CHAT_MIN_INTERVAL_SECS,
        SummaryKind::Help | SummaryKind::SearchResults => 0,
    }
}

/// Private producer configuration selected by the existing auto.summarizer setting. No
/// Serialize implementation: public metadata still comes from the native admitted projection.
#[derive(Debug, Clone, PartialEq)]
pub struct Producer {
    /// Installed producer kind, independent of the target kind.
    pub kind: String,
    /// Its ordinary validated configuration.
    pub config: Json,
    /// The actual setting writer's current Use must be checked for automatic work.
    pub writer: Option<Id>,
}

/// Current typed target policy, independent of its producer and paying/requesting origin.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// UI-only free-read pacing; excluded from the paid-work policy fingerprint.
    pub follow_interval_secs: u32,
    /// Effective full target entry; a lower target entry replaces that member as ordinary Map does.
    pub options: SummaryTargetOptions,
    /// Actual target-entry writer, which can differ from an unrelated later family member.
    pub options_writer: Option<Id>,
    /// Document own-options CAS version; zero before an own settings row exists.
    pub own_revision: i64,
    /// Stored document override, including one currently silenced by an inherited lock.
    pub own: Option<SummaryTargetOptions>,
    /// Every existing ancestor lock is above an individual document, including its containing node.
    pub locked_by: Option<LockedBy<Id>>,
    /// The ordinary selected producer, unset as shipped.
    pub producer: Option<Producer>,
    /// Effective change events; an explicitly empty set stays empty.
    pub on: BTreeSet<SummaryTrigger>,
    /// Effective minimum gap; explicit zero remains zero.
    pub min_interval_secs: u32,
    /// Positive per-target input tokens, otherwise the producer's context default.
    pub context_tokens: NonZeroU32,
    /// Paragraph output default, never a usage-cap/provider allowance override.
    pub output_chars: NonZeroU32,
    /// Private current selection/provenance fingerprint for the effect/publication guard.
    pub fingerprint: Hash,
}

impl Policy {
    /// The paragraph request uses this target's positive output default without changing title/tag limits.
    #[must_use]
    pub fn limits(&self) -> SummaryLimits {
        SummaryLimits {
            max_chars: self.output_chars.get(),
            max_tags: 0,
        }
    }

    /// The automatic event's earliest due time. A change during the gap is delayed, never
    /// dropped. Unchanged inputs, disabled targets and unselected events queue no paid work.
    /// `last_act` is the last accepted automatic act, including failures; actual recovery and
    /// whether a failed source may run again remain with calls.rules and its durable journal.
    #[must_use]
    pub fn earliest(
        &self,
        event: SummaryTrigger,
        changed: bool,
        last_act: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        if !self.options.automatic
            || self.producer.is_none()
            || !changed
            || !self.on.contains(&event)
        {
            return None;
        }
        match last_act {
            Some(at) => at
                .checked_add_signed(Duration::seconds(i64::from(self.min_interval_secs)))
                .map(|due| now.max(due)),
            None => Some(now),
        }
    }
}

fn definition(key: &str) -> Result<&'static SettingDef, EngineError> {
    registry::get(key).ok_or_else(|| EngineError::unreadable("a summary setting is not registered"))
}

fn target_key(kind: SummaryKind) -> &'static str {
    match kind {
        SummaryKind::Thread => "thread",
        SummaryKind::Topic => "topic",
        SummaryKind::Document => "document",
        SummaryKind::Chat => "chat",
        SummaryKind::ChatTitle => "chat_title",
        SummaryKind::Help => "help",
        SummaryKind::SearchResults => "search_results",
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Triggers {
    on: BTreeSet<SummaryTrigger>,
    min_interval_secs: Option<u32>,
}

fn positive(value: u32) -> Result<NonZeroU32, EngineError> {
    NonZeroU32::new(value)
        .ok_or_else(|| EngineError::unreadable("a summary default must be positive"))
}

fn input_default(
    producer: Option<&Producer>,
    key: &str,
    fallback: u32,
) -> Result<NonZeroU32, EngineError> {
    let value = producer.and_then(|producer| producer.config.get(key));
    let number = match value {
        None => fallback,
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| EngineError::unreadable("a summary input default is invalid"))?,
    };
    positive(number)
}

/// Resolve from the actual native identity's place, never Place::Node(document_id).
/// This reads private configuration for a trusted native consumer; ordinary target/settings
/// Read and complete current source admission still govern any outward response.
pub async fn resolve(conn: &mut Conn, identity: &Identity) -> Result<Policy, EngineError> {
    let defs = [
        definition(keys::SUMMARY_TARGETS)?,
        definition(keys::AUTO_SUMMARIZER)?,
        definition(keys::AUTO_TRIGGERS)?,
        definition(keys::SUMMARY_FOLLOW_INTERVAL_SECS)?,
    ];
    let snapshot = Snapshot::load(conn, Place::Node(identity.place()), Wanted::Keys(&defs)).await?;
    let family = snapshot.resolve(defs[0]);
    let selected = snapshot.resolve(defs[1]);
    let triggers = snapshot.resolve(defs[2]);
    let follow: NonZeroU32 = serde_json::from_value(
        snapshot
            .resolve(defs[3])
            .value
            .unwrap_or_else(|| json!(FOLLOW_INTERVAL_SECS)),
    )?;
    let targets: SummaryTargets =
        serde_json::from_value(family.value.clone().unwrap_or_else(|| json!({})))?;
    let mut options = targets
        .generated(identity.target().kind())
        .cloned()
        .unwrap_or_default();
    let option_source = family
        .parts
        .iter()
        .rev()
        .find(|part| {
            part.value
                .get(target_key(identity.target().kind()))
                .is_some()
        })
        .map(|part| part.source.clone());
    let mut options_writer =
        match option_source.filter(|source| source.scope != SourceScope::Default) {
            Some(source) => stored::row(conn, source.node, keys::SUMMARY_TARGETS)
                .await?
                .and_then(|row| row.updated_by),
            None => None,
        };
    // Documents have one explicit own-resource overlay; ordinary node/thread choices stay
    // in summary.targets at their actual Place, never in a parallel cache settings store.
    let own_row = if matches!(identity.target(), SummaryTarget::Document { .. }) {
        summaries::get(conn, identity.target(), None).await?
    } else {
        None
    };
    let own = own_row
        .as_ref()
        .and_then(|row| row.own_options.as_ref())
        .map(|value| serde_json::from_value::<SummaryTargetOptions>(value.clone()))
        .transpose()?;
    let own_revision = own_row.as_ref().map_or(0, |row| row.options_revision);
    let here = if matches!(identity.target(), SummaryTarget::Document { .. }) {
        Here::Own
    } else {
        Here::Node(identity.place())
    };
    let locked_by = lock_above(here, family.locked_by.as_ref()).cloned();
    if locked_by.is_none()
        && let Some(own) = &own
    {
        options = own.clone();
        options_writer = own_row.as_ref().and_then(|row| row.options_updated_by);
    }
    let producer = match selected.value.as_ref().filter(|value| !value.is_null()) {
        Some(value) => {
            let (kind, config) = split_setting(value).map_err(EngineError::unreadable)?;
            let row = if selected.source.scope == SourceScope::Default {
                None
            } else {
                stored::row(conn, selected.source.node, keys::AUTO_SUMMARIZER).await?
            };
            Some(Producer {
                kind,
                config,
                writer: row.and_then(|row| row.updated_by),
            })
        }
        None => None,
    };
    // The registry's shared value is a thread default, not an explicit choice for every kind.
    let shared_is_set = triggers.source.scope != SourceScope::Default;
    let shared: Triggers = serde_json::from_value(triggers.value.unwrap_or_else(|| json!({})))?;
    let min_interval_secs = options
        .min_interval_secs
        .or(shared
            .min_interval_secs
            .filter(|_| shared_is_set || matches!(identity.target(), SummaryTarget::Thread { .. })))
        .unwrap_or(default_min_interval(identity.target().kind()));
    let on = options
        .on
        .clone()
        .unwrap_or_else(|| match identity.target() {
            SummaryTarget::Thread { .. } => shared.on,
            SummaryTarget::Topic { .. } => BTreeSet::from([SummaryTrigger::ChildChanged]),
            SummaryTarget::Document { .. } => BTreeSet::from([SummaryTrigger::ContentChanged]),
            // This is the automatic chat-rollup event. A person's unread catch-up stays an
            // explicit original act and never consults this automatic-event predicate.
            SummaryTarget::Chat { .. } | SummaryTarget::ChatTitle { .. } => {
                BTreeSet::from([SummaryTrigger::MessagePosted])
            }
        });
    let context_tokens = match options.context_tokens {
        Some(value) => value,
        None => input_default(producer.as_ref(), "context_tokens", CONTEXT_TOKENS)?,
    };
    let default_output = if matches!(identity.target(), SummaryTarget::ChatTitle { .. }) {
        SummaryLimits::for_field(orch_summarizer_contract::descriptor::SummaryField::Title, 0)
            .max_chars
    } else {
        SUMMARY_CHARS
    };
    let output_chars = options.output_chars.unwrap_or(positive(default_output)?);
    let fingerprint = Hash::of(&serde_json::to_vec(&json!({
        "target":identity.target(),"place":identity.place(),"options":options,"writer":options_writer,
        "own_revision":own_revision,"locked_by":locked_by,"events":on,"min_interval_secs":min_interval_secs,
        "context_tokens":context_tokens,"output_chars":output_chars,
        "producer":producer.as_ref().map(|producer|json!({"kind":producer.kind,"config":producer.config,"writer":producer.writer})),
    }))?);
    Ok(Policy {
        follow_interval_secs: follow.get(),
        options,
        options_writer,
        own_revision,
        own,
        locked_by,
        producer,
        on,
        min_interval_secs,
        context_tokens,
        output_chars,
        fingerprint,
    })
}
