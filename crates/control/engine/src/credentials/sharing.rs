//! Issue and renew shared copies while provider custody remains.
//!
//! Copies of a credential that are issued for a thread's harness: asked of the provider's runtime under a lease, given to the harness sealed, replaced while the turn runs, and written down each time.
//!
//! For a session on the path `shared` the lease is opened where the credential is kept, with the form
//! and the one machine a copy may go to; the provider's runtime issues the copy, sealed to that machine;
//! the engine passes the sealed form on to the harness and keeps none of it. A copy that lapses is
//! replaced before it does (`credential.refresh_margin_secs` ahead) and whenever the harness says it
//! needs another, but never while an issue is in flight and never within thirty seconds of the last: a
//! harness that asks more often than that fails its turn as `provider_unauthorized`.

use super::Given;
use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::harnesses::HarnessKey;
use crate::ids;
use crate::leases;
use crate::providers::classify_keeper;
use crate::sessions::StartSnapshot;
use crate::turns::{InputAfter, StartFailure, TurnEnd, classify, error_json};
use crate::unit::Unit;
use orch_access_contract::audit::AuditAction;
use orch_agent_contract::credentials::needs_renewal;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::resources::provider_runtime as rows;
use orch_db::queries::resources::thread_engine::ProviderRow;
use orch_db::queries::threads::{self, LeaseRow, turn_state};
use orch_link_contract::commands::{
    AgentCredentialSet, AgentCredentialSetUpstream, LeaseMode, LeaseOpen, MachineKey,
    ProviderCredentialIssue, SealedCredential, SerialKey,
};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde_json::json;

/// How many leases one look at the copies that are about to lapse takes up.
const RENEW_BATCH: i64 = 200;
/// The words of a turn that was ended because its harness asked for a credential too often.
const TOO_OFTEN: &str =
    "the harness asked for a credential again within thirty seconds of the last one it was given";

/// What an attempt to issue a copy came to.
#[derive(Debug)]
pub(crate) enum Issue {
    /// The harness holds a fresh copy, which lapses at the time given, if it does.
    Given(Option<Time>),
    /// An issue is in flight, or the last was less than thirty seconds ago: nothing was done.
    NotNow,
    /// It could not be done, in the words a failed start carries; nothing left the provider.
    Failed(Box<StartFailure>),
    /// The provider's runtime issued the credential and the harness could not take it: what was issued
    /// is out of the provider's hands.
    Lost(Box<StartFailure>),
}

/// The endpoint a harness is to call with a key of an API provider: the upstream that speaks the protocol
/// of the arrangement, else the first. None for a provider whose harness knows its own endpoint.
fn upstream_of(upstreams: &Json, protocol: Option<&str>) -> Option<AgentCredentialSetUpstream> {
    let upstreams = upstreams.as_array()?;
    let chosen = protocol
        .and_then(|wanted| {
            upstreams
                .iter()
                .find(|upstream| upstream.get("protocol").and_then(Json::as_str) == Some(wanted))
        })
        .or_else(|| upstreams.first())?;
    Some(AgentCredentialSetUpstream {
        protocol: chosen.get("protocol")?.as_str()?.to_owned(),
        base_url: chosen.get("base_url")?.as_str()?.to_owned(),
    })
}

impl Engine {
    /// Opens a lease that lets a credential leave, at the machine that keeps it: the mode, the form and
    /// the one machine a copy may be issued to are the lease's, and the machine holds it until it is closed.
    pub(crate) async fn open_credential_lease_at_provider(
        &self,
        lease: &LeaseRow,
        provider: &ProviderRow,
        recipient: &MachineKey,
    ) -> Result<Outcome, EngineError> {
        let machine = leases::machine_of(provider).ok_or_else(|| {
            EngineError::unreadable("the provider has no machine to keep a lease on")
        })?;
        let mode = if lease.mode == "handover" {
            LeaseMode::Handover
        } else {
            LeaseMode::Shared
        };
        let body = LeaseOpen {
            lease_id: lease.id,
            provider_id: lease.provider_id,
            thread_id: lease.thread_id,
            mode,
            protocol: None,
            form: lease.form.clone(),
            recipient: Some(recipient.clone()),
            allowed_models: lease.allowed_models.clone(),
            allowance: None,
        };
        let command = Outgoing::new(
            ids::lease_open(lease.id, 0),
            machine,
            SerialKey::Provider(lease.provider_id),
            &body,
        )?;
        Ok(crate::commands::run(self.inner.commands.as_ref(), command).await?)
    }

    /// Gives the harness of a starting turn a copy of the provider's credential, under a lease of the
    /// mode `shared` that is open by now: the lease is opened where the credential is kept, a copy is
    /// issued, and the harness is given it.
    pub(crate) async fn share_credential(
        &self,
        snap: &StartSnapshot,
        lease: &LeaseRow,
        form: &str,
    ) -> Result<Given, EngineError> {
        let recipient = self.machine_key(snap.key.machine).await?;
        let opened = self
            .open_credential_lease_at_provider(lease, &snap.provider, &recipient)
            .await?;
        if !matches!(opened, Outcome::Succeeded(_)) {
            return Ok(Given::Failed(Box::new(classify_keeper(
                "lease.open",
                &opened,
            ))));
        }
        match self
            .issue_copy(
                lease,
                &snap.provider,
                snap.key,
                form,
                snap.arrangement.protocol.as_deref(),
            )
            .await?
        {
            Issue::Given(_) | Issue::NotNow => Ok(Given::Ready),
            Issue::Failed(failure) | Issue::Lost(failure) => Ok(Given::Failed(failure)),
        }
    }

    /// Issues a copy of the form under the lease and gives it to the harness: the provider's runtime
    /// issues it sealed to the machine of the harness, the engine passes the sealed form on in
    /// `agent.credential.set`, and the issue is written down. Not done while an issue is in flight or the
    /// last was less than thirty seconds ago.
    pub(crate) async fn issue_copy(
        &self,
        lease: &LeaseRow,
        provider: &ProviderRow,
        key: HarnessKey,
        form: &str,
        protocol: Option<&str>,
    ) -> Result<Issue, EngineError> {
        self.restore_copy_state(lease.id).await?;
        let now = Time::new(self.inner.clock.now());
        let Some(number) = lock(&self.inner.credentials).begin(lease.id, now) else {
            return Ok(Issue::NotNow);
        };
        let done = self
            .issue_and_set(lease, provider, key, form, protocol, number)
            .await;
        match &done {
            Ok(Issue::Given(expires_at)) => {
                lock(&self.inner.credentials).done(
                    lease.id,
                    *expires_at,
                    Time::new(self.inner.clock.now()),
                );
            }
            _ => lock(&self.inner.credentials).failed(lease.id),
        }
        done
    }

    async fn issue_and_set(
        &self,
        lease: &LeaseRow,
        provider: &ProviderRow,
        key: HarnessKey,
        form: &str,
        protocol: Option<&str>,
        number: u32,
    ) -> Result<Issue, EngineError> {
        let keeper = leases::machine_of(provider).ok_or_else(|| {
            EngineError::unreadable("the provider has no machine to issue a credential from")
        })?;
        let upstream = if provider.kind == "api" {
            let mut conn = self.inner.db.acquire().await?;
            let upstreams = rows::upstreams_of(&mut conn, provider.id).await?;
            upstream_of(&upstreams, protocol)
        } else {
            None
        };
        let recipient = self.machine_key(key.machine).await?;
        let custodian = self.machine_key(keeper).await?;
        let issue = Outgoing::new(
            ids::derived(
                "provider.credential.issue",
                &[lease.id.as_bytes().as_slice(), &number.to_le_bytes()],
            ),
            keeper,
            SerialKey::Provider(provider.id),
            &ProviderCredentialIssue {
                lease_id: lease.id,
                provider_id: provider.id,
                form: form.to_owned(),
                recipient: recipient.clone(),
            },
        )?;
        let sealed: SealedCredential =
            match crate::commands::run(self.inner.commands.as_ref(), issue).await? {
                Outcome::Succeeded(result) => serde_json::from_value(result)?,
                other => {
                    return Ok(Issue::Failed(Box::new(classify_keeper(
                        "provider.credential.issue",
                        &other,
                    ))));
                }
            };
        let expires_at = sealed.expires_at;
        let set = Outgoing::new(
            ids::derived(
                "agent.credential.set",
                &[lease.id.as_bytes().as_slice(), &number.to_le_bytes()],
            ),
            key.machine,
            SerialKey::Harness {
                provider_id: provider.id,
                agent_id: key.agent,
            },
            &AgentCredentialSet {
                provider_id: provider.id,
                agent_id: key.agent,
                form: sealed.form,
                credential: sealed.credential,
                expires_at,
                upstream,
                custodian,
            },
        )?;
        match crate::commands::run(self.inner.commands.as_ref(), set).await? {
            Outcome::Succeeded(_) => {}
            other => return Ok(Issue::Lost(Box::new(classify(&other)))),
        }
        self.record_issue(lease, provider.id, key.machine, form, expires_at)
            .await?;
        Ok(Issue::Given(expires_at))
    }

    /// Writes down that a copy was issued: for which lease and thread, in which form, to which machine,
    /// and when it lapses. What was issued is not in it.
    pub(crate) async fn record_issue(
        &self,
        lease: &LeaseRow,
        provider: Id,
        machine: Id,
        form: &str,
        expires_at: Option<Time>,
    ) -> Result<(), EngineError> {
        let mut entry = Entry::new(
            Actor::System,
            orch_db::audit::action!(AuditAction::ProviderCredentialIssued).as_str(),
            "provider",
            Some(provider),
            "issued a credential for a thread's harness",
        );
        entry.history = AuditAction::ProviderCredentialIssued.is_history_line();
        entry.node_id = Some(lease.thread_id);
        entry.details = json!({
            "lease": lease.id,
            "thread": lease.thread_id,
            "mode": lease.mode,
            "form": form,
            "machine": machine,
            "expires_at": expires_at,
        });
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        rows::record_shared_copy_expiry(&mut tx, lease.id, expires_at.map(Time::get), now).await?;
        audit::append(&mut tx, &entry, &Origin::default(), now).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Rehydrates expiry, the minimum issue interval and the next command identity from durable state.
    async fn restore_copy_state(&self, lease: Id) -> Result<(), EngineError> {
        let held = lock(&self.inner.credentials).of(lease);
        if held.last.is_some() || held.in_flight {
            return Ok(());
        }
        let state = {
            let mut conn = self.inner.db.acquire().await?;
            rows::shared_copy_state(&mut conn, lease).await?
        };
        if let Some(state) = state {
            lock(&self.inner.credentials).restore(
                lease,
                state.expires_at.map(Time::new),
                state.issued_at.map(Time::new),
                u32::try_from(state.issues).unwrap_or(u32::MAX),
            );
        }
        Ok(())
    }

    /// Replaces the copies that are about to lapse, and those that their harness says it lacks, for the
    /// turns that run now. Looked at on every sweep; what is due is done apart from the sweep, since a
    /// provider's runtime may take a while to answer.
    pub(crate) async fn renew_shared_copies(&self) -> Result<(), EngineError> {
        let open = {
            let mut conn = self.inner.db.acquire().await?;
            rows::open_shared_leases(&mut conn, RENEW_BATCH).await?
        };
        lock(&self.inner.credentials)
            .retain(&open.iter().map(|lease| lease.id).collect::<Vec<_>>());
        if open.is_empty() {
            return Ok(());
        }
        let margin = {
            let mut conn = self.inner.db.acquire().await?;
            setting::<i64>(
                &mut conn,
                SettingsPlace::System,
                keys::CREDENTIAL_REFRESH_MARGIN_SECS,
            )
            .await?
            .unwrap_or(0)
        };
        let now = Time::new(self.inner.clock.now());
        for lease in open {
            self.restore_copy_state(lease.id).await?;
            let held = lock(&self.inner.credentials).of(lease.id);
            if held.in_flight {
                continue;
            }
            let wanted = held.needed.is_some() || needs_renewal(held.expires_at, now, margin);
            if !wanted {
                continue;
            }
            let engine = self.clone();
            tokio::spawn(async move {
                if let Err(error) = engine.renew_one(&lease).await {
                    tracing::warn!(%error, lease = %lease.id, "a copy of a credential could not be renewed");
                }
            });
        }
        Ok(())
    }

    /// Renews the copy a lease's harness holds.
    async fn renew_one(&self, lease: &LeaseRow) -> Result<(), EngineError> {
        let (provider, thread, recipient, form) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                orch_db::queries::resources::thread_engine::provider(&mut conn, lease.provider_id)
                    .await?,
                threads::thread(&mut conn, lease.thread_id).await?,
                rows::lease_recipient(&mut conn, lease.id).await?,
                lease.form.clone(),
            )
        };
        let (Some(provider), Some(thread), Some(machine), Some(form)) =
            (provider, thread, recipient, form)
        else {
            return Ok(());
        };
        let Some(agent) = thread.agent_id else {
            return Ok(());
        };
        let key = HarnessKey {
            provider: provider.id,
            agent,
            machine,
        };
        match self.issue_copy(lease, &provider, key, &form, None).await? {
            Issue::Given(_) => Ok(()),
            Issue::NotNow => {
                // A harness that asks again within thirty seconds of the last copy is refused: its turn
                // fails, so that it does not hold the provider's runtime in a loop.
                let held = lock(&self.inner.credentials).of(lease.id);
                if held.needed.is_some() && !held.in_flight {
                    self.fail_turn_unauthorized(lease, TOO_OFTEN).await?;
                }
                Ok(())
            }
            Issue::Failed(failure) | Issue::Lost(failure) => {
                tracing::warn!(lease = %lease.id, why = %failure.message, "a credential could not be issued for a running turn");
                self.fail_turn_unauthorized(lease, &failure.message).await
            }
        }
    }

    /// Ends the turn of a lease as failed with `provider_unauthorized`: the credential its harness needs
    /// cannot be given. The harness is asked to stop.
    async fn fail_turn_unauthorized(&self, lease: &LeaseRow, why: &str) -> Result<(), EngineError> {
        let message = format!("the harness has no credential to go on with: {why}");
        let Some(turn_id) = lease.turn_id else {
            return Ok(());
        };
        let mut tx = self.inner.db.begin().await?;
        let (finished, machine) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, lease.thread_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(&mut *unit.conn, turn_id).await? else {
                return Ok(());
            };
            if !matches!(
                turn.state.as_str(),
                turn_state::STARTING | turn_state::RUNNING
            ) {
                return Ok(());
            }
            let machine = rows::lease_recipient(&mut *unit.conn, lease.id).await?;
            crate::ingest::add_error_item(
                &mut unit,
                thread.node_id,
                turn.segment_id,
                Some(turn.id),
                "provider_unauthorized",
                &message,
                false,
            )
            .await?;
            self.end_turn(
                &mut unit,
                &thread,
                &turn,
                TurnEnd {
                    state: turn_state::FAILED,
                    error: Some(error_json("provider_unauthorized", &message, false)),
                    input: InputAfter::Retry { retryable: false },
                    then_block: Some(crate::preflight::Blocked {
                        reason: "provider_unavailable",
                        detail: Some(why.to_owned()),
                    }),
                },
            )
            .await?;
            (unit.finish().await?, machine)
        };
        self.commit(tx, finished).await?;
        if let Some(machine) = machine {
            self.send_interrupt_best_effort(machine, lease.thread_id, turn_id)
                .await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_key_of_an_api_provider_is_given_with_the_endpoint_that_speaks_the_protocol_of_the_arrangement()
     {
        let upstreams = json!([
            { "protocol": "openai-chat", "base_url": "https://chat.example/v1", "options": {} },
            { "protocol": "openai-responses", "base_url": "https://responses.example/v1", "options": {} }
        ]);
        let chosen = upstream_of(&upstreams, Some("openai-responses")).unwrap();
        assert_eq!(chosen.base_url, "https://responses.example/v1");
        let first = upstream_of(&upstreams, Some("anthropic-messages")).unwrap();
        assert_eq!(first.protocol, "openai-chat", "else the first endpoint");
    }

    #[test]
    fn a_provider_without_endpoints_points_the_harness_nowhere() {
        assert_eq!(upstream_of(&json!([]), Some("openai-responses")), None);
        assert_eq!(upstream_of(&Json::Null, None), None);
    }
}
