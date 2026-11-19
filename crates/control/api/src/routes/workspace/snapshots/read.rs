//! One native Snapshot window with current Files custody and separate metadata origins.

use super::{after_of, cursor_of, environment_rows, require_where_it_runs, storage, views};
use crate::key_operations::native;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::workspace::Snapshot;
use orch_api_contract::workspace_sources::{self as sources, SnapshotListSource};
use orch_authz::{Caller, Policy, Principal, resources};
use orch_db::{Conn, DbError};
use orch_engine::archive::snapshot_authority::Source;
use orch_engine::operation_scope::targets;
use orch_types::Id;
use std::collections::{BTreeSet, HashMap};

#[cfg(test)]
mod tests;

const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

pub(super) struct ThreadSnapshots;

#[derive(Clone)]
pub(super) struct Window {
    sources: Vec<Source>,
    next_cursor: Option<String>,
}

pub(super) struct Rendered {
    ctx: Ctx,
    accepted: Caller,
    sources: Vec<Source>,
    page: Page<Snapshot>,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the Snapshot list source is no longer admitted",
    )
}

async fn identities(conn: &mut Conn, ids: &[Id]) -> ApiResult<HashMap<Id, KeyResolvedTarget>> {
    let mut graph = orch_authz::engine::Graph::default();
    let rows = resources::read_into(conn, &mut graph, ids)
        .await
        .map_err(ApiError::internal)?;
    Ok(rows
        .into_iter()
        .filter(|row| row.kind == orch_access_contract::permissions::ResourceKind::Environment)
        .filter_map(|row| {
            targets::from_resource_identity(&graph, &row).map(|target| (row.id, target))
        })
        .collect())
}

async fn facts(
    conn: &mut Conn,
    sources: &[Source],
) -> ApiResult<Vec<native::Facts<SnapshotListSource>>> {
    let governors: BTreeSet<_> = sources.iter().map(Source::governing_environment).collect();
    let origins: BTreeSet<_> = sources
        .iter()
        .map(|source| source.row.environment_id)
        .collect();
    let ids: BTreeSet<_> = governors.union(&origins).copied().collect();
    let targets = identities(conn, &ids.into_iter().collect::<Vec<_>>()).await?;
    let group = |ids: BTreeSet<Id>| -> ApiResult<_> {
        let targets = ids
            .into_iter()
            .map(|id| targets.get(&id).cloned().ok_or_else(ApiError::not_found))
            .collect::<ApiResult<Vec<_>>>()?;
        Ok(if targets.is_empty() {
            Vec::new()
        } else {
            vec![(ENVIRONMENT, targets)]
        })
    };
    Ok(vec![
        native::Facts::collection(SnapshotListSource::Governors, group(governors)?),
        native::Facts::collection(SnapshotListSource::Origins, group(origins)?),
    ])
}

async fn permissions(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<HashMap<Id, bool>> {
    let mut permission = HashMap::new();
    for environment in ids {
        if permission.contains_key(environment) {
            continue;
        }
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: *environment,
        };
        let allowed = match resources::check(
            conn,
            principal,
            object,
            Permission::FilesView,
            Policy::default(),
        )
        .await
        {
            Ok(Decision::Allowed { .. }) => true,
            Ok(_) | Err(orch_authz::Error::NotFound(_) | orch_authz::Error::Suspended { .. }) => {
                false
            }
            Err(error) => return Err(ApiError::internal(error)),
        };
        permission.insert(*environment, allowed);
    }
    Ok(permission)
}

async fn require_sources(
    conn: &mut Conn,
    principal: &Principal,
    sources: &[Source],
) -> ApiResult<()> {
    let ids: Vec<_> = sources.iter().map(Source::governing_environment).collect();
    let permission = permissions(conn, principal, &ids).await?;
    if ids.iter().any(|id| permission.get(id) != Some(&true)) {
        return Err(refused());
    }
    Ok(())
}

impl native::Selected for ThreadSnapshots {
    type Source = SnapshotListSource;
    type Query = PageQuery;
    type Value = Window;
    type Reply = Rendered;
    const DECLARATION: &'static Declaration<SnapshotListSource> = &sources::THREAD_SNAPSHOT_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Window, Vec<native::Facts<SnapshotListSource>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            let after = query.cursor.as_deref().map(after_of).transpose()?;
            let current = environment_rows::thread_place(conn, thread)
                .await?
                .and_then(|place| place.environment_id);
            let recorded = orch_db::queries::source::snapshot_environments(conn, thread).await?;
            let origins = storage::snapshot_origins_of_thread(conn, thread).await?;
            let mut governors = recorded.clone();
            governors.extend(current);
            governors.sort();
            governors.dedup();
            let selection = native::bind_known(ctx, Self::DECLARATION, &[])?;
            let mut permitted_governors = BTreeSet::new();
            let mut permitted_origins = Vec::new();
            let principal = if native::modern(caller) {
                let all: BTreeSet<_> = governors.iter().chain(&origins).copied().collect();
                let targets = identities(conn, &all.into_iter().collect::<Vec<_>>()).await?;
                for id in &governors {
                    if let Some(target) = targets.get(id)
                        && native::permits(
                            &selection,
                            Self::DECLARATION,
                            caller,
                            &SnapshotListSource::Governors,
                            ENVIRONMENT,
                            target,
                        )?
                    {
                        permitted_governors.insert(*id);
                    }
                }
                for id in &origins {
                    if let Some(target) = targets.get(id)
                        && native::permits(
                            &selection,
                            Self::DECLARATION,
                            caller,
                            &SnapshotListSource::Origins,
                            ENVIRONMENT,
                            target,
                        )?
                    {
                        permitted_origins.push(*id);
                    }
                }
                let known = if let Some(id) = permitted_governors.first() {
                    native::bind_known(
                        ctx,
                        Self::DECLARATION,
                        &[native::Facts::collection(
                            SnapshotListSource::Governors,
                            vec![(
                                ENVIRONMENT,
                                vec![targets.get(id).cloned().ok_or_else(ApiError::not_found)?],
                            )],
                        )],
                    )?
                } else {
                    native::bind(ctx, Self::DECLARATION, &facts(conn, &[]).await?)?
                };
                native::preparation_principal(ctx, conn, caller, &known).await?
            } else {
                native::preparation_principal(ctx, conn, caller, &selection).await?
            };
            let permission = permissions(conn, &principal, &governors).await?;
            let ordinary_manual =
                current.is_some_and(|environment| permission.get(&environment) == Some(&true));
            let ordinary_recorded: Vec<_> = recorded
                .into_iter()
                .filter(|id| permission.get(id) == Some(&true))
                .collect();
            if !ordinary_manual && ordinary_recorded.is_empty() {
                let gate =
                    require_where_it_runs(conn, &principal, thread, Permission::FilesView).await;
                if native::modern(caller) {
                    gate.map_err(authorization::problem_without_content)?;
                } else {
                    gate?;
                }
            }
            let manual = ordinary_manual
                && (!native::modern(caller)
                    || current.is_some_and(|id| permitted_governors.contains(&id)));
            let recorded: Vec<_> = ordinary_recorded
                .into_iter()
                .filter(|id| !native::modern(caller) || permitted_governors.contains(id))
                .collect();
            let mut found = storage::visible_snapshots_of_thread_from(
                conn,
                thread,
                after,
                i64::from(query.limit()) + 1,
                manual,
                &recorded,
                native::modern(caller).then_some(permitted_origins.as_slice()),
            )
            .await?;
            let more = found.len() > usize::try_from(query.limit()).unwrap_or(usize::MAX);
            found.truncate(usize::try_from(query.limit()).unwrap_or(usize::MAX));
            let next_cursor = found.last().filter(|_| more).map(cursor_of);
            let sources =
                Source::from_thread_rows(thread, current, found).ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, &sources).await?;
            Ok((
                Window {
                    sources,
                    next_cursor,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // This route's ordinary gate is its per-entry Files selector.
        Box::pin(async move {
            let _ = ctx.user()?;
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Window,
    ) -> BoxFuture<'a, ApiResult<Rendered>> {
        Box::pin(async move {
            require_sources(conn, principal, &value.sources).await?;
            let rows = value
                .sources
                .iter()
                .map(|source| source.row.clone())
                .collect();
            Ok(Rendered {
                ctx: ctx.clone(),
                accepted: principal.caller().clone(),
                sources: value.sources.clone(),
                page: Page {
                    items: views(conn, rows, principal.now()).await?,
                    next_cursor: value.next_cursor.clone(),
                },
            })
        })
    }
}

/// Keep the actual held rows through the existing current-origin guard, without lending a replacement window to its old body.
pub(super) async fn list(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<Page<Snapshot>> {
    let held = native::selected_read::<ThreadSnapshots>(ctx, query, ()).await?;
    #[cfg(test)]
    tests::cut(&held.sources).await;
    let (mut tx, guard) = transaction_guard::begin(&held.ctx).await?;
    check_sources(&held.ctx, &mut tx, guard.principal(), &held.sources).await?;
    guard.check_deadline(&held.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(&held.ctx)?;
    crate::key_operations::publication::retain(
        &held.ctx,
        &held.accepted,
        held.sources,
        |ctx, conn, reader, sources| Box::pin(check_sources(ctx, conn, reader, sources)),
    )?;
    Ok(held.page)
}

async fn check_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    held: &[Source],
) -> ApiResult<()> {
    let thread = ctx.id("id")?;
    let current = environment_rows::thread_place(conn, thread)
        .await?
        .and_then(|place| place.environment_id);
    let ids: Vec<_> = held.iter().map(|source| source.row.id).collect();
    let rows = storage::snapshots_by_ids(conn, &ids).await?;
    if rows.len() != ids.len() {
        return Err(refused());
    }
    let fresh = Source::from_thread_rows(thread, current, rows).ok_or_else(refused)?;
    let by_id: HashMap<_, _> = fresh.iter().map(|source| (source.row.id, source)).collect();
    for source in held {
        if !by_id
            .get(&source.row.id)
            .is_some_and(|current| source.witness().matches(&current.row))
        {
            return Err(refused());
        }
    }
    let plan = native::bind(
        ctx,
        &sources::THREAD_SNAPSHOT_LIST,
        &facts(conn, &fresh).await?,
    )?;
    if native::modern(reader.caller()) {
        plan.clone().admit(reader.caller())?;
    }
    let current = native::preparation_principal(ctx, conn, reader.caller(), &plan).await?;
    require_sources(conn, &current, &fresh).await?;
    Ok(())
}
