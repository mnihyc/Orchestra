//! Reviewed permanent loss closes unreachable work atomically and preserves its recoverable identity.

use crate::{Engine, EngineError};
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    loss::*,
    nodes::{NodeKind, NodeRef},
};
use orch_authz::{Act, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{jobs, loss as rows},
};
use orch_types::{Hash, Id, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Why a permanent-loss request cannot be accepted.
#[derive(Debug, thiserror::Error)]
pub enum LossError {
    /// The resource or reviewed plan is not available to this caller.
    #[error("the resource or loss plan is not available")]
    NotFound,
    /// The caller does not manage the resource.
    #[error("only this resource's managers or the machine's owner may declare it lost")]
    Denied,
    /// Loss cannot be declared while the resource answers.
    #[error("the resource still answers; stop or remove it through its ordinary controls")]
    Online,
    /// A dependency or authority changed after the preview.
    #[error("what is on this resource changed; review a fresh loss plan")]
    Stale,
    /// Typed names must match exactly.
    #[error("type the resource's exact name to declare it lost")]
    Confirmation,
    /// The database could not answer.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The orchestration infrastructure failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

#[derive(Serialize, Deserialize)]
struct Stored {
    plan: LossPlan,
    fingerprint: Hash,
}

/// Checks the source actor without borrowing authority over environments that merely run there.
pub async fn allowed(
    conn: &mut Conn,
    principal: &Principal,
    target: LossTarget,
) -> Result<bool, LossError> {
    if let LossTarget::Machine { id } = target {
        let owns = orch_db::queries::resources::machine(conn, id)
            .await?
            .is_some_and(|row| row.owner_id == Some(principal.user()));
        return Ok((owns
            || crate::operation_access::can_bypass(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Machine,
                    id,
                },
                Permission::MachineManage,
            )
            .await?)
            && orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Machine,
                    id,
                },
                Permission::MachineManage,
                Policy::default(),
            )
            .await
            .map_err(EngineError::from)?
            .is_allowed());
    }
    let (kind, permission) = match target {
        LossTarget::Host { .. } => (ObjectKind::Host, Permission::HostManage),
        _ => (ObjectKind::Environment, Permission::EnvironmentManage),
    };
    Ok(orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind,
            id: target.id(),
        },
        permission,
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?
    .is_allowed())
}

async fn visible(
    conn: &mut Conn,
    principal: &Principal,
    kind: ObjectKind,
    id: Id,
    permission: Permission,
) -> Result<bool, LossError> {
    Ok(orch_authz::resources::check(
        conn,
        principal,
        ObjectRef { kind, id },
        permission,
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?
    .is_allowed())
}

async fn shown(
    conn: &mut Conn,
    principal: &Principal,
    target: LossTarget,
    inventory: &rows::Inventory,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<LossPlan, LossError> {
    let mut environments = Vec::new();
    let mut kept_bytes = inventory.mount_bytes;
    for environment in &inventory.environments {
        let mut threads = Vec::new();
        let mut hidden = 0;
        for thread in orch_db::queries::environments::threads_in(conn, &[environment.id]).await? {
            if orch_authz::nodes::check(
                conn,
                principal,
                thread.thread_id,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(EngineError::from)?
            .is_allowed()
            {
                threads.push(NodeRef {
                    thread_display: None,
                    id: thread.thread_id,
                    kind: NodeKind::Thread,
                    name: Some(thread.name),
                    redacted: false,
                });
            } else {
                hidden += 1;
            }
        }
        let name = visible(
            conn,
            principal,
            ObjectKind::Environment,
            environment.id,
            Permission::EnvironmentView,
        )
        .await?
        .then(|| environment.name.clone());
        environments.push(LossEnvironment {
            environment: LossResource {
                id: environment.id,
                name,
            },
            threads,
            threads_hidden: hidden,
            kept_bytes: environment.retained_bytes,
        });
        if environment.retained {
            kept_bytes = kept_bytes
                .zip(environment.retained_bytes)
                .map(|(a, b)| a.saturating_add(b));
        }
    }
    let mut providers = Vec::new();
    for id in &inventory.providers {
        let name = if visible(
            conn,
            principal,
            ObjectKind::Provider,
            *id,
            Permission::ProviderView,
        )
        .await?
        {
            orch_db::queries::resources::thread_engine::provider(conn, *id)
                .await?
                .map(|p| p.name)
        } else {
            None
        };
        providers.push(LossResource { id: *id, name });
    }
    let mut mounts = Vec::new();
    for (id, name) in
        orch_db::queries::resources::hosting::mount_names(conn, &inventory.mounts).await?
    {
        mounts.push(LossResource {
            id,
            name: visible(
                conn,
                principal,
                ObjectKind::Mount,
                id,
                Permission::MountView,
            )
            .await?
            .then_some(name),
        });
    }
    let mut provider_threads = Vec::new();
    let mut provider_threads_hidden = 0;
    for thread in &inventory.provider_threads {
        if orch_authz::nodes::check(
            conn,
            principal,
            *thread,
            Permission::ThreadView,
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(EngineError::from)?
        .is_allowed()
        {
            if let Some(row) = orch_db::queries::threads::thread(conn, *thread).await? {
                provider_threads.push(NodeRef {
                    thread_display: None,
                    id: *thread,
                    kind: NodeKind::Thread,
                    name: Some(row.name),
                    redacted: false,
                });
            }
        } else {
            provider_threads_hidden += 1;
        }
    }
    Ok(LossPlan {
        admin_grants: Vec::new(),
        notification: Default::default(),
        preflight_id: Id::nil(),
        expires_at: Time::new(crate::updates::expires(now)),
        target,
        name: inventory.name.clone(),
        environments,
        provider_threads,
        provider_threads_hidden,
        mounts,
        providers,
        jobs: inventory.jobs.len() as i64,
        kept_bytes,
    })
}

/// Expands current resource managers through the same roles and groups as permission decisions.
pub(crate) async fn managers(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    wanted: Permission,
) -> Result<Vec<Id>, EngineError> {
    let (graph, _, site) = orch_authz::resources::site(conn, object).await?;
    let mut asked = vec![wanted];
    asked.extend(wanted.held_through());
    let frame = orch_authz::engine::Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &asked,
    )
    .await?;
    Ok(orch_authz::holders::who_holds_for_notifications(conn, &frame, &site, wanted).await?)
}

impl Engine {
    /// Plans permanent loss, with names filtered under the caller's current authority.
    pub async fn plan_loss(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        target: LossTarget,
    ) -> Result<LossPlan, LossError> {
        if !allowed(conn, principal, target).await? {
            return Err(LossError::Denied);
        }
        let inventory = rows::inventory(conn, target.kind(), target.id())
            .await?
            .ok_or(LossError::NotFound)?;
        if inventory.online || inventory.lost {
            return Err(LossError::Online);
        }
        let mut plan = shown(conn, principal, target, &inventory, self.inner.clock.now()).await?;
        let mut audience = crate::notifications::operation_audience(
            conn,
            principal,
            &inventory
                .environments
                .iter()
                .map(|environment| environment.id)
                .collect::<Vec<_>>(),
            &inventory.providers,
        )
        .await?;
        if let LossTarget::Machine { id } = target {
            let owner = orch_db::queries::resources::machine(conn, id)
                .await?
                .and_then(|row| row.owner_id);
            if owner != Some(principal.user()) {
                plan.admin_grants.push(OperationAdminGrant {
                    object: ObjectRef {
                        kind: ObjectKind::Machine,
                        id,
                    },
                    permission: Permission::MachineManage,
                    name: plan.name.clone(),
                });
            }
        } else {
            if let Some(grant) = crate::operation_access::review(
                conn,
                principal,
                ObjectRef {
                    kind: if matches!(target, LossTarget::Host { .. }) {
                        ObjectKind::Host
                    } else {
                        ObjectKind::Environment
                    },
                    id: target.id(),
                },
                if matches!(target, LossTarget::Host { .. }) {
                    Permission::HostManage
                } else {
                    Permission::EnvironmentManage
                },
            )
            .await
            .map_err(|error| match error {
                crate::updates::UpdateError::Denied => LossError::Denied,
                other => LossError::Internal(EngineError::unreadable(other)),
            })? {
                plan.admin_grants.push(grant);
            }
        }
        enrich_audience(conn, principal, target, &inventory, &mut audience).await?;
        let notification_place = match target {
            LossTarget::Environment { id } => orch_settings::Place::Environment(id),
            LossTarget::Host { id } => orch_db::queries::resources::hosting::host(conn, id)
                .await?
                .and_then(|host| host.home_id)
                .map_or(orch_settings::Place::System, orch_settings::Place::Node),
            LossTarget::Machine { .. } => orch_settings::Place::System,
        };
        plan.preflight_id = new_id();
        plan.notification = OperationNotificationPlan {
            preview_id: Some(plan.preflight_id),
            people: audience.len() as i64,
            default_on: crate::notifications::operation_default(
                conn,
                notification_place,
                !inventory.threads.is_empty() || !inventory.providers.is_empty(),
            )
            .await?,
        };
        let stored = Stored {
            plan: plan.clone(),
            fingerprint: inventory.fingerprint()?,
        };
        jobs::forks::save_preflight(
            conn,
            &jobs::forks::PreflightRow {
                id: plan.preflight_id,
                kind: "loss".into(),
                node_id: None,
                environment_id: match target {
                    LossTarget::Environment { id } => Some(id),
                    _ => None,
                },
                created_by: principal.user(),
                plan: serde_json::to_value(stored).map_err(EngineError::from)?,
                expires_at: plan.expires_at.get(),
                created_at: self.inner.clock.now(),
            },
        )
        .await?;
        orch_db::queries::notifications::operations::preview(
            conn,
            plan.preflight_id,
            &audience
                .iter()
                .map(|person| person.user)
                .collect::<Vec<_>>(),
        )
        .await?;
        Ok(plan)
    }

    /// Declares an unchanged reviewed inventory lost inside the caller's mutation transaction.
    pub async fn declare_loss(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        body: &LossDeclareBody,
    ) -> Result<LossPlan, LossError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(EngineError::unreadable("loss declaration requires a transaction").into());
        }
        orch_db::queries::transaction_guard::lock_authority(conn).await?;
        let row = jobs::changes::lock_plan(conn, body.preflight_id)
            .await?
            .filter(|row| row.kind == "loss" && row.created_by == principal.user())
            .ok_or(LossError::NotFound)?;
        let stored: Stored = serde_json::from_value(row.plan).map_err(EngineError::from)?;
        let target = stored.plan.target;
        if !allowed(conn, principal, target).await? {
            return Err(LossError::Denied);
        }
        if row.expires_at <= self.inner.clock.now() {
            return Err(LossError::Stale);
        }
        if body.confirm != stored.plan.name {
            return Err(LossError::Confirmation);
        }
        orch_db::queries::resources::lock_resource(conn, target.kind(), target.id()).await?;
        let before = rows::inventory(conn, target.kind(), target.id())
            .await?
            .ok_or(LossError::NotFound)?;
        for environment in &before.environments {
            orch_db::queries::environments::hosting::lock_environment(conn, environment.id).await?;
        }
        for machine in &before.machines {
            orch_db::queries::resources::reenrollment::lock(conn, *machine).await?;
        }
        let inventory = rows::inventory(conn, target.kind(), target.id())
            .await?
            .ok_or(LossError::NotFound)?;
        if inventory.online {
            return Err(LossError::Online);
        }
        if inventory.fingerprint()? != stored.fingerprint {
            return Err(LossError::Stale);
        }
        let now = self.inner.clock.now();
        let mut audience = crate::notifications::operation_audience(
            conn,
            principal,
            &inventory
                .environments
                .iter()
                .map(|environment| environment.id)
                .collect::<Vec<_>>(),
            &inventory.providers,
        )
        .await?;
        enrich_audience(conn, principal, target, &inventory, &mut audience).await?;
        if !orch_db::queries::notifications::operations::matches_preview(
            conn,
            body.preflight_id,
            &audience
                .iter()
                .map(|person| person.user)
                .collect::<Vec<_>>(),
        )
        .await?
        {
            return Err(LossError::Stale);
        }
        let operation = crate::notifications::NotificationOperation {
            id: body.preflight_id,
            kind: "lost".into(),
            name: inventory.name.clone(),
            notify: body.notify,
            path: format!("/resources/{}s/{}", target.kind(), target.id()),
            subject: Some((target.kind().into(), target.id())),
            recipients: audience,
        };
        orch_db::queries::notifications::operations::save(
            conn,
            operation.id,
            &serde_json::to_value(&operation).map_err(EngineError::from)?,
            now,
        )
        .await?;
        crate::operation_access::accept(
            conn,
            principal,
            &stored.plan.admin_grants,
            &orch_db::audit::Origin::default(),
        )
        .await
        .map_err(|error| match error {
            crate::updates::UpdateError::Denied => LossError::Denied,
            other => LossError::Internal(EngineError::unreadable(other)),
        })?;
        rows::close(conn, &inventory, principal.user(), now).await?;
        for job in &inventory.jobs {
            crate::recovery::release_history_pins(conn, *job, now).await?;
        }
        let mut unit = crate::unit::Unit::new(conn, now);
        for id in inventory.threads.iter().chain(&inventory.provider_threads) {
            for approval in orch_db::queries::approvals::pending_of_thread(unit.conn, *id).await? {
                if orch_db::queries::approvals::cancel_pending(unit.conn, approval.id, now).await? {
                    orch_db::queries::notifications::inbox::settle_approval(
                        unit.conn,
                        approval.id,
                        now,
                    )
                    .await?;
                }
            }
            let seq = unit.stamp(*id).await?;
            // A lost source can never confirm delivery. Keep attempted words visibly failed for their author.
            for input in orch_db::queries::threads::delivering_inputs(unit.conn, *id).await? {
                orch_db::queries::threads::change_input(
                    unit.conn,
                    input.id,
                    &["delivering"],
                    &orch_db::queries::threads::InputChange {
                        state: "failed",
                        detail: Some(if inventory.threads.contains(id) {
                            "environment_lost_unconfirmed"
                        } else {
                            "provider_runtime_lost_unconfirmed"
                        }),
                        ..Default::default()
                    },
                    seq,
                )
                .await?;
                crate::ingest::settle_user_message(&mut unit, *id, input.id, "failed").await?;
            }
            sqlx::query("UPDATE turns SET seq_updated=$2 WHERE thread_id=$1 AND state='interrupted' AND error->>'code' IN ('environment_lost','provider_runtime_lost') AND completed_at=$3").bind(id).bind(seq).bind(now).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            sqlx::query("UPDATE items SET status='cancelled',updated_at=$3,seq_updated=$2 WHERE thread_id=$1 AND status='in_progress' AND kind<>'user_message' AND turn_id IN(SELECT id FROM turns WHERE thread_id=$1 AND state='interrupted' AND error->>'code' IN ('environment_lost','provider_runtime_lost') AND completed_at=$3)").bind(id).bind(seq).bind(now).execute(&mut *unit.conn).await.map_err(DbError::from)?;

            sqlx::query("UPDATE threads SET state='blocked',blocked_reason=$3,blocked_detail=$4,starting_step=NULL,current_turn_id=NULL,seq_state=GREATEST(seq_state,$2) WHERE node_id=$1")
                .bind(id).bind(seq).bind(if inventory.threads.contains(id){"environment_unavailable"}else{"provider_unavailable"}).bind(if inventory.threads.contains(id){"Its environment was declared lost"}else{"Its provider runtime was declared lost"}).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            unit.wake(*id);
        }
        unit.finish().await?;
        crate::notifications::notify_operation(conn, &operation, "done", now).await?;
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(principal.user()),
            match target {
                LossTarget::Host { .. } => orch_db::audit::name!("host.lost"),
                LossTarget::Environment { .. } => orch_db::audit::name!("environment.lost"),
                LossTarget::Machine { .. } => orch_db::audit::name!("machine.lost"),
            },
            target.kind(),
            Some(target.id()),
            "Declared permanently lost",
        );
        entry.history = true;
        entry.r#override = !stored.plan.admin_grants.is_empty();
        entry.details = json!({"environments":inventory.environments.iter().map(|env|env.id).collect::<Vec<_>>(),"jobs":inventory.jobs,"kept_bytes":stored.plan.kept_bytes});
        orch_db::audit::append(conn, &entry, &orch_db::audit::Origin::default(), now).await?;
        orch_authz::epoch::bump(conn)
            .await
            .map_err(EngineError::from)?;
        jobs::forks::forget_preflight(conn, body.preflight_id).await?;
        Ok(stored.plan)
    }
}

async fn enrich_audience(
    conn: &mut Conn,
    principal: &Principal,
    target: LossTarget,
    inventory: &rows::Inventory,
    audience: &mut Vec<crate::notifications::OperationRecipient>,
) -> Result<(), LossError> {
    let mut resources = inventory
        .mounts
        .iter()
        .map(|id| {
            (
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id: *id,
                },
                Permission::MountManage,
            )
        })
        .collect::<Vec<_>>();
    match target {
        LossTarget::Host { id } => resources.push((
            ObjectRef {
                kind: ObjectKind::Host,
                id,
            },
            Permission::HostManage,
        )),
        LossTarget::Machine { id } => resources.push((
            ObjectRef {
                kind: ObjectKind::Machine,
                id,
            },
            Permission::MachineManage,
        )),
        LossTarget::Environment { .. } => {}
    }
    let servers:Vec<Id>=sqlx::query_scalar("SELECT DISTINCT server_id FROM mcp_server_offerings WHERE machine_id=ANY($1) AND state<>'lost'").bind(&inventory.machines).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    resources.extend(servers.into_iter().map(|id| {
        (
            ObjectRef {
                kind: ObjectKind::McpServer,
                id,
            },
            Permission::McpManage,
        )
    }));
    crate::notifications::extend_operation_audience(conn, principal, audience, &resources).await?;
    Ok(())
}
