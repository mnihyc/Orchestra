//! The leases of the credential kept here: one turn's leave to use it, the models it allows, what it has used, and which one a session's model calls run under.
//!
//! A **lease** is opened by the system for one turn (`lease.open`) and closed when the turn ends
//! (`lease.close`). Under it the credential may be used in one of three ways ([`LeaseMode`]): by the
//! forwarder, which adds the key to the model calls the harness makes beside this machine; by a copy issued
//! to the machine where the harness runs; or by the credential itself, handed over. A lease in the first mode
//! is the one the forwarder consults for every request: it is found by the provider and the thread of the
//! session that made the call, or, for a call that arrives through a tunnel, by the lease the ticket names.
//! A closed lease is not forgotten at once: its totals are still the answer to a `lease.close` that comes
//! again, and a request that runs under it is refused as one under a closed lease and not as one under none.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use orch_agent_contract::events::TokenUsage;
use orch_api_contract::usage::RequestTiming;
use orch_link_contract::commands::{
    JOURNAL_KEEP_HOURS, LeaseMode, LeaseOpen, LeaseUsageGroup, MachineKey,
};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};

use crate::credentials::SecretStore;
use crate::usage::{Allowance, AllowanceError, Totals, growth};

/// Why a lease could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    /// A lease in a mode that issues a credential names the form and the machine it goes to.
    #[error("a lease in mode {0:?} names the {1} of the credential it lets out")]
    Incomplete(LeaseMode, &'static str),
    /// The lease was opened and closed already; its id is not used again.
    #[error("the lease was closed already")]
    Closed,
    /// The allowance cannot be held.
    #[error("{0}")]
    Allowance(#[from] AllowanceError),
    /// The sealed local journal could not be restored or saved; no new grant is admitted.
    #[error("the lease journal is unavailable: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    open: bool,
    closed_at: Option<Time>,
    totals: Totals,
    last_issue: Option<Time>,
}

const RECORD_PREFIX: &str = "lease-";

#[derive(Serialize, Deserialize)]
struct SavedLease {
    version: u32,
    opened_at: Time,
    request: LeaseOpen,
    state: State,
}

/// One lease.
#[derive(Debug)]
pub struct Lease {
    lease_id: Id,
    provider_id: Id,
    thread_id: Id,
    mode: LeaseMode,
    protocol: Option<String>,
    form: Option<String>,
    recipient: Option<MachineKey>,
    allowed_models: Vec<String>,
    allowance: Allowance,
    state: Mutex<State>,
    request: LeaseOpen,
    opened_at: Time,
    store: Option<Arc<SecretStore>>,
}

impl Lease {
    /// The provider's ceiling accepted for this model when the turn opened its lease.
    #[must_use]
    pub fn model_parameters(
        &self,
        model: &str,
    ) -> Option<&orch_provider_contract::models::ModelParameters> {
        self.request.model_parameters.get(model)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn save(&self, state: &mut State) -> Result<(), LeaseError> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let saved = SavedLease {
            version: 1,
            opened_at: self.opened_at,
            request: self.request.clone(),
            state: state.clone(),
        };
        let result = serde_json::to_string(&saved)
            .map_err(|error| error.to_string())
            .and_then(|value| {
                store
                    .put(&format!("{RECORD_PREFIX}{}", self.lease_id), &value)
                    .map_err(|error| error.to_string())
            });
        result.map_err(|error| {
            state.open = false;
            LeaseError::Storage(error)
        })
    }

    fn save_count(&self, state: &mut State) {
        if let Err(error) = self.save(state) {
            tracing::warn!(lease = %self.lease_id, %error, "usage could not be retained; the lease is closed locally");
        }
    }

    /// Its identifier.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.lease_id
    }

    /// The provider whose credential it lets be used.
    #[must_use]
    pub const fn provider_id(&self) -> Id {
        self.provider_id
    }

    /// The thread whose turn it is for.
    #[must_use]
    pub const fn thread_id(&self) -> Id {
        self.thread_id
    }

    /// How the credential may be used under it.
    #[must_use]
    pub const fn mode(&self) -> LeaseMode {
        self.mode
    }

    /// The protocol of the calls that are forwarded under it, where the system said.
    #[must_use]
    pub fn protocol(&self) -> Option<&str> {
        self.protocol.as_deref()
    }

    /// The form of the credential it lets out.
    #[must_use]
    pub fn form(&self) -> Option<&str> {
        self.form.as_deref()
    }

    /// The only machine, and the key, a credential may be issued to under it.
    #[must_use]
    pub fn recipient(&self) -> Option<&MachineKey> {
        self.recipient.as_ref()
    }

    /// The models a request under it may name.
    #[must_use]
    pub fn allowed_models(&self) -> &[String] {
        &self.allowed_models
    }

    /// Whether a request may name `model`: the string is compared as it is written.
    #[must_use]
    pub fn allows(&self, model: &str) -> bool {
        self.allowed_models.iter().any(|allowed| allowed == model)
    }

    /// Whether it is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state().open
    }

    /// Whether what it has used reaches its allowance: the next request is refused.
    #[must_use]
    pub fn is_spent(&self) -> bool {
        self.allowance.reached(&self.state().totals)
    }

    /// Counts one request that ran.
    pub fn count_request(&self) {
        let mut state = self.state();
        state.totals.count_request();
        self.save_count(&mut state);
    }

    /// What `usage` of one request is charged in credits, where the lease holds a figure in credits.
    #[must_use]
    pub fn accrue(&self, usage: &TokenUsage) -> i128 {
        self.allowance.accrue(usage)
    }

    /// Native search fees at the existing allowance's rate.
    #[must_use]
    pub fn accrue_searches(&self, searches: i64) -> i128 {
        self.allowance.accrue_searches(searches)
    }

    /// Adds what a request used, in whole or in the part read since the last time, with its charge.
    pub fn add_usage(&self, usage: &TokenUsage, accrued: i128) {
        let mut state = self.state();
        state.totals.add_usage(usage, accrued);
        self.save_count(&mut state);
    }

    /// Counts one request that ran, and what it used where that was read.
    pub fn record(&self, usage: Option<&TokenUsage>) {
        self.count_request();
        if let Some(usage) = usage {
            self.add_usage(usage, self.accrue(usage));
        }
    }

    /// What the requests under it have used so far.
    #[must_use]
    pub fn totals(&self) -> Totals {
        self.state().totals.clone()
    }

    /// When a copy was last issued under it.
    #[must_use]
    pub fn last_issue(&self) -> Option<Time> {
        self.state().last_issue
    }

    /// Notes that a copy was issued now.
    pub fn issued_at(&self, now: Time) {
        if let Err(error) = self.issued_at_checked(now) {
            tracing::warn!(lease = %self.lease_id, %error, "a credential issue could not be retained");
        }
    }

    /// A credential is returned only once its issue time is durable.
    pub fn issued_at_checked(&self, now: Time) -> Result<(), LeaseError> {
        let mut state = self.state();
        state.last_issue = Some(now);
        self.save(&mut state)
    }

    fn close_checked(&self, now: Time) -> Result<Totals, LeaseError> {
        let mut state = self.state();
        if state.open {
            state.open = false;
            state.closed_at = Some(now);
        }
        self.save(&mut state)?;
        Ok(state.totals.clone())
    }
}

/// What one request has added to its lease so far. The usage of a stream is read as it passes, so the lease
/// is kept up to date piece by piece and the allowance holds while a long answer is still arriving.
#[derive(Debug)]
pub struct Meter {
    lease: Arc<Lease>,
    counted: TokenUsage,
    accrued: i128,
    /// Only this active request's current contribution, never completed request history.
    group: Option<LeaseUsageGroup>,
}

impl Meter {
    /// Counts a request under `lease` and begins to meter it.
    #[must_use]
    pub fn start(lease: Arc<Lease>) -> Self {
        lease.count_request();
        Self {
            lease,
            counted: TokenUsage::default(),
            accrued: 0,
            group: None,
        }
    }

    /// Starts an actual-wire-aware raw request before the upstream send or awaited key report.
    /// Old journal usage is deliberately left uncovered instead of inventing historical facts.
    #[must_use]
    pub fn start_observed(lease: Arc<Lease>) -> Self {
        {
            let mut state = lease.state();
            state.totals.count_request();
            state.totals.usage_groups.get_or_insert_with(Vec::new);
            lease.save_count(&mut state);
        }
        Self {
            lease,
            counted: TokenUsage::default(),
            accrued: 0,
            group: None,
        }
    }

    /// The lease it meters under.
    #[must_use]
    pub fn lease(&self) -> &Arc<Lease> {
        &self.lease
    }

    /// Takes in what the request has used so far (the figures are cumulative): the lease is given the part
    /// that is new.
    pub fn update(&mut self, usage: &TokenUsage) {
        self.update_inner(usage, None);
    }

    /// Adds cumulative usage and atomically moves this meter's existing contribution if its
    /// actual reported tier becomes known or changes, even when no token member grew.
    pub fn update_observed(&mut self, model: &str, usage: &TokenUsage, timing: &RequestTiming) {
        self.update_inner(usage, Some((model, timing)));
    }

    fn update_inner(&mut self, usage: &TokenUsage, facts: Option<(&str, &RequestTiming)>) {
        let new = growth(usage, &self.counted);
        let charge = self.lease.accrue(usage).saturating_add(
            facts
                .and_then(|(_, timing)| timing.web_searches.value().copied())
                .map_or(0, |searches| self.lease.accrue_searches(searches)),
        );
        let added = (charge - self.accrued).max(0);
        if facts.is_none() && new == TokenUsage::default() && added == 0 {
            return;
        }
        let counted = self.counted.plus(new);
        let group = facts.map(|(model, timing)| LeaseUsageGroup {
            search_usage: Some(orch_link_contract::commands::LeaseSearchUsage {
                searches: timing.web_searches.value().copied().unwrap_or(0).max(0),
                known_requests: i64::from(matches!(
                    timing.web_searches,
                    orch_api_contract::usage::UsageMeasurement::Told { .. }
                        | orch_api_contract::usage::UsageMeasurement::Unsupported
                )),
            }),
            model: model.to_owned(),
            usage: counted,
            requests: 1,
            min_context_tokens: counted.input_tokens,
            max_context_tokens: counted.input_tokens,
            timing: timing.clone(),
        });
        if new == TokenUsage::default() && added == 0 && group == self.group {
            return;
        }
        {
            let mut state = self.lease.state();
            state.totals.add_usage(&new, added);
            if let Some(group) = &group {
                state.totals.replace_group(self.group.as_ref(), group);
            }
            self.lease.save_count(&mut state);
        }
        self.counted = counted;
        self.accrued += added;
        self.group = group;
    }
}

#[derive(Debug, Default)]
struct Table {
    by_id: HashMap<Id, Arc<Lease>>,
    /// The open lease in mode `forwarded` of a provider's thread: the one its session's calls run under.
    current: HashMap<(Id, Id), Id>,
}

/// All the leases of this machine.
#[derive(Debug, Default)]
pub struct Leases {
    table: Mutex<Table>,
    store: Option<Arc<SecretStore>>,
    failure: Option<LeaseError>,
}

impl Leases {
    /// No lease.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores grants and usage from the existing sealed store. A corrupt journal refuses new grants.
    #[must_use]
    pub fn stored(store: Arc<SecretStore>, now: Time) -> Self {
        let mut leases = Self {
            store: Some(store),
            ..Self::default()
        };
        if let Err(error) = leases.restore(now) {
            leases.table = Mutex::new(Table::default());
            leases.failure = Some(error);
        }
        leases
    }

    fn restore(&mut self, now: Time) -> Result<(), LeaseError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| LeaseError::Storage("no store".to_owned()))?;
        let mut saved = Vec::new();
        for name in store
            .names()
            .map_err(|error| LeaseError::Storage(error.to_string()))?
        {
            let Some(id) = name.strip_prefix(RECORD_PREFIX) else {
                continue;
            };
            let id: Id = id
                .parse()
                .map_err(|_| LeaseError::Storage("invalid stored lease identity".to_owned()))?;
            let value = store
                .get(&name)
                .map_err(|error| LeaseError::Storage(error.to_string()))?
                .ok_or_else(|| LeaseError::Storage("a stored lease disappeared".to_owned()))?;
            let row: SavedLease = serde_json::from_str(&value)
                .map_err(|error| LeaseError::Storage(error.to_string()))?;
            if row.version != 1 || row.request.lease_id != id {
                return Err(LeaseError::Storage(
                    "stored lease identity or version differs".to_owned(),
                ));
            }
            if row.state.closed_at.is_some_and(|at| {
                now.get() - at.get() >= chrono::Duration::hours(JOURNAL_KEEP_HOURS)
            }) {
                store
                    .remove(&name)
                    .map_err(|error| LeaseError::Storage(error.to_string()))?;
            } else {
                saved.push(row);
            }
        }
        saved.sort_by_key(|row| (row.opened_at, row.request.lease_id));
        let mut table = self.table();
        for saved in saved {
            let body = &saved.request;
            let allowance = Allowance::from_open(body.allowance.as_ref())?;
            let lease = Arc::new(Lease {
                lease_id: body.lease_id,
                provider_id: body.provider_id,
                thread_id: body.thread_id,
                mode: body.mode,
                protocol: body.protocol.clone(),
                form: body.form.clone(),
                recipient: body.recipient.clone(),
                allowed_models: body.allowed_models.clone(),
                allowance,
                state: Mutex::new(saved.state),
                request: saved.request.clone(),
                opened_at: saved.opened_at,
                store: Some(Arc::clone(store)),
            });
            if lease.mode == LeaseMode::Forwarded {
                let key = (lease.provider_id, lease.thread_id);
                let previous = table.current.get(&key).and_then(|id| table.by_id.get(id));
                if lease.is_open() && previous.is_some_and(|before| before.is_open()) {
                    return Err(LeaseError::Storage(
                        "two open grants were stored for one provider's thread".to_owned(),
                    ));
                }
                if lease.is_open() || previous.is_none_or(|before| !before.is_open()) {
                    table.current.insert(key, lease.lease_id);
                }
            }
            table.by_id.insert(lease.lease_id, lease);
        }
        Ok(())
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens the lease a `lease.open` asks for. A lease that is open already is the same lease: asking
    /// again changes nothing.
    pub fn open(&self, body: &LeaseOpen, now: Time) -> Result<Arc<Lease>, LeaseError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if body.mode != LeaseMode::Forwarded {
            if body.form.is_none() {
                return Err(LeaseError::Incomplete(body.mode, "form"));
            }
            if body.recipient.is_none() {
                return Err(LeaseError::Incomplete(body.mode, "recipient"));
            }
        }
        let allowance = Allowance::from_open(body.allowance.as_ref())?;
        let mut table = self.table();
        if let Some(existing) = table.by_id.get(&body.lease_id) {
            return if existing.is_open() {
                Ok(Arc::clone(existing))
            } else {
                Err(LeaseError::Closed)
            };
        }
        let keep = chrono::Duration::hours(JOURNAL_KEEP_HOURS);
        table.by_id.retain(|_, lease| {
            lease
                .state()
                .closed_at
                .is_none_or(|closed| now.get().signed_duration_since(closed.get()) < keep)
        });
        if body.mode == LeaseMode::Forwarded
            && let Some(before) = table
                .current
                .get(&(body.provider_id, body.thread_id))
                .and_then(|id| table.by_id.get(id))
        {
            before.close_checked(now)?;
        }
        let lease = Arc::new(Lease {
            lease_id: body.lease_id,
            provider_id: body.provider_id,
            thread_id: body.thread_id,
            mode: body.mode,
            protocol: body.protocol.clone(),
            form: body.form.clone(),
            recipient: body.recipient.clone(),
            allowed_models: body.allowed_models.clone(),
            allowance,
            state: Mutex::new(State {
                open: true,
                ..State::default()
            }),
            request: body.clone(),
            opened_at: now,
            store: self.store.clone(),
        });
        lease.save(&mut lease.state())?;
        table.by_id.insert(body.lease_id, Arc::clone(&lease));
        if body.mode == LeaseMode::Forwarded {
            // A thread has one turn at a time: a lease opened for it ends the one before, which the system
            // did not close (it may have failed to), so that no lease is left open that nothing will close.
            table
                .current
                .insert((body.provider_id, body.thread_id), body.lease_id);
        }
        Ok(lease)
    }

    /// The lease with this identifier, open or closed.
    #[must_use]
    pub fn get(&self, lease_id: Id) -> Option<Arc<Lease>> {
        self.table().by_id.get(&lease_id).cloned()
    }

    /// The lease a session's calls run under: the open one in mode `forwarded` that was last opened for the
    /// thread on this provider; or, where none is open, the last one that was (closed), so that the refusal
    /// says that the lease is closed.
    #[must_use]
    pub fn of_session(&self, provider_id: Id, thread_id: Id) -> Option<Arc<Lease>> {
        let table = self.table();
        let id = table.current.get(&(provider_id, thread_id))?;
        table.by_id.get(id).cloned()
    }

    /// The lease a call that came through a tunnel runs under: the one its ticket names when that is open,
    /// otherwise the open one of the thread the ticket is for.
    #[must_use]
    pub fn of_ticket(&self, lease_id: Id, thread_id: Option<Id>) -> Option<Arc<Lease>> {
        let table = self.table();
        let named = table.by_id.get(&lease_id).cloned();
        if let Some(lease) = &named
            && lease.is_open()
            && thread_id.is_none_or(|thread| thread == lease.thread_id)
        {
            return named;
        }
        let thread = thread_id.or_else(|| named.as_ref().map(|lease| lease.thread_id));
        let of_thread = thread.and_then(|thread| {
            table
                .current
                .iter()
                .filter(|((_, of), _)| *of == thread)
                .filter_map(|(_, id)| table.by_id.get(id))
                .find(|lease| lease.is_open())
                .cloned()
        });
        of_thread.or(named)
    }

    /// Closes a lease; what it used is returned. A lease that is not known here (it was never opened, or
    /// the machine started again since) has used nothing that this machine knows of.
    pub fn close(&self, lease_id: Id, now: Time) -> Totals {
        self.close_checked(lease_id, now).unwrap_or_default()
    }

    /// A command acknowledges closure only after its closed state and totals are durable.
    pub fn close_checked(&self, lease_id: Id, now: Time) -> Result<Totals, LeaseError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let lease = self.table().by_id.get(&lease_id).cloned();
        // The session's calls go on finding the lease, closed, and are refused as calls under a closed one.
        lease.map_or_else(|| Ok(Totals::default()), |lease| lease.close_checked(now))
    }

    /// Closes every lease of a provider that is forgotten.
    pub fn close_all_of(&self, provider_id: Id, now: Time) {
        if let Err(error) = self.close_all_checked(provider_id, now) {
            tracing::warn!(%provider_id, %error, "the provider's leases could not be closed durably");
        }
    }

    /// Closes every grant before a provider stop is acknowledged.
    pub fn close_all_checked(&self, provider_id: Id, now: Time) -> Result<(), LeaseError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let leases: Vec<Arc<Lease>> = self
            .table()
            .by_id
            .values()
            .filter(|lease| lease.provider_id == provider_id)
            .cloned()
            .collect();
        for lease in leases {
            lease.close_checked(now)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::events::TokensReportedApart;
    use orch_link_contract::commands::LeaseOpenAllowance;
    use orch_types::new_id;
    use serde_json::json;

    fn now() -> Time {
        "2026-10-11T12:00:00.000Z".parse().unwrap()
    }

    fn forwarded(provider: Id, thread: Id) -> LeaseOpen {
        serde_json::from_value(json!({
            "lease_id": new_id(), "provider_id": provider, "thread_id": thread, "mode": "forwarded",
            "protocol": "openai-responses", "allowed_models": ["gpt-6-sol"]
        }))
        .unwrap()
    }

    fn used(input: i64, output: i64) -> TokenUsage {
        TokenUsage::from_parts(TokensReportedApart {
            input,
            cache_read: 0,
            cache_write: 0,
            output,
            reasoning: 0,
        })
    }

    fn raw_timing(reported: Option<&str>) -> RequestTiming {
        use orch_api_contract::usage::UsageMeasurement;
        RequestTiming {
            wire_protocol: Some("openai-responses".to_owned()),
            requested_service_tier: Some("fast".to_owned()),
            reported_service_tier: reported.map_or(UsageMeasurement::Unread, |value| {
                UsageMeasurement::Told {
                    value: value.to_owned(),
                }
            }),
            ..Default::default()
        }
    }

    #[test]
    fn active_meter_refinement_moves_only_its_prior_contribution_without_recounting_tokens() {
        let leases = Leases::new();
        let lease = leases.open(&forwarded(new_id(), new_id()), now()).unwrap();
        let mut first = Meter::start_observed(Arc::clone(&lease));
        let mut second = Meter::start_observed(Arc::clone(&lease));
        first.update_observed("gpt-6-sol", &used(100, 10), &raw_timing(None));
        second.update_observed("gpt-6-sol", &used(200, 20), &raw_timing(None));
        first.update_observed("gpt-6-sol", &used(100, 10), &raw_timing(Some("default")));
        // Repeating the cumulative report or refining the tier without growth cannot add tokens.
        first.update_observed("gpt-6-sol", &used(100, 10), &raw_timing(Some("default")));
        second.update_observed("gpt-6-sol", &used(250, 25), &raw_timing(Some("priority")));
        let closed = leases.close_checked(lease.id(), now()).unwrap();
        assert_eq!(closed.requests, 2);
        assert_eq!(closed.usage, used(350, 35));
        let groups = closed.usage_groups.as_ref().unwrap();
        assert_eq!(
            groups.len(),
            2,
            "the former unread group has no contribution left"
        );
        for (reported, tokens) in [("default", used(100, 10)), ("priority", used(250, 25))] {
            let group = groups
                .iter()
                .find(|group| {
                    group
                        .timing
                        .reported_service_tier
                        .value()
                        .map(String::as_str)
                        == Some(reported)
                })
                .unwrap();
            assert_eq!(group.requests, 1);
            assert_eq!(group.usage, tokens);
            assert_eq!(group.min_context_tokens, tokens.input_tokens);
            assert_eq!(group.max_context_tokens, tokens.input_tokens);
            assert!(
                group.timing.is_unsupported(),
                "raw facts do not invent performance capability"
            );
        }
        assert_eq!(leases.close_checked(lease.id(), now()).unwrap(), closed);
    }

    #[test]
    fn awareness_does_not_assign_new_facts_to_restored_legacy_usage() {
        let leases = Leases::new();
        let lease = leases.open(&forwarded(new_id(), new_id()), now()).unwrap();
        let old: Totals = serde_json::from_value(json!({
            "requests": 1, "usage": used(100, 10), "known": true, "accrued": 0
        }))
        .unwrap();
        assert!(old.usage_groups.is_none());
        lease.state().totals = old;
        let mut meter = Meter::start_observed(Arc::clone(&lease));
        let aware = lease.totals();
        assert_eq!(aware.usage, used(100, 10));
        assert_eq!(aware.usage_groups, Some(Vec::new()));
        meter.update_observed("gpt-6-sol", &used(20, 2), &raw_timing(Some("default")));
        let totals = lease.totals();
        assert_eq!(totals.usage, used(120, 12));
        assert_eq!(totals.usage_groups.unwrap()[0].usage, used(20, 2));
    }

    #[test]
    fn aware_request_without_a_usage_report_retains_unknown_usage_in_a_close() {
        let leases = Leases::new();
        let lease = leases.open(&forwarded(new_id(), new_id()), now()).unwrap();
        let _meter = Meter::start_observed(Arc::clone(&lease));
        let closed = leases.close_checked(lease.id(), now()).unwrap();
        assert_eq!(closed.requests, 1);
        assert_eq!(closed.reported(), None);
        assert_eq!(closed.usage_groups, Some(Vec::new()));
    }

    #[test]
    fn grouped_actual_facts_survive_the_existing_lease_journal_restart() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(crate::roles::Keys::new(
            orch_pki::certificates::LeafKey::generate().unwrap(),
        ));
        let store = Arc::new(SecretStore::open(dir.path(), keys).unwrap());
        let leases = Leases::stored(Arc::clone(&store), now());
        let request = forwarded(new_id(), new_id());
        let lease = leases.open(&request, now()).unwrap();
        let mut meter = Meter::start_observed(Arc::clone(&lease));
        meter.update_observed("gpt-6-sol", &used(100, 10), &raw_timing(None));
        meter.update_observed("gpt-6-sol", &used(100, 10), &raw_timing(Some("priority")));
        let before = lease.totals();
        drop(meter);
        drop(lease);
        drop(leases);
        let restored = Leases::stored(store, now());
        let after = restored.close_checked(request.lease_id, now()).unwrap();
        assert_eq!(after, before);
        assert_eq!(
            after.usage_groups.unwrap()[0].timing,
            raw_timing(Some("priority"))
        );
    }

    #[test]
    fn a_session_finds_the_lease_that_was_opened_last_for_its_thread() {
        let leases = Leases::new();
        let (provider, thread) = (new_id(), new_id());
        let first = leases.open(&forwarded(provider, thread), now()).unwrap();
        let second = leases.open(&forwarded(provider, thread), now()).unwrap();
        let found = leases.of_session(provider, thread).unwrap();
        assert_eq!(found.id(), second.id());
        assert_ne!(found.id(), first.id());
        assert!(leases.of_session(provider, new_id()).is_none());
        assert!(leases.of_session(new_id(), thread).is_none());
    }

    #[test]
    fn a_lease_opened_for_a_thread_ends_the_one_the_system_left_open_before_it() {
        let leases = Leases::new();
        let (provider, thread) = (new_id(), new_id());
        let first = leases.open(&forwarded(provider, thread), now()).unwrap();
        let second = leases.open(&forwarded(provider, thread), now()).unwrap();
        assert!(!first.is_open());
        assert!(second.is_open());
        let other = leases.open(&forwarded(provider, new_id()), now()).unwrap();
        assert!(other.is_open(), "the lease of another thread is not ended");
    }

    #[test]
    fn a_lease_allows_a_model_by_its_name_as_written_and_no_other() {
        let leases = Leases::new();
        let lease = leases.open(&forwarded(new_id(), new_id()), now()).unwrap();
        assert!(lease.allows("gpt-6-sol"));
        assert!(!lease.allows("GPT-6-SOL"));
        assert!(!lease.allows("gpt-6-sol:free"));
        assert!(!lease.allows(""));
    }

    #[test]
    fn closing_a_lease_answers_what_it_used_and_answers_the_same_again() {
        let leases = Leases::new();
        let lease = leases.open(&forwarded(new_id(), new_id()), now()).unwrap();
        lease.record(Some(&used(10, 5)));
        lease.record(None);
        let first = leases.close(lease.id(), now());
        assert_eq!(first.requests, 2);
        assert_eq!(first.reported().map(|usage| usage.total_tokens), Some(15));
        assert!(!lease.is_open());
        assert_eq!(leases.close(lease.id(), now()), first);
        assert_eq!(leases.close(new_id(), now()), Totals::default());
    }

    #[test]
    fn a_closed_lease_stays_the_one_of_its_session_so_the_refusal_can_say_so() {
        let leases = Leases::new();
        let (provider, thread) = (new_id(), new_id());
        let lease = leases.open(&forwarded(provider, thread), now()).unwrap();
        leases.close(lease.id(), now());
        let found = leases.of_session(provider, thread).unwrap();
        assert!(!found.is_open());
        assert!(matches!(
            leases.open(
                &LeaseOpen {
                    model_parameters: Default::default(),
                    lease_id: lease.id(),
                    ..forwarded(provider, thread)
                },
                now()
            ),
            Err(LeaseError::Closed)
        ));
    }

    #[test]
    fn a_ticket_that_names_a_closed_lease_runs_under_the_open_one_of_its_thread() {
        let leases = Leases::new();
        let (provider, thread) = (new_id(), new_id());
        let old = leases.open(&forwarded(provider, thread), now()).unwrap();
        leases.close(old.id(), now());
        let new = leases.open(&forwarded(provider, thread), now()).unwrap();
        assert_eq!(leases.of_ticket(old.id(), None).unwrap().id(), new.id());
        assert_eq!(
            leases.of_ticket(new_id(), Some(thread)).unwrap().id(),
            new.id()
        );
        leases.close(new.id(), now());
        assert!(!leases.of_ticket(old.id(), Some(thread)).unwrap().is_open());
        assert!(leases.of_ticket(new_id(), Some(new_id())).is_none());
    }

    #[test]
    fn a_lease_that_lets_a_credential_out_names_its_form_and_its_recipient() {
        let leases = Leases::new();
        let body: LeaseOpen = serde_json::from_value(json!({
            "lease_id": new_id(), "provider_id": new_id(), "thread_id": new_id(), "mode": "shared",
            "allowed_models": []
        }))
        .unwrap();
        assert!(matches!(
            leases.open(&body, now()),
            Err(LeaseError::Incomplete(LeaseMode::Shared, "form"))
        ));
        let with_form = LeaseOpen {
            model_parameters: Default::default(),
            form: Some("access_token".into()),
            ..body.clone()
        };
        assert!(matches!(
            leases.open(&with_form, now()),
            Err(LeaseError::Incomplete(LeaseMode::Shared, "recipient"))
        ));
    }

    #[test]
    fn a_lease_with_an_allowance_is_spent_when_the_figure_is_reached() {
        let leases = Leases::new();
        let body = LeaseOpen {
            model_parameters: Default::default(),
            allowance: Some(LeaseOpenAllowance {
                tokens: Some(20),
                credits: None,
            }),
            ..forwarded(new_id(), new_id())
        };
        let lease = leases.open(&body, now()).unwrap();
        assert!(!lease.is_spent());
        lease.record(Some(&used(10, 9)));
        assert!(!lease.is_spent());
        lease.record(Some(&used(1, 0)));
        assert!(lease.is_spent());
    }

    #[test]
    fn closing_every_lease_of_a_provider_leaves_the_leases_of_others_open() {
        let leases = Leases::new();
        let (mine, other) = (new_id(), new_id());
        let a = leases.open(&forwarded(mine, new_id()), now()).unwrap();
        let b = leases.open(&forwarded(other, new_id()), now()).unwrap();
        leases.close_all_of(mine, now());
        assert!(!a.is_open());
        assert!(b.is_open());
    }

    #[test]
    fn restart_restores_forwarded_and_shared_grants_usage_allowance_and_closed_identity() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(crate::roles::Keys::new(
            orch_pki::certificates::LeafKey::generate().unwrap(),
        ));
        let store = Arc::new(SecretStore::open(dir.path(), keys).unwrap());
        let leases = Leases::stored(Arc::clone(&store), now());
        let request = LeaseOpen {
            model_parameters: Default::default(),
            allowance: Some(LeaseOpenAllowance {
                tokens: Some(20),
                credits: None,
            }),
            ..forwarded(new_id(), new_id())
        };
        let lease = leases.open(&request, now()).unwrap();
        lease.record(Some(&used(10, 6)));
        let shared = LeaseOpen {
            model_parameters: Default::default(),
            mode: LeaseMode::Shared,
            form: Some("copy".to_owned()),
            recipient: Some(MachineKey {
                machine_id: new_id(),
                public_key: "recipient-key".to_owned(),
            }),
            ..forwarded(new_id(), new_id())
        };
        leases
            .open(&shared, now())
            .unwrap()
            .issued_at_checked(now())
            .unwrap();
        drop(lease);
        drop(leases);
        let restored = Leases::stored(Arc::clone(&store), now());
        let lease = restored.open(&request, now()).unwrap();
        assert_eq!(
            restored
                .of_session(request.provider_id, request.thread_id)
                .unwrap()
                .id(),
            request.lease_id
        );
        assert_eq!(lease.totals().requests, 1);
        assert_eq!(lease.totals().usage.total_tokens, 16);
        assert!(!lease.is_spent());
        lease.record(Some(&used(4, 0)));
        assert!(lease.is_spent());
        let copied = restored.get(shared.lease_id).unwrap();
        assert_eq!(copied.recipient(), shared.recipient.as_ref());
        assert_eq!(copied.last_issue(), Some(now()));
        let totals = restored.close_checked(request.lease_id, now()).unwrap();
        drop(restored);
        let again = Leases::stored(store, now());
        assert!(!again.get(request.lease_id).unwrap().is_open());
        assert_eq!(
            again.close_checked(request.lease_id, now()).unwrap(),
            totals
        );
        assert!(matches!(
            again.open(&request, now()),
            Err(LeaseError::Closed)
        ));
    }

    #[test]
    fn a_corrupt_stored_grant_is_never_replaced_with_an_empty_open_lease_table() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(crate::roles::Keys::new(
            orch_pki::certificates::LeafKey::generate().unwrap(),
        ));
        let store = Arc::new(SecretStore::open(dir.path(), keys).unwrap());
        let request = forwarded(new_id(), new_id());
        store
            .put(
                &format!("{RECORD_PREFIX}{}", request.lease_id),
                "incomplete journal",
            )
            .unwrap();
        let restored = Leases::stored(store, now());
        assert!(matches!(
            restored.open(&request, now()),
            Err(LeaseError::Storage(_))
        ));
        assert!(
            restored
                .of_session(request.provider_id, request.thread_id)
                .is_none()
        );
    }
    #[test]
    fn search_reports_replace_one_active_contribution_without_double_counting() {
        use orch_api_contract::usage::UsageMeasurement;
        let leases = Leases::new();
        let lease = leases.open(&forwarded(new_id(), new_id()), now()).unwrap();
        let mut one = Meter::start_observed(Arc::clone(&lease));
        let mut timing = raw_timing(Some("default"));
        timing.web_searches = UsageMeasurement::Told { value: 2 };
        one.update_observed("gpt-6-sol", &used(10, 2), &timing);
        one.update_observed("gpt-6-sol", &used(10, 2), &timing);
        let mut two = Meter::start_observed(Arc::clone(&lease));
        timing.web_searches = UsageMeasurement::Unread;
        two.update_observed("gpt-6-sol", &used(5, 1), &timing);
        let groups = lease.totals().usage_groups.unwrap();
        assert_eq!(groups[0].search_usage.as_ref().unwrap().searches, 2);
        assert_eq!(groups[0].search_usage.as_ref().unwrap().known_requests, 1);
        timing.web_searches = UsageMeasurement::Told { value: 1 };
        two.update_observed("gpt-6-sol", &used(5, 1), &timing);
        let groups = lease.totals().usage_groups.unwrap();
        assert_eq!(groups[0].search_usage.as_ref().unwrap().searches, 3);
        assert_eq!(groups[0].search_usage.as_ref().unwrap().known_requests, 2);
    }
}
