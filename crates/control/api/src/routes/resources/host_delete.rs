//! Reviewed host deletion runs independent recovery or removal before detaching native resources and revoking its machine.

use crate::{
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
    },
    registry::{Ctx, Registry},
};
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    common::ErrorCode, environment::EnvironmentMovePreflightBody, forks::PlanBlocker, loss as api,
    workspace::Job,
};
use orch_authz::Principal;
use orch_db::{
    Conn, DbError,
    queries::{
        environments::hosting as envs,
        jobs, loss,
        resources::{hosting as hosts, provider_runtime as providers},
    },
};
use orch_engine::{
    jobs::{
        environment_move, host_delete as operation, machine_move, provider_provision, recreate,
    },
    notifications, operation_access, recovery,
    updates::{self, Target, UpdateError},
};
use orch_types::{Hash, Id, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod data;

/// Binds the host's reviewed cascade instead of an empty-host-only control.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::HostDeletePreflight, preflight)
        .on(api::HostDeleteCreate, create);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum EnvironmentAction {
    Recreate {
        recipe: Box<recovery::Recipe>,
    },
    Move {
        recipe: Box<environment_move::MoveRecipe>,
        request: EnvironmentMovePreflightBody,
    },
    Portable {
        recipe: Box<machine_move::Recipe>,
        request: EnvironmentMovePreflightBody,
    },
    Remove {
        id: Id,
        revision: i64,
        lost: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ProviderAction {
    Leave {
        provider: Id,
        runtime: Id,
    },
    Replace {
        recipe: Box<updates::Recipe>,
        host: Id,
    },
    Make {
        recipe: Box<orch_engine::provider_recovery::Recipe>,
    },
}
#[derive(Serialize, Deserialize)]
struct Stored {
    host: Id,
    fingerprint: Hash,
    body: api::HostDeletePreflightBody,
    plan: api::HostDeletePlan,
    environments: Vec<EnvironmentAction>,
    providers: Vec<ProviderAction>,
    volumes: Vec<operation::Volume>,
    audience: Vec<notifications::OperationRecipient>,
    data: Option<data::Planned>,
}
fn blocked(code: &str, message: impl Into<String>) -> PlanBlocker {
    updates::blocker(code, message)
}
fn refused(error: UpdateError) -> ApiError {
    crate::routes::environments::updates::refused(error)
}
async fn add_grant(
    conn: &mut Conn,
    principal: &Principal,
    grants: &mut Vec<api::OperationAdminGrant>,
    object: ObjectRef,
    permission: Permission,
) -> ApiResult<()> {
    if let Some(grant) = operation_access::review(conn, principal, object, permission)
        .await
        .map_err(refused)?
        && !grants.contains(&grant)
    {
        grants.push(grant);
    }
    Ok(())
}
fn needs(
    grants: &[api::OperationAdminGrant],
    object: ObjectRef,
    permission: Permission,
) -> Vec<api::OperationAdminGrant> {
    grants
        .iter()
        .filter(|grant| grant.object == object && grant.permission == permission)
        .cloned()
        .collect()
}

fn retain_copy_names(
    old: &orch_engine::mount_copies::MountDecisions,
    fresh: &mut orch_engine::mount_copies::MountDecisions,
) {
    for copy in &mut fresh.copies {
        if let Some(reviewed) = old.copies.iter().find(|reviewed| {
            reviewed.mount_id == copy.mount_id && reviewed.target_machine == copy.target_machine
        }) {
            if reviewed.made
                && copy.made
                && reviewed.to.kind == "volume"
                && copy.to.kind == "volume"
            {
                copy.to.source = reviewed.to.source.clone();
            }
            if let Some(old) = reviewed
                .kept
                .as_ref()
                .and_then(|kept| kept.copied_into.as_ref())
                && let Some(new) = copy
                    .kept
                    .as_mut()
                    .and_then(|kept| kept.copied_into.as_mut())
                && old.kind == "volume"
                && new.kind == "volume"
            {
                new.source = old.source.clone();
            }
        }
    }
}

async fn draft_move(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    request: &mut EnvironmentMovePreflightBody,
    choices: &[orch_api_contract::forks::MountChoice],
) -> Result<environment_move::MoveDraft, UpdateError> {
    let draft = ctx
        .app
        .engine
        .draft_move(conn, principal, environment, request, None)
        .await?;
    let picks = choices
        .iter()
        .filter(|choice| {
            draft.mounts.iter().any(|mount| {
                mount.mount.id == choice.mount_id
                    && mount.fate == orch_api_contract::mounts::MountFate::Copy
                    && mount
                        .fates
                        .contains(&orch_api_contract::mounts::MountFate::Copy)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if picks.is_empty() {
        return Ok(draft);
    }
    request.mounts = Some(picks);
    ctx.app
        .engine
        .draft_move(conn, principal, environment, request, None)
        .await
}

/// All temporary grants and scheduler changes live only in this rolled-back preview transaction.
async fn build(
    ctx: &Ctx,
    principal: &Principal,
    host: Id,
    id: Id,
    body: &api::HostDeletePreflightBody,
) -> ApiResult<Stored> {
    let mut tx = ctx.app.db.begin().await?;
    let host_row = hosts::host(&mut tx, host)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut grants = Vec::new();
    add_grant(
        &mut tx,
        principal,
        &mut grants,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        Permission::HostManage,
    )
    .await?;
    operation_access::accept(
        &mut tx,
        principal,
        &grants,
        &orch_db::audit::Origin::default(),
    )
    .await
    .map_err(refused)?;
    let mut facts = orch_engine::resource_facts::custody::host(&mut tx, host)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let physical = body.erase.unwrap_or(true)
        || body.mounts == api::HostMountCascade::DeleteData
        || body.data.iter().any(|row| {
            matches!(
                row.fate,
                api::HostDataFate::Delete | api::HostDataFate::Move
            )
        });
    let inventory_measurement = if facts.inventory.online && !facts.inventory.lost && physical {
        let (inventory, measurement) = ctx
            .app
            .engine
            .host_custody_inventory(host, host_row.machine_id, principal.user())
            .await
            .map_err(ApiError::internal)?;
        if inventory.is_some() {
            facts.native = inventory;
        }
        Some(measurement)
    } else {
        None
    };
    let inventory = &facts.inventory;
    let fingerprint = inventory.fingerprint()?;
    let open:Option<Id>=sqlx::query_scalar("SELECT id FROM jobs WHERE kind='host_delete' AND state IN ('queued','running') AND plan#>>'{cascade,host}'=$1 ORDER BY created_at,id LIMIT 1").bind(host.to_string()).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
    if let Some(open) = open {
        let row = jobs::job(&mut tx, open)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let job = crate::routes::environments::jobs::view(&mut tx, &row).await?;
        let mut managed = std::collections::BTreeSet::new();
        for environment in &inventory.environments {
            if !matches!(
                operation_access::review(
                    &mut tx,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: environment.id
                    },
                    Permission::EnvironmentManage
                )
                .await,
                Err(UpdateError::Denied)
            ) {
                managed.insert(environment.id);
            }
        }
        let plan = api::HostDeletePlan {
            job: Some(job),
            notification: api::OperationNotificationPlan {
                preview_id: Some(id),
                people: 0,
                default_on: false,
            },
            admin_grants: grants,
            preflight_id: id,
            expires_at: Time::new(updates::expires(ctx.now)),
            name: host_row.name,
            online: inventory.online,
            rows: vec![api::HostCascadeRow {
                kind: "environments".into(),
                count: facts
                    .counts
                    .iter()
                    .find(|(kind, _)| kind == "environments")
                    .map_or(0, |(_, count)| *count),
                blockers: vec![blocked("host_busy", "Host removal is already running")],
            }],
            recovery: vec![],
            others: facts.pending_managers(&managed),
            volumes: vec![],
            kept_until: None,
            data: vec![],
            objects: vec![],
            erase: false,
            erase_blockers: vec![],
            inventory_measurement,
            executable: false,
        };
        tx.rollback().await.map_err(DbError::from)?;
        return Ok(Stored {
            host,
            fingerprint,
            body: body.clone(),
            plan,
            environments: vec![],
            providers: vec![],
            volumes: vec![],
            audience: vec![],
            data: None,
        });
    }

    // A host being removed must not be the automatic recovery destination.
    sqlx::query("UPDATE hosts SET schedulable=false WHERE id=$1")
        .bind(host)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let mut environment_actions = Vec::new();
    let mut rows = Vec::new();
    let mut recovery_rows = Vec::new();
    let mut prepared_recoveries = Vec::new();
    let mut managed = std::collections::BTreeSet::new();
    let mut environment_blockers = Vec::new();
    let source_mounts =
        orch_db::queries::resources::mounts::mounts(&mut tx, &inventory.mounts).await?;
    let copy_choices = source_mounts
        .iter()
        .filter(|mount| {
            mount.default_fate == "copy"
                && body.data.iter().any(|choice| {
                    choice.id == format!("mount:{}", mount.id)
                        && choice.fate == api::HostDataFate::Move
                })
        })
        .map(|mount| orch_api_contract::forks::MountChoice {
            mount_id: mount.id,
            fate: orch_api_contract::mounts::MountFate::Copy,
            into: Some(orch_api_contract::forks::CopyInto::NewLocation),
        })
        .collect::<Vec<_>>();
    for environment in &inventory.environments {
        if environment.status == "removed" {
            continue;
        }
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: environment.id,
        };
        match operation_access::review(&mut tx, principal, object, Permission::EnvironmentManage)
            .await
        {
            Err(UpdateError::Denied) => {
                continue;
            }
            Err(error) => return Err(refused(error)),
            Ok(_) => {}
        }
        managed.insert(environment.id);
        let prospective =
            operation_access::preview_recovery(&mut tx, principal, environment.id, body.host_id)
                .await
                .map_err(refused)?;
        grants.extend(needs(&prospective, object, Permission::EnvironmentManage));
        if body.environments == api::HostEnvironmentCascade::Remove {
            if environment.status != "lost" && !inventory.online {
                environment_blockers.push(blocked(
                    "source_offline",
                    "Declare the unreachable host lost first",
                ));
            }
            environment_actions.push(EnvironmentAction::Remove {
                id: environment.id,
                revision: environment.revision,
                lost: environment.status == "lost",
            });
            continue;
        }
        let shown = crate::routes::environments::show(ctx, &mut tx, principal, environment.id)
            .await?
            .base;
        let mut row = api::EnvironmentRecoveryRow {
            environment: shown,
            action: if environment.status == "lost" {
                api::RecoveryAction::Recreate
            } else {
                api::RecoveryAction::Move
            },
            host: None,
            files: None,
            listing_available: true,
            restored: false,
            restored_bytes: None,
            move_plan: None,
            switch_plan: None,
            blockers: Vec::new(),
        };
        if environment.status == "lost" {
            match ctx
                .app
                .engine
                .prepare_recovery(&mut tx, principal, environment.id, body.host_id)
                .await
            {
                Ok(prepared) => {
                    for (kind, id, permission) in [
                        (ObjectKind::Host, prepared.host(), Permission::HostUse),
                        (
                            ObjectKind::Template,
                            prepared.template(),
                            Permission::TemplateUse,
                        ),
                    ] {
                        grants.extend(needs(&prospective, ObjectRef { kind, id }, permission));
                    }
                    prepared_recoveries.push((recovery_rows.len(), prepared));
                }
                Err(UpdateError::Blocked(blockers)) => row.blockers = blockers,
                Err(error) => return Err(refused(error)),
            }
        } else if !inventory.online {
            row.blockers.push(blocked(
                "source_offline",
                "Declare the unreachable host lost first",
            ));
        } else {
            let mut request = EnvironmentMovePreflightBody {
                host_id: body.host_id,
                machine_id: None,
                template_id: None,
                copy_directories: None,
                home: None,
                private_views: None,
                files: None,
                check_fingerprints: None,
                fingerprint_request_id: None,
                mounts: None,
            };
            if machine_move::needed(&mut tx, environment.id, &request)
                .await
                .map_err(ApiError::internal)?
            {
                // Ordinary machine authority is read on its own connection; temporary grants are never committed.
                match ctx
                    .app
                    .engine
                    .draft_machine_move(principal, environment.id, &request, None)
                    .await
                {
                    Ok(draft) => {
                        let plan = crate::routes::environments::moves::shown_machine(
                            ctx, &mut tx, principal, id, &draft,
                        )
                        .await?;
                        row.blockers = plan.blockers.clone();
                        row.move_plan = Some(plan);
                        for (object, permission) in draft.recipe.destination_access() {
                            grants.extend(needs(&prospective, object, permission));
                        }
                        if row.blockers.is_empty() {
                            environment_actions.push(EnvironmentAction::Portable {
                                recipe: Box::new(draft.recipe),
                                request,
                            });
                        }
                    }
                    Err(UpdateError::Blocked(blockers)) => row.blockers = blockers,
                    Err(error) => return Err(refused(error)),
                }
            } else {
                match draft_move(
                    ctx,
                    &mut tx,
                    principal,
                    environment.id,
                    &mut request,
                    &copy_choices,
                )
                .await
                {
                    Ok(draft) => {
                        let plan = crate::routes::environments::moves::shown(
                            ctx, &mut tx, principal, id, &draft,
                        )
                        .await?;
                        row.blockers = plan.blockers.clone();
                        row.move_plan = Some(plan);
                        for (kind, id, permission) in [
                            (
                                ObjectKind::Host,
                                draft.recipe.destination,
                                Permission::HostUse,
                            ),
                            (
                                ObjectKind::Template,
                                draft.recipe.replacement.template,
                                Permission::TemplateUse,
                            ),
                        ] {
                            grants.extend(needs(&prospective, ObjectRef { kind, id }, permission));
                        }
                        if row.blockers.is_empty() {
                            environment_actions.push(EnvironmentAction::Move {
                                recipe: Box::new(draft.recipe),
                                request,
                            });
                        }
                    }
                    Err(UpdateError::Blocked(blockers)) => row.blockers = blockers,
                    Err(error) => return Err(refused(error)),
                }
            }
        }
        environment_blockers.extend(row.blockers.clone());
        recovery_rows.push(row);
    }
    rows.push(api::HostCascadeRow {
        kind: "environments".into(),
        count: inventory.environments.len() as i64,
        blockers: environment_blockers,
    });
    let data = data::plan(ctx, &mut tx, principal, host, body, &facts).await?;
    let volume_names = data
        .rows
        .iter()
        .filter(|row| row.selected == api::HostDataFate::Delete && row.kind == "mount")
        .map(|row| row.resource.clone())
        .collect();
    rows.push(api::HostCascadeRow {
        kind: "mount_locations".into(),
        count: 0,
        blockers: Vec::new(),
    });
    rows.push(api::HostCascadeRow {
        kind: "mcp_servers".into(),
        count: 0,
        blockers: Vec::new(),
    });
    let mut provider_actions = Vec::new();
    let mut provider_blockers = Vec::new();
    for provider in &inventory.providers {
        let state = providers::state(&mut tx, *provider)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let runtime = providers::live_runtime(&mut tx, *provider).await?;
        if body.providers == api::HostProviderCascade::Leave {
            if let Some(runtime) = runtime.filter(|row| row.host_id == host) {
                if providers::busy(&mut tx, *provider).await?.turns > 0 {
                    provider_blockers.push(blocked(
                        "provider_busy",
                        "A provider still has active turns",
                    ));
                }
                if jobs::changes::provider_hold(&mut tx, *provider)
                    .await?
                    .is_some()
                {
                    provider_blockers.push(blocked(
                        "provider_busy",
                        "Another operation holds a provider",
                    ));
                }
                provider_actions.push(ProviderAction::Leave {
                    provider: *provider,
                    runtime: runtime.id,
                });
            }
            continue;
        }
        match operation_access::review(
            &mut tx,
            principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: *provider,
            },
            Permission::ProviderManage,
        )
        .await
        {
            Ok(Some(grant)) => grants.push(grant),
            Ok(None) => {}
            Err(UpdateError::Denied) => {
                provider_blockers.push(blocked(
                    "access.denied",
                    "Its managers re-create this provider",
                ));
                continue;
            }
            Err(error) => return Err(refused(error)),
        }
        let template = runtime
            .as_ref()
            .map(|runtime| runtime.template_id)
            .unwrap_or(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID);
        let template_row = hosts::template(&mut tx, template)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let candidates =
            provider_provision::candidates(&mut tx, &state, &template_row, body.host_id, ctx.now)
                .await
                .map_err(ApiError::internal)?;
        match provider_provision::choose(&candidates, body.host_id) {
            Err(error) => provider_blockers.push(blocked(&error.code, error.message)),
            Ok(destination) => {
                let destination = destination.host.id;
                add_grant(
                    &mut tx,
                    principal,
                    &mut grants,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: destination,
                    },
                    Permission::HostUse,
                )
                .await?;
                add_grant(
                    &mut tx,
                    principal,
                    &mut grants,
                    ObjectRef {
                        kind: ObjectKind::Template,
                        id: template,
                    },
                    Permission::TemplateUse,
                )
                .await?;
                operation_access::accept(
                    &mut tx,
                    principal,
                    &grants,
                    &orch_db::audit::Origin::default(),
                )
                .await
                .map_err(refused)?;
                if runtime.is_some() {
                    sqlx::query("UPDATE providers SET runtime_config=runtime_config||jsonb_build_object('runtime.host_id',$2::uuid::text) WHERE id=$1").bind(provider).bind(destination).execute(&mut *tx).await.map_err(DbError::from)?;
                    match ctx
                        .app
                        .engine
                        .draft_update(
                            &mut tx,
                            principal,
                            Target::Provider(*provider),
                            orch_api_contract::environment::UpdatePreflightReason::Recreate,
                            None,
                        )
                        .await
                    {
                        Ok(draft) => {
                            provider_blockers.extend(draft.blockers.clone());
                            if draft.blockers.is_empty() {
                                provider_actions.push(ProviderAction::Replace {
                                    recipe: Box::new(draft.recipe),
                                    host: destination,
                                });
                            }
                        }
                        Err(UpdateError::Blocked(blockers)) => provider_blockers.extend(blockers),
                        Err(error) => return Err(refused(error)),
                    }
                } else {
                    sqlx::query("UPDATE providers SET runtime_config=runtime_config||jsonb_build_object('runtime.host_id',$2::uuid::text) WHERE id=$1").bind(provider).bind(destination).execute(&mut *tx).await.map_err(DbError::from)?;
                    match ctx
                        .app
                        .engine
                        .draft_provider_recovery(&mut tx, principal, *provider)
                        .await
                    {
                        Ok(draft) => {
                            grants.extend(draft.grants);
                            provider_actions.push(ProviderAction::Make {
                                recipe: Box::new(draft.recipe),
                            });
                        }
                        Err(UpdateError::Blocked(blockers)) => provider_blockers.extend(blockers),
                        Err(UpdateError::Denied) => provider_blockers.push(blocked(
                            "access.denied",
                            "The caller needs their own use of the provider's destination",
                        )),
                        Err(error) => return Err(refused(error)),
                    }
                }
            }
        }
    }
    rows.push(api::HostCascadeRow {
        kind: "provider_runtimes".into(),
        count: inventory.providers.len() as i64,
        blockers: provider_blockers,
    });
    for row in &mut rows {
        row.count = facts
            .counts
            .iter()
            .find(|(kind, _)| kind == &row.kind)
            .map_or(0, |(_, count)| *count);
    }
    let kept_until = data
        .rows
        .iter()
        .filter(|row| row.selected == api::HostDataFate::Wait)
        .filter_map(|row| row.until)
        .max();
    let mut erase_blockers = Vec::new();
    if data.custody.erase || physical {
        if inventory_measurement
            .as_ref()
            .is_some_and(|count| count.measured_at.is_none())
        {
            erase_blockers.push(blocked(
                "measurement_pending",
                "Waiting for the node's owned-object inventory",
            ));
        } else if facts.native.is_none() {
            erase_blockers.push(blocked(
                "inventory_unrecorded",
                "The node has not reported its owned objects",
            ));
        } else if facts
            .native
            .as_ref()
            .is_some_and(|inventory| !inventory.unavailable.is_empty())
        {
            erase_blockers.push(blocked(
                "inventory_incomplete",
                "The node cannot report every owned object",
            ));
        }
    }
    let host_ref = orch_api_contract::resources::HostRef {
        id: host,
        name: host_row.name.clone(),
        address: String::new(),
    };
    let objects = facts
        .native
        .as_ref()
        .map(|inventory| {
            orch_engine::storage::node_records(&host_ref, inventory, 0, facts.inventory.online)
                .into_iter()
                .filter(|row| {
                    !matches!(
                        row.kind,
                        orch_api_contract::storage::StorageKind::Volume
                            | orch_api_contract::storage::StorageKind::KeptLayer
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    grants.sort_by_key(|grant| {
        (
            grant.object.kind.as_str(),
            grant.object.id,
            grant.permission.as_str(),
        )
    });
    grants.dedup();
    let mut audience = notifications::operation_audience(
        &mut tx,
        principal,
        &inventory
            .environments
            .iter()
            .map(|env| env.id)
            .collect::<Vec<_>>(),
        &inventory.providers,
    )
    .await
    .map_err(ApiError::internal)?;
    let mut affected_resources = vec![(
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        Permission::HostManage,
    )];
    affected_resources.extend(inventory.mounts.iter().map(|id| {
        (
            ObjectRef {
                kind: ObjectKind::Mount,
                id: *id,
            },
            Permission::MountManage,
        )
    }));
    let servers: Vec<Id> = sqlx::query_scalar(
        "SELECT DISTINCT server_id FROM mcp_server_offerings WHERE machine_id=$1 AND state<>'lost'",
    )
    .bind(host_row.machine_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(DbError::from)?;
    affected_resources.extend(servers.into_iter().map(|id| {
        (
            ObjectRef {
                kind: ObjectKind::McpServer,
                id,
            },
            Permission::McpManage,
        )
    }));
    notifications::extend_operation_audience(
        &mut tx,
        principal,
        &mut audience,
        &affected_resources,
    )
    .await
    .map_err(ApiError::internal)?;
    let mut plan = api::HostDeletePlan {
        job: None,
        notification: api::OperationNotificationPlan {
            preview_id: Some(id),
            people: audience.len() as i64,
            default_on: notifications::operation_default(
                &mut tx,
                host_row
                    .home_id
                    .map_or(orch_settings::Place::System, orch_settings::Place::Node),
                !inventory.threads.is_empty() || !inventory.providers.is_empty(),
            )
            .await
            .map_err(ApiError::internal)?,
        },
        admin_grants: grants,
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        name: host_row.name,
        online: inventory.online,
        executable: rows.iter().all(|row| row.blockers.is_empty()),
        rows,
        recovery: recovery_rows,
        others: facts.pending_managers(&managed),
        volumes: volume_names,
        kept_until,
        data: data.rows.clone(),
        objects,
        erase: data.custody.erase,
        erase_blockers,
        inventory_measurement,
    };
    tx.rollback().await.map_err(DbError::from)?;
    for (index, prepared) in prepared_recoveries {
        match ctx.app.engine.measure_recovery(prepared).await {
            Ok(draft) => {
                let row = &mut plan.recovery[index];
                row.host = Some(draft.host);
                row.files = draft.recipe.listing_available.then_some(draft.files);
                row.listing_available = draft.recipe.listing_available;
                row.restored = !draft.recipe.restored.is_empty();
                row.restored_bytes = draft
                    .recipe
                    .restored
                    .values()
                    .try_fold(0_i64, |sum, bytes| {
                        bytes.map(|bytes| sum.saturating_add(bytes))
                    });
                environment_actions.push(EnvironmentAction::Recreate {
                    recipe: Box::new(draft.recipe),
                });
            }
            Err(UpdateError::Blocked(blockers)) => {
                plan.recovery[index].blockers = blockers.clone();
                if let Some(row) = plan.rows.iter_mut().find(|row| row.kind == "environments") {
                    row.blockers.extend(blockers);
                }
            }
            Err(error) => return Err(refused(error)),
        }
    }
    plan.executable = plan.rows.iter().all(|row| row.blockers.is_empty())
        && plan.data.iter().all(|row| row.blockers.is_empty())
        && plan.erase_blockers.is_empty();
    Ok(Stored {
        host,
        fingerprint,
        body: body.clone(),
        plan,
        environments: environment_actions,
        providers: provider_actions,
        volumes: vec![],
        audience,
        data: Some(data),
    })
}

async fn preflight(
    ctx: Ctx,
    (): (),
    body: api::HostDeletePreflightBody,
) -> ApiResult<api::HostDeletePlan> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    drop(conn);
    let id = new_id();
    let stored = build(&ctx, &principal, ctx.id("id")?, id, &body).await?;
    let plan = stored.plan.clone();
    let (_, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    operation_access::review(
        &mut tx,
        &principal,
        ObjectRef {
            kind: ObjectKind::Host,
            id: stored.host,
        },
        Permission::HostManage,
    )
    .await
    .map_err(refused)?;
    jobs::forks::save_preflight(
        &mut tx,
        &jobs::forks::PreflightRow {
            id,
            kind: "host_delete".into(),
            node_id: None,
            environment_id: None,
            created_by: principal.user(),
            plan: serde_json::to_value(&stored).map_err(ApiError::internal)?,
            expires_at: plan.expires_at.get(),
            created_at: ctx.now,
        },
    )
    .await?;
    orch_db::queries::notifications::operations::preview(
        &mut tx,
        id,
        &stored
            .audience
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(plan)
}

async fn create(ctx: Ctx, (): (), body: api::HostDeleteCreateBody) -> ApiResult<Job> {
    let host = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let preflight = jobs::forks::preflight(&mut conn, body.preflight_id)
        .await?
        .filter(|row| {
            row.kind == "host_delete"
                && row.created_by == principal.user()
                && row.expires_at > ctx.now
        })
        .ok_or_else(ApiError::not_found)?;
    drop(conn);
    let stored: Stored =
        serde_json::from_value(preflight.plan.clone()).map_err(ApiError::internal)?;
    if stored.host != host || body.confirm != stored.plan.name {
        return Err(ApiError::invalid("confirm", "type the host's exact name"));
    }
    let mut fresh = build(&ctx, &principal, host, body.preflight_id, &stored.body).await?;
    for (old, new) in stored.environments.iter().zip(&mut fresh.environments) {
        if let (
            EnvironmentAction::Move { recipe: old, .. },
            EnvironmentAction::Move { recipe: new, .. },
        ) = (old, new)
        {
            retain_copy_names(&old.mount_decisions, &mut new.mount_decisions);
        }
    }
    if !fresh.plan.executable {
        return Err(
            ApiError::new(ErrorCode::PlanBlocked, "A selected cascade cannot run")
                .with("rows", fresh.plan.rows)
                .with("data", fresh.plan.data)
                .with("erase", fresh.plan.erase_blockers),
        );
    }
    if fresh.fingerprint != stored.fingerprint
        || serde_json::to_value(&fresh.environments).map_err(ApiError::internal)?
            != serde_json::to_value(&stored.environments).map_err(ApiError::internal)?
        || serde_json::to_value(&fresh.providers).map_err(ApiError::internal)?
            != serde_json::to_value(&stored.providers).map_err(ApiError::internal)?
        || fresh.volumes != stored.volumes
        || !stored
            .data
            .as_ref()
            .zip(fresh.data.as_ref())
            .is_some_and(|(stored, fresh)| stored.unchanged(fresh))
        || fresh.audience != stored.audience
        || fresh
            .plan
            .admin_grants
            .iter()
            .any(|grant| !stored.plan.admin_grants.contains(grant))
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The host's dependencies changed; review its deletion again",
        ));
    }
    for row in stored.plan.data.iter().filter(|row| {
        matches!(
            row.selected,
            api::HostDataFate::Delete | api::HostDataFate::Purge
        )
    }) {
        let name = row.label.as_ref().or(row.resource.name.as_ref());
        if !body
            .data_confirmations
            .iter()
            .any(|typed| typed.id == row.id && Some(&typed.confirm) == name)
            && !(row.kind == "mount"
                && body
                    .volumes
                    .iter()
                    .any(|typed| typed.mount_id == row.resource.id && Some(&typed.confirm) == name))
        {
            return Err(ApiError::invalid(
                "data_confirmations",
                "type the exact name shown for each selected data deletion",
            ));
        }
    }
    let mut portable_drafts = std::collections::BTreeMap::new();
    for action in &stored.environments {
        if let EnvironmentAction::Portable { recipe, request } = action {
            let draft = ctx
                .app
                .engine
                .draft_machine_move(&principal, recipe.environment, request, None)
                .await
                .map_err(refused)?;
            if !recipe.same_choice(&draft.recipe) {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "The machine move changed",
                ));
            }
            portable_drafts.insert(recipe.environment, draft);
        }
    }
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    orch_db::queries::resources::lock_resource(&mut tx, "host", host).await?;
    let current = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if current.plan != preflight.plan {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The deletion plan changed",
        ));
    }
    let inventory = loss::inventory(&mut tx, "host", host)
        .await?
        .ok_or_else(ApiError::not_found)?;
    for environment in &inventory.environments {
        envs::lock_environment(&mut tx, environment.id).await?;
    }
    if loss::inventory(&mut tx, "host", host)
        .await?
        .ok_or_else(ApiError::not_found)?
        .fingerprint()?
        != stored.fingerprint
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The host's inventory changed",
        ));
    }
    operation_access::accept(
        &mut tx,
        &principal,
        &stored.plan.admin_grants,
        &orch_db::audit::Origin::default(),
    )
    .await
    .map_err(refused)?;
    let host_row = hosts::host(&mut tx, host)
        .await?
        .ok_or_else(ApiError::not_found)?;
    sqlx::query("UPDATE hosts SET schedulable=false,revision=revision+1,updated_at=$2 WHERE id=$1")
        .bind(host)
        .bind(ctx.now)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let parent = new_id();
    let batch = new_id();
    let mut children = Vec::new();
    let notification = notifications::NotificationOperation {
        id: parent,
        kind: "host_delete".into(),
        name: host_row.name.clone(),
        notify: body.notify,
        path: format!("/resources/hosts/{host}"),
        subject: Some(("host".into(), host)),
        recipients: stored.audience,
    };
    // All child acceptance runs under the one explicit notification choice.
    notifications::operation_scope(Some(parent),async {
        for action in stored.environments {
            let child=match action {
                EnvironmentAction::Recreate{recipe}=>Some(recovery::queue(&ctx.app.engine,&mut tx,&recipe,Some(batch),ctx.now).await.map_err(refused)?),
                EnvironmentAction::Move{recipe,request}=>{let mut draft=ctx.app.engine.draft_move(&mut tx,&principal,recipe.replacement.target.id(),&request,None).await.map_err(refused)?;retain_copy_names(&recipe.mount_decisions,&mut draft.recipe.mount_decisions);if serde_json::to_value(&draft.recipe).map_err(ApiError::internal)?!=serde_json::to_value(&recipe).map_err(ApiError::internal)? {return Err(ApiError::new(ErrorCode::ConflictStaleRevision,"The reviewed environment move changed"));}Some(environment_move::queue(&mut tx,draft,Some(batch),ctx.now).await.map_err(refused)?)},
                EnvironmentAction::Portable{recipe,..}=>{let draft=portable_drafts.remove(&recipe.environment).ok_or_else(||ApiError::internal("The reviewed machine move is missing"))?;Some(machine_move::queue(&mut tx,&principal,draft,Some(batch),ctx.now).await.map_err(refused)?)},
                EnvironmentAction::Remove{id,lost,..}=>{if lost{ctx.app.engine.remove_lost_environment_in(&mut tx,id,principal.user(),ctx.now).await.map_err(ApiError::internal)?;None}else{ctx.app.engine.remove_bindings_in(&mut tx,id,principal.user(),Time::new(ctx.now)).await.map_err(ApiError::internal)?;ctx.app.engine.schedule_removal_in(&mut tx,id,ctx.now,principal.user(),Time::new(ctx.now)).await.map_err(crate::routes::environments::removal_schedule_error)?;jobs::open_of_environment(&mut tx,id).await?.into_iter().find(|job|job.kind==jobs::kind::ENVIRONMENT_REMOVE)}},
            };if let Some(child)=child{children.push(child.id);}
        }
        for action in stored.providers {
            let child=match action {
                ProviderAction::Leave{provider,runtime}=>{
                    let queued=provider_provision::queue_removal_with_options(&mut tx,ctx.now,provider,runtime,false,Some(principal.user())).await?;
                    envs::retire_runtime(&mut tx,runtime,Some("Host deletion"),ctx.now).await?;
                    sqlx::query("UPDATE providers SET runtime_unavailable='Its host was removed',status='offline',status_detail='Its host was removed',machine_id=NULL,revision=revision+1,updated_at=$2 WHERE id=$1").bind(provider).bind(ctx.now).execute(&mut *tx).await.map_err(DbError::from)?;
                    match queued{jobs::provider_jobs::Queued::Made(job)=>*job,jobs::provider_jobs::Queued::Running(_)=>return Err(ApiError::new(ErrorCode::PlanBlocked,"Another operation holds a provider")),jobs::provider_jobs::Queued::NoProvider=>return Err(ApiError::not_found())}
                },
                ProviderAction::Replace{recipe,host}=>{sqlx::query("UPDATE providers SET runtime_config=runtime_config||jsonb_build_object('runtime.host_id',$2::uuid::text) WHERE id=$1").bind(recipe.target.id()).bind(host).execute(&mut *tx).await.map_err(DbError::from)?;let draft=ctx.app.engine.draft_update(&mut tx,&principal,recipe.target,orch_api_contract::environment::UpdatePreflightReason::Recreate,None).await.map_err(refused)?;recreate::queue(&mut tx,draft,Some(batch),ctx.now).await.map_err(refused)?},
                ProviderAction::Make{recipe}=>orch_engine::provider_recovery::queue(&ctx.app.engine,&mut tx,&principal,&recipe,ctx.now).await.map_err(refused)?,
            };children.push(child.id);
        }
        if let Some(data)=&stored.data {children.extend(data::queue(&ctx,&mut tx,&principal,data).await?);}
        Ok::<(),ApiError>(())
    }).await?;
    let recipe = operation::Recipe {
        host,
        machine: host_row.machine_id,
        by: principal.user(),
        token: principal.caller().grant().and_then(|grant| grant.token()),
        schedulable: host_row.schedulable,
        children: children.clone(),
        delete_volumes: stored.volumes,
        admin_override: !stored.plan.admin_grants.is_empty(),
        custody: stored.data.map(|data| data.custody),
    };
    let row = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: parent,
            kind: operation::KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            plan: &json!({"identity":crate::routes::environments::retained::identity(&ctx)?,"origin":orch_db::audit::current_origin(),"mount_decisions":recipe.custody.as_ref().map(|custody|&custody.copies),"cascade":recipe}),
            progress: &serde_json::to_value(orch_engine::jobs::runner::initial_progress(
                &operation::PHASES,
            ))
            .map_err(ApiError::internal)?,
            created_by: Some(principal.user()),
        },
        ctx.now,
    )
    .await?;
    children.push(parent);
    notifications::attach_operation(&mut tx, &children, &notification, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    notifications::notify_operation(&mut tx, &notification, "running", ctx.now)
        .await
        .map_err(ApiError::internal)?;
    jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    let result = crate::routes::environments::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(result)
}
