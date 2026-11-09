//! Physical program updates: one durable drain fence for web and automatic requests.

use crate::backplane::Link;
use orch_api_contract::resources::ComponentUpdate;
use orch_db::queries::{component_updates as updates, resources};
use orch_db::{Conn, DbError};
use orch_engine::component_update_authority::{RequestOrigin as Origin, StoredOrigin};
use orch_link::streams::Delivery;
use orch_link_contract::commands::{MachineUpdateNow, SerialKey};
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::{Clock, Id, Time};
use std::collections::HashMap;

/// Current running/recommended facts for machine and node lists in one batch.
pub async fn views(
    link: &Link,
    conn: &mut Conn,
    ids: &[Id],
) -> Result<HashMap<Id, ComponentUpdate>, DbError> {
    let progress: HashMap<_, _> = updates::among(conn, ids)
        .await?
        .into_iter()
        .map(|row| (row.machine_id, row))
        .collect();
    let machines: Vec<(Id, String, Vec<String>)> =
        sqlx::query_as("SELECT id,connector_version,roles FROM machines WHERE id=ANY($1)")
            .bind(ids)
            .fetch_all(&mut *conn)
            .await?;
    let recommended = &link.inner.settings.versions.recommended;
    Ok(machines
        .into_iter()
        .map(|(id, running, roles)| {
            let row = progress.get(&id);
            let behind = newer(recommended, &running);
            let pending = row.filter(|row| newer(&row.recommended_version, &running));
            let view = ComponentUpdate {
                running_version: running,
                recommended_version: recommended.clone(),
                behind,
                status: pending
                    .map_or("current", |row| row.state.as_str())
                    .to_owned(),
                detail: pending.and_then(|row| row.detail.clone()),
                command: if roles.iter().any(|role| role == "node") {
                    "orch-node update"
                } else {
                    "orch-connector update"
                }
                .into(),
            };
            (id, view)
        })
        .collect())
}

/// Canonical semantic comparison; malformed or development versions never invent an upgrade.
#[must_use]
pub fn newer(recommended: &str, running: &str) -> bool {
    match (
        semver::Version::parse(recommended.trim_start_matches('v')),
        semver::Version::parse(running.trim_start_matches('v')),
    ) {
        (Ok(recommended), Ok(running)) => recommended > running,
        _ => false,
    }
}

/// The actual pending request and its own original authority across native grant waits.
struct TrackedRequest {
    id: Id,
    machine: Id,
    recommended: String,
    origin: Option<Origin>,
}
struct CheckedRequest {
    caller: Option<orch_authz::Caller>,
    until: Option<Time>,
}
impl CheckedRequest {
    fn current(&self, clock: &dyn Clock) -> bool {
        self.caller
            .as_ref()
            .is_none_or(|caller| caller.key_deadline_allows(clock))
            && self.until.is_none_or(|until| until.get() > clock.now())
    }
}
impl TrackedRequest {
    async fn load(conn: &mut Conn, machine: Id) -> Result<Option<Self>, DbError> {
        let Some(row) = updates::origin(conn, machine).await? else {
            return Ok(None);
        };
        if !matches!(row.state.as_str(), "queued" | "updating") {
            return Ok(None);
        }
        let origin = match Origin::load(conn, machine).await? {
            StoredOrigin::Legacy => None,
            StoredOrigin::Bound(origin) => Some(origin),
            StoredOrigin::Invalid => return Ok(None),
        };
        Ok(Some(Self {
            id: row.request_id,
            machine,
            recommended: row.recommended_version,
            origin,
        }))
    }
    async fn check(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<CheckedRequest>, DbError> {
        let Some(current) = Self::load(conn, self.machine).await? else {
            return Ok(None);
        };
        if current.id != self.id
            || current.recommended != self.recommended
            || current.origin != self.origin
        {
            return Ok(None);
        }
        match &self.origin {
            None => Ok(Some(CheckedRequest {
                caller: None,
                until: None,
            })),
            Some(origin) => match origin.current(conn, clock).await {
                Ok(Some((caller, until))) => Ok(Some(CheckedRequest {
                    caller: Some(caller),
                    until,
                })),
                Ok(None) => Ok(None),
                Err(orch_engine::EngineError::Db(error)) => Err(error),
                Err(_) => Ok(None),
            },
        }
    }
}

pub(crate) struct MinimumGrant {
    tracked: TrackedRequest,
}

impl Link {
    /// The authenticated recovery path uses its real request, ordinary actor and same drain.
    pub(crate) async fn begin_minimum_component_update(
        &self,
        conn: &mut Conn,
        machine: Id,
    ) -> Result<Option<MinimumGrant>, DbError> {
        let Some(tracked) = TrackedRequest::load(conn, machine).await? else {
            return Ok(None);
        };
        let Some(checked) = tracked.check(conn, self.inner.clock.as_ref()).await? else {
            updates::finish_request(
                conn,
                tracked.id,
                machine,
                "refused",
                Some("The update request's original authority or local policy has ended."),
                self.inner.clock.now(),
            )
            .await?;
            return Ok(None);
        };
        if updates::busy(conn, machine).await? || !checked.current(self.inner.clock.as_ref()) {
            return Ok(None);
        }
        updates::grant_minimum(conn, machine, self.inner.clock.now()).await?;
        if !checked.current(self.inner.clock.as_ref()) {
            updates::finish_request(
                conn,
                tracked.id,
                machine,
                "refused",
                Some("The update request's original authority ended before the grant."),
                self.inner.clock.now(),
            )
            .await?;
            return Ok(None);
        }
        Ok(Some(MinimumGrant { tracked }))
    }

    /// Entry/role preparation can wait after the original transaction. Publish only that
    /// same request's still-current grant, never authority borrowed from its successor.
    pub(crate) async fn finish_minimum_component_update(
        &self,
        grant: MinimumGrant,
    ) -> Result<Option<orch_link_contract::session::UpdatePermit>, DbError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        updates::lock(&mut tx, grant.tracked.machine).await?;
        let checked = grant
            .tracked
            .check(&mut tx, self.inner.clock.as_ref())
            .await?;
        let allowed = checked
            .as_ref()
            .is_some_and(|checked| checked.current(self.inner.clock.as_ref()));
        if !allowed {
            updates::finish_request(
                &mut tx,
                grant.tracked.id,
                grant.tracked.machine,
                "refused",
                Some("The update request's original authority ended before delivery."),
                self.inner.clock.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        if !allowed
            || checked
                .as_ref()
                .is_some_and(|checked| !checked.current(self.inner.clock.as_ref()))
        {
            return Ok(None);
        }
        Ok(Some(orch_link_contract::session::UpdatePermit {
            recommended: grant.tracked.recommended,
        }))
    }

    /// One audited automatic producer, used by a ready machine and a proved minimum-version node.
    pub(crate) async fn request_automatic_component_update(
        &self,
        conn: &mut Conn,
        machine: Id,
        running: &str,
        recommended: &str,
    ) -> Result<(), DbError> {
        if updates::request(conn, machine, recommended, None, self.inner.clock.now()).await? {
            let action = orch_db::audit::action!(
                orch_access_contract::audit::AuditAction::MachineUpdateRequested
            );
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::System,
                action.as_str(),
                "machine",
                Some(machine),
                "Automatic verified program update requested",
            );
            entry.details =
                serde_json::json!({"running":running,"recommended":recommended,"automatic":true});
            orch_db::audit::append(
                conn,
                &entry,
                &orch_db::audit::Origin::default(),
                self.inner.clock.now(),
            )
            .await?;
        }
        Ok(())
    }

    /// Applies only authenticated Machine-stream update events. Child connectors cannot ask to replace a node.
    pub(crate) async fn apply_component_updates(
        &self,
        conn: &mut Conn,
        machine: Id,
        delivery: &Delivery,
    ) -> Result<(), DbError> {
        if delivery.stream != Stream::Machine {
            return Ok(());
        }
        let Some(row) = resources::machine(conn, machine).await? else {
            return Ok(());
        };
        if row.placement != "connected" && !row.roles.iter().any(|role| role == "node") {
            return Ok(());
        }
        for event in &delivery.events {
            match Event::decode(&event.kind, event.body.clone()) {
                Ok(Event::MachineUpdateRequested(request))
                    if request.recommended == self.inner.settings.versions.recommended
                        && newer(&request.recommended, &row.connector_version) =>
                {
                    self.request_automatic_component_update(
                        conn,
                        machine,
                        &row.connector_version,
                        &request.recommended,
                    )
                    .await?;
                }
                Ok(Event::MachineUpdateFailed(failure)) => {
                    updates::finish(
                        conn,
                        machine,
                        "failed",
                        Some(&failure.detail),
                        self.inner.clock.now(),
                    )
                    .await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// One pass of the link's existing one-second clock; no network is held inside a transaction.
    pub(crate) async fn advance_component_updates(
        &self,
    ) -> Result<(), crate::dispatch::DispatchError> {
        let pending = {
            let mut conn = self.inner.db.acquire().await?;
            updates::pending(&mut conn).await?
        };
        for request in pending {
            let mut tx = self.inner.db.begin().await?;
            orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            match updates::lock(&mut tx, request.machine_id).await {
                Ok(()) => {}
                Err(DbError::Retry) => {
                    tx.rollback().await.map_err(DbError::from)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            let Some(machine) = resources::machine(&mut tx, request.machine_id).await? else {
                continue;
            };
            let live = updates::among(&mut tx, &[request.machine_id]).await?.pop();
            let Some(live) = live.filter(|row| matches!(row.state.as_str(), "queued" | "updating"))
            else {
                continue;
            };
            let now = self.inner.clock.now();
            if !newer(&live.recommended_version, &machine.connector_version)
                || machine.status != "active"
                || machine.deleted_at.is_some()
            {
                updates::finish(&mut tx, machine.id, "current", None, now).await?;
                tx.commit().await.map_err(DbError::from)?;
                self.component_update_finished(machine.id);
                continue;
            }
            let command = live.command_id.unwrap_or_else(orch_types::new_id);
            let tracked = TrackedRequest::load(&mut tx, machine.id).await?;
            let saved = orch_db::queries::link::command_with_authority(&mut tx, command).await?;
            if let Some(saved) = saved {
                if let Some(origin) = tracked.as_ref().and_then(|tracked| tracked.origin.as_ref()) {
                    let proposed = orch_engine::commands::Outgoing::new(
                        command,
                        machine.id,
                        SerialKey::Machine,
                        &MachineUpdateNow {
                            recommended: live.recommended_version.clone(),
                        },
                    )
                    .map_err(|_| crate::dispatch::DispatchError::Unavailable)?;
                    let expected = orch_engine::commands::CommandAuthority::component_update(
                        origin
                            .command(&proposed)
                            .map_err(|_| crate::dispatch::DispatchError::Unavailable)?,
                    );
                    if saved.authority != Some(serde_json::to_value(expected)?) {
                        // An older canonical command keeps its actual owner and None metadata.
                        // End only this unfiled request's attempted association with it.
                        updates::finish(&mut tx,machine.id,"refused",Some("The retained command belongs to another update request. Request Update now again."),now).await?;
                        tx.commit().await.map_err(DbError::from)?;
                        self.component_update_finished(machine.id);
                        continue;
                    }
                } else if updates::origin(&mut tx, machine.id)
                    .await?
                    .is_some_and(|request| request.origin_required || request.origin.is_some())
                {
                    updates::finish(
                        &mut tx,
                        machine.id,
                        "refused",
                        Some("The update request's original source is no longer valid."),
                        now,
                    )
                    .await?;
                    tx.commit().await.map_err(DbError::from)?;
                    self.component_update_finished(machine.id);
                    continue;
                }
                let row = saved.command;
                let mut finished = false;
                if matches!(row.state.as_str(), "failed" | "expired" | "cancelled") {
                    let detail = row.error.as_ref().and_then(|value| value.get("detail")).and_then(|value|value.as_str())
                        .unwrap_or("The update command failed before the machine accepted it. Retry Update now.");
                    finished =
                        updates::finish(&mut tx, machine.id, "refused", Some(detail), now).await?;
                } else if row.state == "succeeded"
                    && row
                        .result
                        .as_ref()
                        .and_then(|value| value.get("status"))
                        .and_then(|value| value.as_str())
                        == Some("current")
                {
                    finished = updates::finish(&mut tx, machine.id, "current", None, now).await?;
                }
                tx.commit().await.map_err(DbError::from)?;
                if finished {
                    self.component_update_finished(machine.id);
                }
                continue;
            }
            let checked = match &tracked {
                Some(tracked) => tracked.check(&mut tx, self.inner.clock.as_ref()).await?,
                None => None,
            };
            let Some(checked) =
                checked.filter(|checked| checked.current(self.inner.clock.as_ref()))
            else {
                updates::finish(&mut tx,machine.id,"refused",Some("The update request's original authority or local policy has ended. Request Update now again."),self.inner.clock.now()).await?;
                tx.commit().await.map_err(DbError::from)?;
                self.component_update_finished(machine.id);
                continue;
            };
            if updates::busy(&mut tx, machine.id).await? {
                continue;
            }
            // This node cannot receive commands on its old build. Its certificate-authenticated
            // handshake refusal carries the idle grant instead, while this same fence stays held.
            if machine.roles.iter().any(|role| role == "node")
                && matches!(
                    self.inner
                        .settings
                        .versions
                        .standing(&machine.connector_version),
                    Ok(orch_link_contract::version::Standing::TooOld)
                )
            {
                continue;
            }
            if live.command_id.is_none() {
                updates::grant(&mut tx, machine.id, command, now).await?;
            }
            if !checked.current(self.inner.clock.as_ref()) {
                tx.rollback().await.map_err(DbError::from)?;
                continue;
            }
            tx.commit().await.map_err(DbError::from)?;
            // The id survived the transaction; replaying after a crash has exactly the same command identity.
            let mut outgoing = orch_engine::commands::Outgoing::new(
                command,
                machine.id,
                SerialKey::Machine,
                &MachineUpdateNow {
                    recommended: live.recommended_version,
                },
            )
            .map_err(|_| crate::dispatch::DispatchError::Unavailable)?
            .by(Time::new(now + chrono::Duration::seconds(30)));
            if let Some(origin) = tracked.and_then(|tracked| tracked.origin) {
                let authority = origin
                    .command(&outgoing)
                    .map_err(|_| crate::dispatch::DispatchError::Unavailable)?;
                outgoing.authority = Some(
                    orch_engine::commands::CommandAuthority::component_update(authority),
                );
            }
            self.dispatcher()
                .issue_stored_before(
                    crate::dispatch::StoredCommand {
                        id: Some(outgoing.id),
                        machine: outgoing.machine,
                        kind: outgoing.kind.as_str().into(),
                        key: outgoing.key.wire(),
                        deadline: outgoing.deadline,
                        body: outgoing.body,
                        authority: outgoing.authority,
                    },
                    Box::new(|| {
                        if checked.current(self.inner.clock.as_ref()) {
                            Ok(())
                        } else {
                            Err(Box::new(crate::middleware::requests::ApiError::new(
                                orch_api_contract::common::ErrorCode::AuthzDenied,
                                "the original update issuer has ended",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>)
                        }
                    }),
                )
                .await?;
        }
        Ok(())
    }
}
