//! Per-turn provider authorization, forwarder allowance and closure.
//!
//! A turn that reaches its model through the forwarder runs under a lease:
//! one turn's leave, for the models it names, to use the provider's key
//! where the key is kept. The lease is a row opened in the transaction that
//! starts the turn, sent to the provider's side before the turn is, and
//! closed in the transaction that ends it, with `lease.close` sent after.
//! A turn on the provider's own login (the path `native`) has none.
//!
//! The allowance a lease carries is the least room among the caps that
//! cover the turn and would stop it (`at_limit` is `stop`) and that it was not
//! let past, in tokens and in credits at the rate the turn is counted at. The
//! caps work it out (`caps::Engine::check_caps`), and [`allowance_for`] is the
//! one place where it is written in the form the lease carries to the machine
//! that keeps the key: the forwarder adds up what it sees and refuses the next
//! request once a figure is reached, so that a harness that calls through it is
//! held to a cap within one request, even while the link to the API is down. A
//! lease no cap would stop carries none, and the forwarder then refuses only a
//! model the lease does not name.
//!
//! A lease is opened on the machine that keeps the key *after* the session
//! is open and *before* `turn.start`: a session lasts for many turns and a
//! lease for one. It is closed after the turn ends in any way, with the
//! reason the turn ended for; a lease whose turn is over and that was never
//! closed (a crash between the two, or a turn that was ended by something
//! else) is found by the supervisor and closed too, and the machine is told.
//!
//! A credential that leaves the provider (a copy that is issued, the login
//! itself that is handed over) runs under a lease of the modes `shared` and
//! `handover`. It is opened by the start sequence of the turn and not by
//! pre-flight, since a lease of the mode `shared` names the one machine a copy
//! may be issued to, which is the machine where the harness runs, and that is
//! not known until the environment of the turn is up ([`open_for_credential`]).

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::models;
use crate::unit::Unit;
use orch_agent_contract::credentials::CredentialPath;
use orch_agent_contract::parameters::Rate;
use orch_db::queries::resources::provider_runtime as credential_rows;
use orch_db::queries::resources::thread_engine::ProviderRow;
use orch_db::queries::threads::{self, LeaseRow, NewLease, turn_state};
use orch_db::usage::micros::Micros;
use orch_link_contract::commands::{LeaseClose, LeaseMode, LeaseOpen, SerialKey};
use orch_types::{Id, Json, new_id};

/// The `leases.close_reason` of a lease whose turn ended on its own.
pub const TURN_ENDED: &str = "turn_ended";
/// The `leases.close_reason` of a lease whose turn was stopped.
pub const INTERRUPTED: &str = "interrupted";
/// The `leases.close_reason` of a lease whose turn failed.
pub const ERROR: &str = "error";
/// The `leases.close_reason` of a lease whose turn was ended because a usage cap was reached.
pub const CAP_REACHED: &str = "cap_reached";

/// The mode of the lease a credential path needs; none for the path on which
/// the harness uses the provider's own login.
#[must_use]
pub const fn mode_of(path: CredentialPath) -> Option<LeaseMode> {
    match path {
        CredentialPath::Native => None,
        CredentialPath::Forwarded => Some(LeaseMode::Forwarded),
        CredentialPath::Shared => Some(LeaseMode::Shared),
        CredentialPath::Handover => Some(LeaseMode::Handover),
    }
}

/// The word the lease table has for a mode.
const fn mode_word(mode: LeaseMode) -> &'static str {
    match mode {
        LeaseMode::Forwarded => "forwarded",
        LeaseMode::Shared => "shared",
        LeaseMode::Handover => "handover",
    }
}

/// The credential keeper recorded independently from the provider's runtime machine.
#[must_use]
pub fn machine_of(provider: &ProviderRow) -> Option<Id> {
    provider.key_machine_id
}

/// What a lease carries of the room a turn has before the forwarder refuses
/// it, as `{ tokens?, credits?: { amount, rate } }`: the least room left among
/// the covering caps in tokens whose `at_limit` is `stop` that the turn was not
/// let past, and the same in credits with the rate its requests are counted at.
/// None where no such cap covers the turn: the lease then carries no allowance.
#[must_use]
pub fn allowance_for(tokens: Option<i64>, credits: Option<(Micros, &Rate)>) -> Option<Json> {
    let mut allowance = serde_json::Map::new();
    if let Some(tokens) = tokens {
        allowance.insert("tokens".to_owned(), Json::from(tokens));
    }
    if let Some((amount, rate)) = credits
        && let Ok(rate) = serde_json::to_value(rate)
    {
        allowance.insert(
            "credits".to_owned(),
            serde_json::json!({ "amount": amount.text(), "rate": rate }),
        );
    }
    (!allowance.is_empty()).then_some(Json::Object(allowance))
}

/// Opens the lease of the turn that is about to be made, for the one model
/// the turn runs ([`models::lease_models`]). A lease that an earlier turn
/// left open (a crash between its end and its closing) is closed first: a
/// thread has one open lease at most.
pub async fn open_for_turn(
    unit: &mut Unit<'_>,
    thread: Id,
    provider: Id,
    model: &str,
    mode: LeaseMode,
    allowance: Option<Json>,
) -> Result<Option<LeaseRow>, EngineError> {
    let now = unit.now();
    if let Some(stale) = threads::open_lease_of(&mut *unit.conn, thread).await? {
        threads::close_lease(&mut *unit.conn, stale.id, ERROR, now).await?;
    }
    let allowed = models::lease_models(model);
    Ok(threads::open_lease(
        &mut *unit.conn,
        &NewLease {
            id: new_id(),
            provider_id: provider,
            thread_id: thread,
            mode: mode_word(mode),
            form: None,
            allowed_models: &allowed,
            allowance: allowance.as_ref(),
            at: now,
        },
    )
    .await?)
}

/// Opens the lease of a turn whose harness is given the provider's credential: the mode `shared` (a
/// copy; `recipient` is the machine where the harness runs, the only one a copy may go to) or `handover`
/// (the login itself; its recipient is recorded once the credential has been delivered). A lease that an
/// earlier turn left open is closed first.
pub async fn open_for_credential(
    unit: &mut Unit<'_>,
    thread: Id,
    provider: Id,
    model: &str,
    form: &str,
    handover: bool,
    recipient: Option<Id>,
) -> Result<Option<LeaseRow>, EngineError> {
    let now = unit.now();
    if let Some(stale) = threads::open_lease_of(&mut *unit.conn, thread).await? {
        threads::close_lease(&mut *unit.conn, stale.id, ERROR, now).await?;
    }
    let allowed = models::lease_models(model);
    Ok(credential_rows::open_credential_lease(
        &mut *unit.conn,
        &credential_rows::NewCredentialLease {
            id: new_id(),
            provider_id: provider,
            thread_id: thread,
            mode: if handover { "handover" } else { "shared" },
            form,
            recipient_machine_id: recipient,
            allowed_models: &allowed,
            at: now,
        },
    )
    .await?)
}

/// Closes the lease of a turn that ended, for `reason`. The answer is the
/// lease, where it was open: `lease.close` is to be sent for it.
pub async fn close_for_turn(
    unit: &mut Unit<'_>,
    thread: Id,
    reason: &str,
) -> Result<Option<LeaseRow>, EngineError> {
    let Some(lease) = threads::open_lease_of(&mut *unit.conn, thread).await? else {
        return Ok(None);
    };
    let now = unit.now();
    let closed = threads::close_lease(&mut *unit.conn, lease.id, reason, now).await?;
    Ok(closed.then_some(lease))
}

impl Engine {
    /// Sends `lease.open` for a lease to where the credential is kept and
    /// waits for the answer. Asked again, the machine answers with what it
    /// answered.
    pub(crate) async fn open_lease_at_provider(
        &self,
        lease: &LeaseRow,
        provider: &ProviderRow,
        protocol: Option<String>,
        attempt: u32,
    ) -> Result<Outcome, EngineError> {
        let machine = machine_of(provider).ok_or_else(|| {
            EngineError::unreadable("the provider has no machine to keep a lease on")
        })?;
        let mode = match lease.mode.as_str() {
            "shared" => LeaseMode::Shared,
            "handover" => LeaseMode::Handover,
            _ => LeaseMode::Forwarded,
        };
        let body = LeaseOpen {
            lease_id: lease.id,
            provider_id: lease.provider_id,
            thread_id: lease.thread_id,
            mode,
            protocol,
            form: lease.form.clone(),
            recipient: None,
            allowed_models: lease.allowed_models.clone(),
            allowance: lease
                .allowance
                .clone()
                .map(serde_json::from_value)
                .transpose()?,
        };
        // A forwarded lease is its thread's own (the forwarder keeps one entry for each lease), so it
        // waits only for its own thread's commands: keyed by the provider, every thread's start waited for
        // every other thread's lease on that provider, and with ten threads on one provider 20 of 308
        // missed their 15-second deadline before they could start (C-turns). A credential that is shared or
        // handed over moves the provider's sign-in and stays in its provider's line, as do closings, which a
        // purge sends before it stops the provider.
        let key = if body.mode == LeaseMode::Forwarded {
            SerialKey::Thread(lease.thread_id)
        } else {
            SerialKey::Provider(lease.provider_id)
        };
        let command = Outgoing::new(ids::lease_open(lease.id, attempt), machine, key, &body)?;
        Ok(crate::commands::run(self.inner.commands.as_ref(), command).await?)
    }

    /// Closes the leases that no turn holds: open, with a turn that is over
    /// (or gone). What a crash between the end of a turn and the closing of
    /// its lease leaves, and what the end of a turn by something else than
    /// [`Engine::end_turn`] leaves. The reason is the one the turn's end gives
    /// a lease; the machine that keeps the key is told by the look at closed
    /// leases that follows.
    pub(crate) async fn close_stale_leases(&self) -> Result<(), EngineError> {
        let stale = {
            let mut conn = self.inner.db.acquire().await?;
            threads::stale_open_leases(&mut conn, self.inner.config.sweep_batch).await?
        };
        for lease in stale {
            let mut conn = self.inner.db.acquire().await?;
            let state = match lease.turn_id {
                Some(turn) => threads::turn(&mut conn, turn).await?.map(|turn| turn.state),
                None => None,
            };
            let reason = match state.as_deref() {
                Some(turn_state::COMPLETED) => TURN_ENDED,
                Some(turn_state::INTERRUPTED) => INTERRUPTED,
                _ => ERROR,
            };
            let now = self.inner.clock.now();
            if threads::close_lease(&mut conn, lease.id, reason, now).await? {
                tracing::info!(lease = %lease.id, thread = %lease.thread_id, "a lease that no turn held was closed");
                self.wake(lease.thread_id);
            }
        }
        Ok(())
    }

    /// Sends `lease.close` for a lease that was closed and reconciles its final usage when it
    /// answers. A lease that cannot be closed is closed by the provider when it forgets it.
    pub(crate) async fn send_lease_close(
        &self,
        lease: &LeaseRow,
        provider: &ProviderRow,
    ) -> Result<(), EngineError> {
        let Some(machine) = machine_of(provider) else {
            return Ok(());
        };
        let command = Outgoing::new(
            ids::lease_close(lease.id),
            machine,
            SerialKey::Provider(lease.provider_id),
            &LeaseClose { lease_id: lease.id },
        )?;
        self.inner.commands.issue(command).await?;
        // What the forwarder counted in all is compared with the ledger once it answers; nothing waits
        // for that.
        let engine = self.clone();
        let (lease_id, provider) = (lease.id, provider.clone());
        tokio::spawn(async move {
            match engine.inner.commands.wait(ids::lease_close(lease_id)).await {
                Ok(crate::commands::Outcome::Succeeded(answer)) => {
                    if let Err(error) = engine
                        .reconcile_lease_close(lease_id, &provider, &answer)
                        .await
                    {
                        tracing::warn!(%error, lease = %lease_id, "what a lease used in all could not be compared with the ledger");
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, lease = %lease_id, "the answer to closing a lease was not heard");
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_lease_carries_the_room_that_is_left_in_tokens_and_in_credits_and_nothing_when_no_cap_would_stop_it()
     {
        assert_eq!(allowance_for(None, None), None);
        assert_eq!(
            allowance_for(Some(250_000), None),
            Some(json!({ "tokens": 250_000 }))
        );
        let rate: Rate = serde_json::from_value(json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10"
        }))
        .unwrap();
        let both = allowance_for(Some(1), Some((Micros::parse("12.5").unwrap(), &rate))).unwrap();
        assert_eq!(both["credits"]["amount"], "12.5");
        assert_eq!(both["credits"]["rate"]["input_per_mtok"], "2");
        assert_eq!(both["tokens"], 1);
    }

    #[test]
    fn only_a_harness_that_leaves_its_providers_login_runs_under_a_lease() {
        assert_eq!(mode_of(CredentialPath::Native), None);
        assert_eq!(
            mode_of(CredentialPath::Forwarded),
            Some(LeaseMode::Forwarded)
        );
        assert_eq!(mode_of(CredentialPath::Shared), Some(LeaseMode::Shared));
        assert_eq!(mode_of(CredentialPath::Handover), Some(LeaseMode::Handover));
    }
}
