//! Provider observation cadence from durable read attempts and recent bound-thread activity.

use crate::{Engine, EngineError};
use chrono::Duration;
use orch_db::queries::provider_facts as rows;

/// One minute bounds interactive vendor reads even when a vendor refuses them.
pub const PAGE_READ_INTERVAL: Duration = Duration::minutes(1);
/// Five minutes keeps plan percentages useful before a long turn is begun.
pub const ACTIVE_READ_INTERVAL: Duration = Duration::minutes(5);
/// A thread that has run in the last quarter hour keeps its provider on the active cadence.
pub const RECENT_ACTIVITY: Duration = Duration::minutes(15);
/// An hour keeps idle credentials and available resets current with twenty-four reads a day.
pub const IDLE_READ_INTERVAL: Duration = Duration::hours(1);
/// Deployment-selected provider observation intervals with the shipped default reasons above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadConfig {
    /// Interactive Refresh coalescing; ordinary page/list reads never observe a machine.
    pub page: Duration,
    /// Recent-work observation interval.
    pub active: Duration,
    /// Recent-work grace.
    pub recent: Duration,
    /// Idle observation interval.
    pub idle: Duration,
    /// Refused-credential recovery interval.
    pub refused: Duration,
}

impl ReadConfig {
    /// Reads the configured intervals; an unrepresentable duration keeps that interval's shipped bound.
    #[must_use]
    pub fn from_server(config: &orch_config::ServerConfig) -> Self {
        let span = |seconds, fallback| {
            Duration::from_std(std::time::Duration::from_secs(seconds)).unwrap_or(fallback)
        };
        Self {
            page: span(config.engine.provider_facts_page_secs, PAGE_READ_INTERVAL),
            active: span(
                config.engine.provider_facts_active_secs,
                ACTIVE_READ_INTERVAL,
            ),
            recent: span(
                config.engine.provider_facts_active_recent_secs,
                RECENT_ACTIVITY,
            ),
            idle: span(config.engine.provider_facts_idle_secs, IDLE_READ_INTERVAL),
            refused: span(
                config.engine.provider_facts_refused_secs,
                ACTIVE_READ_INTERVAL,
            ),
        }
    }
}

impl Default for ReadConfig {
    fn default() -> Self {
        Self::from_server(&orch_config::ServerConfig::default())
    }
}

/// A bounded batch keeps observation work inside the existing reaper's resource budget.
const READ_BATCH: i64 = 100;
/// Thirty-two retained failed deliveries bound the journal search; each HTTP retry sends at most one new delivery.
const RESET_DELIVERIES: u32 = 32;

/// Canonical facts on every stored/read boundary; independently discovered models and extensions stay.
pub fn normalize_observation(value: &orch_types::Json) -> Result<orch_types::Json, EngineError> {
    if value.is_null() {
        return Ok(value.clone());
    }
    let mut facts: orch_agent_contract::facts::ProviderFacts =
        serde_json::from_value(value.clone())?;
    orch_agent_contract::facts::normalize(&mut facts);
    let normalized = serde_json::to_value(facts)?;
    let mut combined = value.as_object().cloned().unwrap_or_default();
    if let Some(fields) = normalized.as_object() {
        combined.extend(fields.clone());
    }
    for legacy in ["plan_credits", "key_credits", "key_usage"] {
        combined.remove(legacy);
    }
    Ok(orch_types::Json::Object(combined))
}

/// Only the contract's told admission members hold work; money and percentages do not decide it.
#[must_use]
pub fn admission_exhausted(
    facts: &orch_agent_contract::facts::ProviderFacts,
    now: orch_types::Time,
) -> bool {
    use orch_provider_contract::{FactMember, MemberState, member_state};
    let ended = facts
        .limit_reached
        .as_ref()
        .and_then(|limit| limit.resets_at)
        .filter(|at| at.get() <= now.get());
    let old_denial_ended = ended.is_some_and(|ended| {
        facts
            .field_times
            .get("ordinary_usage_allowed")
            .is_none_or(|at| at.get() <= ended.get())
    });
    member_state(FactMember::OrdinaryUsageAllowed, facts, &[]) == MemberState::Told
        && facts.ordinary_usage_allowed == Some(false)
        && !old_denial_ended
        || member_state(FactMember::LimitReached, facts, &[]) == MemberState::Told
            && facts
                .limit_reached
                .as_ref()
                .is_some_and(|limit| limit.resets_at.is_none_or(|at| at.get() > now.get()))
}

/// Current people who manage a provider, including permission holders through groups and roles.
/// All provider-attention paths use the same expansion under their transaction's authority snapshot.
pub(crate) async fn managers(
    conn: &mut orch_db::Conn,
    provider: orch_types::Id,
    owner: orch_types::Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<orch_types::Id>, EngineError> {
    let (graph, _, site) = orch_authz::resources::site(
        conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Provider,
            id: provider,
        },
    )
    .await?;
    let principal = orch_authz::Principal::load(
        conn,
        orch_authz::Caller::session(owner),
        orch_types::Time::new(now),
    )
    .await?;
    let wanted = orch_access_contract::permissions::Permission::ProviderManage;
    let mut asked = vec![wanted];
    asked.extend(wanted.held_through());
    let frame = orch_authz::engine::Frame::load(
        conn,
        &principal,
        orch_authz::Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &asked,
    )
    .await?;
    Ok(orch_authz::holders::who_holds_for_notifications(conn, &frame, &site, wanted).await?)
}

/// Merges a by-the-way observation without erasing facts that its frame did not tell.
#[must_use]
pub fn merge_observation(known: &orch_types::Json, incoming: orch_types::Json) -> orch_types::Json {
    use orch_types::Json;
    let Some(update) = incoming.as_object() else {
        return known.clone();
    };
    let mut merged = known.as_object().cloned().unwrap_or_default();
    let known_times = known
        .get("field_times")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let supplied_times = incoming
        .get("field_times")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let current = |field: &str| match (known_times.get(field), supplied_times.get(field)) {
        (Some(old), Some(new)) => old
            .as_str()
            .zip(new.as_str())
            .is_none_or(|(old, new)| new >= old),
        _ => true,
    };
    for (key, value) in update {
        if key == "field_times" {
            continue;
        }
        // Recovery checks the new response before merging; an absent proof keeps its dated observation.
        if !current(key) || (value.is_null() && !supplied_times.contains_key(key)) {
            continue;
        }
        if matches!(key.as_str(), "windows" | "balances" | "spend")
            && !supplied_times.contains_key(key)
        {
            let mut windows = merged
                .get(key)
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default();
            for window in value.as_array().into_iter().flatten() {
                let Some(id) = window.get("id").and_then(Json::as_str) else {
                    continue;
                };
                if !current(&format!("{key}.{id}")) {
                    continue;
                }
                if let Some(index) = windows
                    .iter()
                    .position(|old| old.get("id").and_then(Json::as_str) == Some(id))
                {
                    windows[index] = window.clone();
                } else {
                    windows.push(window.clone());
                }
            }
            merged.insert(key.clone(), Json::Array(windows));
        } else if key == "credential"
            && !supplied_times.contains_key("credential")
            && value.is_object()
        {
            let mut credential = merged
                .get(key)
                .and_then(Json::as_object)
                .cloned()
                .unwrap_or_default();
            let partial = supplied_times
                .keys()
                .any(|field| field.starts_with("credential."));
            for (member, value) in value.as_object().into_iter().flatten() {
                let field = format!("credential.{member}");
                if (!partial || supplied_times.contains_key(&field)) && current(&field) {
                    credential.insert(member.clone(), value.clone());
                }
            }
            merged.insert(key.clone(), Json::Object(credential));
        } else if key == "extra_usage" && !supplied_times.contains_key("extra_usage") {
            let mut extra = merged
                .get(key)
                .and_then(Json::as_object)
                .cloned()
                .unwrap_or_default();
            for (member, value) in value.as_object().into_iter().flatten() {
                if !value.is_null() && current(&format!("extra_usage.{member}")) {
                    extra.insert(member.clone(), value.clone());
                }
            }
            if !extra.is_empty() {
                merged.insert(key.clone(), Json::Object(extra));
            }
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    let mut times = known_times.clone();
    for (field, time) in &supplied_times {
        if current(field) {
            times.insert(field.clone(), time.clone());
        }
    }
    // An explicit later admission ends the old limit even if this source does not repeat it as null.
    if update.get("ordinary_usage_allowed") == Some(&Json::Bool(true))
        && current("ordinary_usage_allowed")
        && update.get("limit_reached").is_none_or(Json::is_null)
        && known_times.get("limit_reached").is_none_or(|old| {
            supplied_times
                .get("ordinary_usage_allowed")
                .is_none_or(|new| new.as_str() >= old.as_str())
        })
    {
        merged.insert("limit_reached".to_owned(), Json::Null);
        if let Some(at) = supplied_times.get("ordinary_usage_allowed") {
            times.insert("limit_reached".to_owned(), at.clone());
        }
    }
    merged.insert("field_times".to_owned(), Json::Object(times));
    Json::Object(merged)
}

/// A custom facts source owns usage and expiry; a harness push still reports its keeper's identity and method.
#[must_use]
pub fn keeper_observation(incoming: orch_types::Json) -> orch_types::Json {
    use orch_types::Json;
    let mut kept = serde_json::Map::new();
    let mut times = serde_json::Map::new();
    let source_times = incoming.get("field_times").and_then(Json::as_object);
    if let Some(identity) = incoming.get("identity") {
        kept.insert("identity".into(), identity.clone());
        if let Some(at) = source_times.and_then(|times| times.get("identity")) {
            times.insert("identity".into(), at.clone());
        }
    }
    if let Some(credential) = incoming.get("credential").and_then(Json::as_object) {
        let mut fields = serde_json::Map::new();
        for member in ["method", "yields"] {
            if let Some(value) = credential.get(member) {
                fields.insert(member.into(), value.clone());
                if let Some(at) = source_times.and_then(|times| {
                    times
                        .get(&format!("credential.{member}"))
                        .or_else(|| times.get("credential"))
                }) {
                    times.insert(format!("credential.{member}"), at.clone());
                }
            }
        }
        if !fields.is_empty() {
            kept.insert("credential".into(), Json::Object(fields));
        }
    }
    kept.insert("field_times".into(), Json::Object(times));
    Json::Object(kept)
}

/// A source may verify only the provider credential that this binding actually gives it.
/// Default/older calls retain their connector's built-in proof rule; explicit bindings are narrowed here too.
#[must_use]
pub fn source_can_verify(sources: &orch_types::Json, native_keeper: bool) -> bool {
    use orch_types::Json;
    let Some(choice) = sources.get("facts") else {
        return true;
    };
    let by = choice.get("by").and_then(Json::as_str).unwrap_or("default");
    let wants_key = choice
        .pointer("/descriptor/facts/credential")
        .and_then(Json::as_str)
        == Some("key");
    match by {
        "default" if choice.get("descriptor").is_some_and(Json::is_object) => {
            if native_keeper {
                choice.pointer("/descriptor/name").and_then(Json::as_str) == Some("adapter")
            } else {
                wants_key
            }
        }
        "default" => true,
        "none" => false,
        "program" => !native_keeper && wants_key,
        "address" => {
            !native_keeper
                && wants_key
                && choice
                    .pointer("/address/send_credential")
                    .and_then(Json::as_bool)
                    == Some(true)
        }
        "builtin" if choice.get("name").and_then(Json::as_str) == Some("adapter") => native_keeper,
        "builtin" => !native_keeper && wants_key,
        _ => false,
    }
}

impl Engine {
    /// Runs credential-expiry upkeep on the engine's clock, including the short-token notice rule.
    pub async fn watch_provider_credential_ends(&self) -> Result<(), EngineError> {
        self.watch_token_ends().await
    }

    /// Reads the runtime's current facts for a reset confirmation without writing outside the caller's transaction.
    pub async fn inspect_provider_reset(
        &self,
        provider: orch_types::Id,
    ) -> Result<Option<orch_link_contract::commands::ProviderRefreshResult>, EngineError> {
        let (machine, facts_source) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(row) =
                orch_db::queries::resources::thread_engine::provider(&mut conn, provider)
                    .await?
                    .filter(|row| row.deleted_at.is_none())
            else {
                return Ok(None);
            };
            let Some(machine) = crate::leases::machine_of(&row) else {
                return Ok(None);
            };
            if !orch_db::queries::resources::thread_engine::machine_place(&mut conn, machine)
                .await?
                .is_some_and(|machine| {
                    machine.online && machine.status == "active" && machine.deleted_at.is_none()
                })
            {
                return Ok(None);
            }
            let source = crate::provider_sources::spec(
                &mut conn,
                provider,
                &row.sources,
                orch_provider_contract::SourceElement::Facts,
            )
            .await?;
            (machine, source)
        };
        let command = crate::commands::Outgoing::new(
            orch_types::new_id(),
            machine,
            orch_link_contract::commands::SerialKey::Provider(provider),
            &orch_link_contract::commands::ProviderRefresh {
                provider_id: provider,
                retry: None,
                renew: None,
                facts: facts_source,
            },
        )?;
        // The caller already holds authority; the command journal does not change it and must not
        // try to take that same exclusive lock in another transaction.
        match orch_db::queries::transaction_guard::scope(
            false,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await?
        {
            crate::commands::Outcome::Succeeded(result) => {
                let result: orch_link_contract::commands::ProviderRefreshResult =
                    serde_json::from_value(result)?;
                Ok((result.status != "not_told" || result.source.is_some()).then_some(result))
            }
            _ => Ok(None),
        }
    }

    /// Dispatches a confirmed reset to the provider runtime; the caller holds browser authority and the confirmation lock.
    pub async fn consume_provider_reset(
        &self,
        provider: orch_types::Id,
        reset_id: &str,
        user: orch_types::Id,
        request_id: orch_types::Id,
        available_count: i64,
    ) -> Result<Option<orch_link_contract::commands::ProviderResetConsumeResult>, EngineError> {
        let (machine, facts_source) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(row) =
                orch_db::queries::resources::thread_engine::provider(&mut conn, provider)
                    .await?
                    .filter(|row| row.deleted_at.is_none())
            else {
                return Ok(None);
            };
            let Some(machine) = crate::leases::machine_of(&row) else {
                return Ok(None);
            };
            if !orch_db::queries::resources::thread_engine::machine_place(&mut conn, machine)
                .await?
                .is_some_and(|machine| {
                    machine.online && machine.status == "active" && machine.deleted_at.is_none()
                })
            {
                return Ok(None);
            }
            let source = crate::provider_sources::spec(
                &mut conn,
                provider,
                &row.sources,
                orch_provider_contract::SourceElement::Facts,
            )
            .await?;
            (machine, source)
        };
        let operation = reset_operation(provider, user, request_id);
        let mut delivery = None;
        for attempt in 0..RESET_DELIVERIES {
            let id = if attempt == 0 {
                operation
            } else {
                crate::ids::derived(
                    "provider.reset.retry",
                    &[operation.as_bytes(), &attempt.to_le_bytes()],
                )
            };
            match self.inner.commands.status(id).await? {
                Some(crate::commands::Info {
                    outcome: Some(crate::commands::Outcome::Succeeded(value)),
                    ..
                }) if serde_json::from_value::<
                    orch_link_contract::commands::ProviderResetConsumeResult,
                >(value.clone())
                .ok()
                .is_some_and(|result| {
                    result.source.is_some_and(|source| {
                        source.error.is_some() && source.supported != Some(true)
                    })
                }) =>
                {
                    continue;
                }
                Some(info)
                    if matches!(
                        info.outcome,
                        Some(
                            crate::commands::Outcome::Failed(_)
                                | crate::commands::Outcome::Expired
                                | crate::commands::Outcome::Cancelled
                        )
                    ) =>
                {
                    continue;
                }
                _ => {
                    delivery = Some(id);
                    break;
                }
            }
        }
        let Some(delivery) = delivery else {
            return Ok(None);
        };
        let command = crate::commands::Outgoing::new(
            delivery,
            machine,
            orch_link_contract::commands::SerialKey::Provider(provider),
            &orch_link_contract::commands::ProviderResetConsume {
                provider_id: provider,
                reset_id: reset_id.to_owned(),
                request_id,
                available_count,
                facts: facts_source,
            },
        )?;
        match orch_db::queries::transaction_guard::scope(
            false,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await?
        {
            crate::commands::Outcome::Succeeded(result) => {
                Ok(Some(serde_json::from_value(result)?))
            }
            _ => Ok(None),
        }
    }

    /// Reads providers at the active or idle cadence, and never queues a read for an away machine.
    pub async fn refresh_due_provider_facts(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let due = {
            let mut conn = self.inner.db.acquire().await?;
            rows::due_reads(
                &mut conn,
                now,
                rows::ReadSchedule {
                    recent: self.inner.config.provider_facts.recent,
                    active: self.inner.config.provider_facts.active,
                    idle: self.inner.config.provider_facts.idle,
                    refused: self.inner.config.provider_facts.refused,
                },
                READ_BATCH,
            )
            .await?
        };
        for provider in due {
            let engine = self.clone();
            tokio::spawn(async move {
                if let Err(error) = engine.refresh_provider(provider).await {
                    tracing::debug!(%provider,%error,"a scheduled provider observation was not available");
                }
                if let Err(error) = engine.discover_provider_models(provider).await {
                    tracing::debug!(%provider,%error,"a scheduled model source observation was not available");
                }
            });
        }
        Ok(())
    }
}

/// One existing durable command identity for a person's confirmed reset, including a lost-response retry.
#[must_use]
pub fn reset_operation(
    provider: orch_types::Id,
    user: orch_types::Id,
    request: orch_types::Id,
) -> orch_types::Id {
    crate::ids::derived(
        "provider.reset",
        &[provider.as_bytes(), user.as_bytes(), request.as_bytes()],
    )
}
