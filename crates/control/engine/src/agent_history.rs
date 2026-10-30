//! Managed native history inventories, idle removal and durable reconstruction fences.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use orch_agent_contract::history::{NativeHistoryFact, NativeHistorySource, NativePrefixSource};
use orch_db::queries::{agent_history as rows, resources::thread_engine as resources, threads};
use orch_link_contract::commands::{AgentHistoryManage, AgentHistoryManageResult, SerialKey};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, new_id};

use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::harnesses::HarnessKey;

mod notices;
mod recovery;
mod sides;

/// Reuse the retention runner's ordinary metadata page size for batched home inventories.
const PAGE: i64 = 200;
/// The registry's shipped value is used only when a setting has no resolved row.
const DEFAULT_IDLE_DAYS: i64 = 7;

/// Pending native cache work is coalesced without holding the supervisor behind machine I/O.
#[derive(Default)]
pub(crate) struct Memory {
    task: Option<tokio::task::JoinHandle<()>>,
    full: bool,
    closed: bool,
}

fn source(row: &rows::History) -> NativePrefixSource {
    NativePrefixSource {
        thread_id: row.thread_id,
        segment_id: row.id,
        native_session_ref: row.native_session_ref.clone(),
    }
}

/// Zero disables removal; very large configured durations do not overflow into an immediate purge.
#[must_use]
pub fn idle_due(last_turn: DateTime<Utc>, now: DateTime<Utc>, days: i64) -> bool {
    days > 0
        && chrono::Duration::try_days(days)
            .and_then(|age| last_turn.checked_add_signed(age))
            .is_some_and(|due| due <= now)
}

impl Engine {
    /// Schedules one cache worker; a full pass also covers every pending close observation.
    pub(crate) fn schedule_agent_history(&self, full: bool) {
        if self.is_stopped() {
            return;
        }
        let mut memory = lock(&self.inner.agent_history);
        if full {
            memory.full = true;
        } else {
            memory.closed = true;
        }
        if memory.task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let engine = self.clone();
        memory.task = Some(tokio::spawn(async move {
            loop {
                let full = {
                    let mut memory = lock(&engine.inner.agent_history);
                    if engine.is_stopped() {
                        memory.task = None;
                        return;
                    }
                    if memory.full {
                        memory.full = false;
                        memory.closed = false;
                        true
                    } else if memory.closed {
                        memory.closed = false;
                        false
                    } else {
                        memory.task = None;
                        return;
                    }
                };
                let result = if full {
                    engine.retain_agent_history().await
                } else {
                    engine.inventory_closed_agent_history().await
                };
                if let Err(error) = result {
                    tracing::warn!(%error, "native cache work will be examined at its next interval");
                }
            }
        }));
    }

    /// Shutdown cancels this process's cache worker; durable command claims remain for reconciliation.
    pub(crate) fn stop_agent_history(&self) {
        if let Some(task) = lock(&self.inner.agent_history).task.take() {
            task.abort();
        }
    }

    /// Measures native history in batches per home and stores results for later request-only reads.
    pub async fn inventory_agent_history(&self) -> Result<(), EngineError> {
        self.inventory_agent_history_inner(false).await
    }

    /// Successful session-close receipts trigger a fresh batched measurement on the reaper's ordinary pace.
    pub async fn inventory_closed_agent_history(&self) -> Result<(), EngineError> {
        self.inventory_agent_history_inner(true).await
    }

    async fn inventory_agent_history_inner(&self, closed_only: bool) -> Result<(), EngineError> {
        let lease = orch_db::queries::job_leases::Lease::for_run(
            "agent-history-inventory",
            crate::schedule::RUN_LEASE,
        );
        if !lease.hold(self.db(), self.inner.clock.now()).await? {
            return Ok(());
        }
        let result = self.inventory_agent_history_held(closed_only, &lease).await;
        lease.release(self.db()).await?;
        result
    }

    async fn inventory_agent_history_held(
        &self,
        closed_only: bool,
        lease: &orch_db::queries::job_leases::Lease,
    ) -> Result<(), EngineError> {
        let pass = new_id();
        let mut visited = BTreeSet::new();
        let mut after = None;
        loop {
            let histories = if closed_only {
                rows::closed_page(&mut *self.db().acquire().await?, after, PAGE).await?
            } else {
                rows::page(&mut *self.db().acquire().await?, after, PAGE).await?
            };
            if histories.is_empty() {
                break;
            }
            after = histories.last().map(|row| row.id);
            let mut homes = BTreeMap::<(Id, Id, Id), Vec<rows::History>>::new();
            let mut unavailable = Vec::new();
            for row in histories {
                if let Some(machine) = row.machine_id {
                    homes
                        .entry((machine, row.provider_id, row.agent_id))
                        .or_default()
                        .push(row);
                } else {
                    unavailable.push(row);
                }
            }
            if !unavailable.is_empty() {
                let mut tx = self.db().begin().await?;
                let observations: Vec<_> = unavailable
                    .into_iter()
                    .map(|row| rows::Observation {
                        segment: row.id,
                        native: row.native_session_ref,
                        bytes: None,
                        removed: false,
                        error: Some("history.machine_unavailable".into()),
                        revision: None,
                    })
                    .collect();
                rows::measured_batch(&mut tx, &observations, self.inner.clock.now()).await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
            }
            for ((machine, provider, agent), histories) in homes {
                if !lease.hold(self.db(), self.inner.clock.now()).await? {
                    return Ok(());
                }
                let key = HarnessKey {
                    machine,
                    provider,
                    agent,
                };
                let outcome = self
                    .agent_history_command(
                        key,
                        histories
                            .iter()
                            .map(|row| NativeHistorySource {
                                source: source(row),
                                started_by_orch: row.started_by_orch,
                                inventory_pass: Some(pass),
                            })
                            .collect(),
                        false,
                        new_id(),
                        true,
                    )
                    .await;
                // A refusal that asking again would not change settles what it was asked of until the next close:
                // asked again at every pass, a home whose harness keeps its sign-in on the machine was refused
                // hundreds of times an hour.
                let (facts, failure, refused) = match outcome {
                    Ok(Outcome::Succeeded(value)) => {
                        visited.insert((machine, provider, agent));
                        (
                            serde_json::from_value::<AgentHistoryManageResult>(value)?.facts,
                            None,
                            false,
                        )
                    }
                    Ok(Outcome::Failed(failure)) => {
                        (Vec::new(), Some(failure.code), !failure.retryable)
                    }
                    Ok(_) => (Vec::new(), None, false),
                    Err(error) => {
                        tracing::warn!(%error, provider=%provider, "native history inventory could not be measured");
                        (Vec::new(), None, false)
                    }
                };
                let mut tx = self.db().begin().await?;
                let observations: Vec<_> = histories
                    .into_iter()
                    .map(|row| {
                        let found = facts.iter().find(|fact| fact.source == source(&row));
                        let error = found
                            .and_then(|fact| fact.error.as_deref())
                            .or(failure.as_deref())
                            .or_else(|| {
                                found.is_none().then_some("history.measurement_unavailable")
                            });
                        let settled = error.is_none()
                            || (refused && found.is_none())
                            || matches!(
                                error,
                                Some(
                                    "history.not_owned" | "unsupported" | "history.unmanaged_home"
                                )
                            );
                        rows::Observation {
                            segment: row.id,
                            native: row.native_session_ref,
                            bytes: found.and_then(|fact| fact.bytes),
                            removed: found.is_some_and(|fact| fact.removed),
                            error: error.map(str::to_owned),
                            revision: settled.then_some(row.measure_revision),
                        }
                    })
                    .collect();
                rows::measured_batch(&mut tx, &observations, self.inner.clock.now()).await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
            }
        }
        self.release_history_inventories(visited).await?;
        Ok(())
    }

    async fn release_history_inventories(
        &self,
        homes: BTreeSet<(Id, Id, Id)>,
    ) -> Result<(), EngineError> {
        for (machine, provider, agent) in homes {
            let command = Outgoing::new(
                new_id(),
                machine,
                SerialKey::Harness {
                    provider_id: provider,
                    agent_id: agent,
                },
                &AgentHistoryManage {
                    provider_id: provider,
                    agent_id: agent,
                    sources: Vec::new(),
                    remove: false,
                },
            )?;
            // Releasing an in-memory snapshot needs no online wait and changes no native files.
            self.inner.commands.issue(command).await?;
        }
        Ok(())
    }

    async fn cleanup_agent_history_homes(&self) -> Result<(), EngineError> {
        let days = setting::<i64>(
            &mut *self.db().acquire().await?,
            Place::System,
            keys::RETENTION_AGENT_HISTORY_IDLE_DAYS,
        )
        .await?
        .unwrap_or(DEFAULT_IDLE_DAYS);
        let Some(before) = chrono::Duration::try_days(days)
            .filter(|_| days > 0)
            .and_then(|age| self.inner.clock.now().checked_sub_signed(age))
        else {
            return Ok(());
        };
        let programs = orch_db::programs::registry(&mut *self.db().acquire().await?).await?;
        let kinds: Vec<_> = programs
            .programs()
            .filter(|program| program.files.managed_home_cleanup)
            .map(|program| program.kind.clone())
            .collect();
        if kinds.is_empty() {
            return Ok(());
        }
        let mut after = None;
        loop {
            let homes =
                rows::home_page(&mut *self.db().acquire().await?, &kinds, after, PAGE).await?;
            let Some(last) = homes.last() else {
                break;
            };
            after = Some((last.machine, last.provider, last.agent));
            for home in homes {
                if !home.eligible {
                    continue;
                }
                let (machine, provider, agent) = (home.machine, home.provider, home.agent);
                let command = Outgoing::new(
                    new_id(),
                    machine,
                    SerialKey::Harness {
                        provider_id: provider,
                        agent_id: agent,
                    },
                    &orch_link_contract::commands::AgentHistoryHomeCleanup {
                        provider_id: provider,
                        agent_id: agent,
                        before: orch_types::Time::new(before),
                    },
                )?;
                match crate::commands::run(self.inner.commands.as_ref(), command).await? {
                    Outcome::Succeeded(_) => {}
                    other => {
                        tracing::info!(provider=%provider,agent=%agent,?other,"old home caches were not confirmed removed")
                    }
                }
            }
        }
        Ok(())
    }

    /// The ordinary retention run applies each thread's current idle setting, including dormant threads.
    pub async fn retain_agent_history(&self) -> Result<(), EngineError> {
        self.reconcile_agent_history_removals().await?;
        self.inventory_agent_history().await?;
        let pass = new_id();
        let mut visited = BTreeSet::new();
        let mut after = None;
        loop {
            let histories = rows::page(&mut *self.db().acquire().await?, after, PAGE).await?;
            if histories.is_empty() {
                break;
            }
            after = histories.last().map(|row| row.id);
            for row in histories {
                if !row.owned || row.claim.is_some() {
                    continue;
                }
                let mut tx = self.db().begin().await?;
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                let Some(_) = threads::lock(&mut tx, row.thread_id).await? else {
                    continue;
                };
                let days = setting::<i64>(
                    &mut tx,
                    Place::Node(row.thread_id),
                    keys::RETENTION_AGENT_HISTORY_IDLE_DAYS,
                )
                .await?
                .unwrap_or(DEFAULT_IDLE_DAYS);
                if !idle_due(row.last_turn_at, self.inner.clock.now(), days) {
                    continue;
                }
                let command = new_id();
                if !rows::claim(&mut tx, &row, command, None, self.inner.clock.now()).await? {
                    continue;
                }
                tx.commit().await.map_err(orch_db::DbError::from)?;
                if let Some(machine) = row.machine_id {
                    visited.insert((machine, row.provider_id, row.agent_id));
                }
                self.finish_agent_history_removal(row, command, Some(pass))
                    .await?;
            }
        }
        self.release_history_inventories(visited).await?;
        self.cleanup_agent_history_homes().await?;
        Ok(())
    }

    async fn agent_history_command(
        &self,
        key: HarnessKey,
        sources: Vec<NativeHistorySource>,
        remove: bool,
        command: Id,
        check_home: bool,
    ) -> Result<Outcome, EngineError> {
        if check_home {
            {
                let mut conn = self.db().acquire().await?;
                let machine = resources::machine_place(&mut conn, key.machine).await?;
                if !machine.is_some_and(|machine| {
                    machine.online && machine.status == "active" && machine.deleted_at.is_none()
                }) {
                    return Err(EngineError::unreadable(
                        "The native history's machine is unavailable.",
                    ));
                }
                if !resources::harness(&mut conn, key.provider, key.agent, key.machine)
                    .await?
                    .is_some_and(|harness| harness.status == "online")
                {
                    return Err(EngineError::unreadable(
                        "The native history's home is unavailable.",
                    ));
                }
            }
        }
        let outgoing = Outgoing::new(
            command,
            key.machine,
            SerialKey::Harness {
                provider_id: key.provider,
                agent_id: key.agent,
            },
            &AgentHistoryManage {
                provider_id: key.provider,
                agent_id: key.agent,
                sources,
                remove,
            },
        )?;
        Ok(crate::commands::run(self.inner.commands.as_ref(), outgoing).await?)
    }

    async fn reconcile_agent_history_removals(&self) -> Result<(), EngineError> {
        let Some(through) = rows::claim_ceiling(&mut *self.db().acquire().await?).await? else {
            return Ok(());
        };
        let mut after = None;
        loop {
            let rows = rows::claims(&mut *self.db().acquire().await?, after, through, PAGE).await?;
            if rows.is_empty() {
                break;
            }
            after = rows.last().map(|row| row.id);
            for row in rows {
                if let Some(command) = row.claim {
                    self.finish_agent_history_removal(row, command, None)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn finish_agent_history_removal(
        &self,
        row: rows::History,
        command: Id,
        inventory_pass: Option<Id>,
    ) -> Result<(), EngineError> {
        let Some(machine) = row.machine_id else {
            return Ok(());
        };
        let inventory_pass = if let Some(body) = self.inner.commands.retained_body(command).await? {
            let retained: AgentHistoryManage = serde_json::from_value(body)?;
            if retained.provider_id != row.provider_id
                || retained.agent_id != row.agent_id
                || !retained.remove
                || retained.sources.len() != 1
                || retained.sources[0].source != source(&row)
                || retained.sources[0].started_by_orch != row.started_by_orch
            {
                return Err(EngineError::unreadable(
                    "The retained history removal command changed identity.",
                ));
            }
            retained.sources[0].inventory_pass
        } else {
            inventory_pass
        };
        let outcome = self
            .agent_history_command(
                HarnessKey {
                    machine,
                    provider: row.provider_id,
                    agent: row.agent_id,
                },
                vec![NativeHistorySource {
                    source: source(&row),
                    started_by_orch: row.started_by_orch,
                    inventory_pass,
                }],
                true,
                command,
                false,
            )
            .await?;
        let fact = match &outcome {
            Outcome::Succeeded(body) => {
                let result: AgentHistoryManageResult = serde_json::from_value(body.clone())?;
                if result.facts.len() != 1 || result.facts[0].source != source(&row) {
                    return Err(EngineError::unreadable(
                        "The native history result names another session.",
                    ));
                }
                result.facts.into_iter().next()
            }
            _ => None,
        };
        let error = match (&outcome, &fact) {
            (_, Some(fact)) => fact
                .error
                .as_deref()
                .or_else(|| (!fact.removed).then_some("history.removal_unconfirmed")),
            (Outcome::Failed(failure), _) => Some(failure.code.as_str()),
            _ => Some("history.removal_unconfirmed"),
        };
        let known = matches!(outcome, Outcome::Succeeded(_) | Outcome::Failed(_))
            || self.inner.commands.stopped(command).await?;
        let mut tx = self.db().begin().await?;
        threads::lock(&mut tx, row.thread_id).await?;
        if let Some(NativeHistoryFact {
            bytes,
            removed,
            error: fact_error,
            ..
        }) = &fact
        {
            rows::measured(
                &mut tx,
                row.id,
                &row.native_session_ref,
                *bytes,
                *removed,
                fact_error.as_deref(),
                self.inner.clock.now(),
            )
            .await?;
        }
        if known {
            rows::finish(&mut tx, row.id, command, error).await?;
        }
        let finished = {
            let mut unit = crate::unit::Unit::new(&mut tx, self.inner.clock.now());
            if row.removed_at.is_none()
                && fact
                    .as_ref()
                    .is_some_and(|fact| fact.removed && fact.error.is_none())
            {
                let current = rows::get(unit.conn, row.id).await?;
                let program = resources::agent(unit.conn, row.agent_id)
                    .await?
                    .map(|agent| {
                        crate::programs::shipped_name(&agent.harness_kind, Some(&agent.descriptor))
                            .name
                    })
                    .unwrap_or_else(|| "Agent".into());
                Self::agent_history_marker(
                    &mut unit,
                    row.thread_id,
                    row.id,
                    current.as_ref(),
                    row.id,
                    &program,
                    None,
                    &format!("history-removed:{command}"),
                )
                .await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        if fact
            .as_ref()
            .is_some_and(|fact| fact.removed && fact.error.is_none())
        {
            lock(&self.inner.sessions).closed(row.id);
        }
        if known {
            self.wake(row.thread_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn seven_idle_days_are_due_exactly_at_the_boundary_and_zero_never_removes() {
        let at = Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
        let due = at + chrono::Duration::days(7);
        assert!(!idle_due(at, due - chrono::Duration::milliseconds(1), 7));
        assert!(idle_due(at, due, 7));
        assert!(!idle_due(at, due, 0));
        assert!(!idle_due(at, due, i64::MAX));
    }
}
