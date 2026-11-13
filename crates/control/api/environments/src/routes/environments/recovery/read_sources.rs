//! Stored native batch selection, retaining each actual job and its ordinary read cut.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment::EnvironmentRef;

use orch_api_contract::environment_sources as sources;

use orch_api_contract::environment_sources::RecoveryReadSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use std::collections::HashMap;
pub(super) struct Recovered;

struct MemberRead {
    environment_id: Id,
    environment: EnvironmentRef,
    job: crate::routes::environments::jobs::PreparedRead,
}

pub(super) struct BatchRead {
    batch: Id,
    members: Vec<MemberRead>,
    names: HashMap<Id, String>,
    limited: bool,
}

fn collection(
    source: Source,
    kind: KeyTargetKind,
    targets: Vec<KeyResolvedTarget>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(kind, targets)]
        },
    )
}

impl native::Selected for Recovered {
    type Source = Source;
    type Query = ();
    type Value = BatchRead;
    type Reply = EnvironmentBatch;
    const DECLARATION: &'static Declaration<Source> = &sources::RECOVERY_GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(BatchRead, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let batch = ctx.id("batch_id")?;
            let rows = jobs::forks::of_batch(conn, batch).await?;
            if rows.is_empty() {
                return Err(ApiError::not_found());
            }
            let modern = native::modern(caller);
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let mut members = Vec::new();
            let mut selected = Vec::new();
            for row in rows {
                let environment = match row.environment_id {
                    Some(id) => Some(id),
                    None => match row.node_id {
                        Some(thread) => orch_db::queries::threads::thread(conn, thread)
                            .await?
                            .and_then(|row| row.environment_id),
                        None => None,
                    },
                };
                let Some(environment) = environment else {
                    continue;
                };
                let known = if modern {
                    let target = match targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: environment,
                        },
                    )
                    .await
                    {
                        Ok(target) => target,
                        Err(error)
                            if error.code() == orch_api_contract::common::ErrorCode::NotFound =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    if !native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Environments,
                        KeyTargetKind::Object(ObjectKind::Environment),
                        &target,
                    )? {
                        continue;
                    }
                    Some(target)
                } else {
                    None
                };
                let facts = known
                    .as_ref()
                    .map(|target| {
                        collection(
                            Source::Environments,
                            KeyTargetKind::Object(ObjectKind::Environment),
                            vec![target.clone()],
                        )
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                let partial = native::bind_known(ctx, Self::DECLARATION, &facts)?;
                let principal = native::preparation_principal(ctx, conn, caller, &partial).await?;
                // Preserve the original native Environment projection and skip behavior. The
                // stored row supplied authority before this projection; no DTO supplies a target.
                let Ok(view) = super::super::show(ctx, conn, &principal, environment).await else {
                    continue;
                };
                let job =
                    crate::routes::environments::jobs::prepare_read_for(conn, &principal, row.id)
                        .await?;
                members.push(MemberRead {
                    environment_id: environment,
                    environment: view.base,
                    job,
                });
                if let Some(target) = known {
                    selected.push(target);
                }
            }
            let mut facts = vec![collection(
                Source::Environments,
                KeyTargetKind::Object(ObjectKind::Environment),
                selected,
            )];
            let full = members
                .iter()
                .filter(|member| !member.job.progress_only())
                .map(|member| member.job.row().clone())
                .collect::<Vec<_>>();
            let mut names = if modern {
                crate::routes::environments::jobs::machine_name_sources(conn, &full).await?
            } else {
                HashMap::new()
            };
            if modern {
                let identities =
                    targets::machine_identities(conn, &names.keys().copied().collect::<Vec<_>>())
                        .await?;
                let mut allowed = HashMap::new();
                let mut selected = Vec::new();
                for (id, name) in names {
                    let target = identities.get(&id).ok_or_else(ApiError::not_found)?;
                    if native::permits_usage(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Machines,
                        FacetUsage::ResponseContent,
                        KeyTargetKind::Object(ObjectKind::Machine),
                        target,
                    )? {
                        allowed.insert(id, name);
                        selected.push(target.clone());
                    }
                }
                names = allowed;
                facts.push(collection(
                    Source::Machines,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    selected,
                ));
            }
            Ok((
                BatchRead {
                    batch,
                    members,
                    names,
                    limited: modern,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal(
                    "the native batch reader changed account",
                ));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a BatchRead,
    ) -> BoxFuture<'a, ApiResult<EnvironmentBatch>> {
        Box::pin(async move {
            if value.members.is_empty() {
                return Err(ApiError::not_found());
            }
            let mut jobs = Vec::new();
            for member in &value.members {
                member.job.require_current(conn, principal).await?;
                jobs.push(EnvironmentBatchJob {
                    environment: member.environment.clone(),
                    job: member
                        .job
                        .render(conn, value.limited.then_some(&value.names))
                        .await?,
                });
            }
            Ok(EnvironmentBatch {
                batch_id: value.batch,
                jobs,
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        opened: &'a BatchRead,
        fresh: &'a BatchRead,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let current: HashMap<_, _> = fresh
                .members
                .iter()
                .map(|member| (member.job.row().id, member))
                .collect();
            Ok(opened.batch == fresh.batch
                && opened.members.iter().all(|old| {
                    current.get(&old.job.row().id).is_some_and(|now| {
                        old.environment_id == now.environment_id
                            && old.job.preserves(&now.job)
                            && old
                                .job
                                .machine_source()
                                .filter(|machine| opened.names.contains_key(machine))
                                .is_none_or(|machine| {
                                    now.job.machine_source() == Some(machine)
                                        && fresh.names.contains_key(&machine)
                                })
                    })
                }))
        })
    }
}
