//! Native snapshot windows bind only selected custody and metadata sources before rendering.

use super::after_of;

use super::cursor_of;

use super::environment_rows;

use super::require_where_it_runs;

use super::storage;

use super::views;

use crate::key_operations::native;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use orch_access_contract::decisions::Decision;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::workspace::Snapshot;

use orch_api_contract::workspace_sources as sources;

use orch_api_contract::workspace_sources::SnapshotListSource as Role;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::resources;

use orch_db::Conn;

use orch_engine::archive::snapshot_authority::Source;

use orch_engine::operation_scope::targets;

use orch_types::Id;

use std::collections::BTreeSet;

use std::collections::HashMap;
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

pub(super) struct ThreadSnapshots;

pub(super) struct Window {
    sources: Vec<Source>,
    continuation: Option<Source>,
    next_cursor: Option<String>,
}

impl Window {
    fn witnesses(&self) -> impl Iterator<Item = &Source> {
        self.sources.iter().chain(self.continuation.iter())
    }
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the selected Snapshot source is no longer readable",
    )
}

pub(super) async fn identities(
    conn: &mut Conn,
    ids: &[Id],
) -> ApiResult<HashMap<Id, KeyResolvedTarget>> {
    let mut graph = orch_authz::engine::Graph::default();
    let rows = resources::read_into(conn, &mut graph, ids)
        .await
        .map_err(ApiError::internal)?;
    Ok(rows
        .into_iter()
        .filter(|row| row.kind == ResourceKind::Environment)
        .filter_map(|row| {
            targets::from_resource_identity(&graph, &row).map(|target| (row.id, target))
        })
        .collect())
}

async fn facts(conn: &mut Conn, window: &Window) -> ApiResult<Vec<native::Facts<Role>>> {
    let governors: BTreeSet<_> = window
        .witnesses()
        .map(Source::governing_environment)
        .collect();
    let origins: BTreeSet<_> = window
        .witnesses()
        .map(|source| source.row.environment_id)
        .collect();
    let ids: Vec<_> = governors.union(&origins).copied().collect();
    let targets = identities(conn, &ids).await?;
    let groups = |ids: BTreeSet<Id>| -> ApiResult<_> {
        let selected = ids
            .into_iter()
            .map(|id| targets.get(&id).cloned().ok_or_else(ApiError::not_found))
            .collect::<ApiResult<Vec<_>>>()?;
        Ok(if selected.is_empty() {
            Vec::new()
        } else {
            vec![(ENVIRONMENT, selected)]
        })
    };
    Ok(vec![
        native::Facts::collection(Role::Governors, groups(governors)?),
        native::Facts::collection(Role::Origins, groups(origins)?),
    ])
}

async fn permissions(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<HashMap<Id, bool>> {
    let mut permission = HashMap::new();
    for id in ids {
        if permission.contains_key(id) {
            continue;
        }
        let allowed = match resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: *id,
            },
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
        permission.insert(*id, allowed);
    }
    Ok(permission)
}

async fn ordinary_selection(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    current: Option<Id>,
    recorded: &[Id],
) -> ApiResult<(bool, Vec<Id>)> {
    let mut governors = recorded.to_vec();
    governors.extend(current);
    let allowed = permissions(conn, principal, &governors).await?;
    let manual = current.is_some_and(|id| allowed.get(&id) == Some(&true));
    let recorded = recorded
        .iter()
        .copied()
        .filter(|id| allowed.get(id) == Some(&true))
        .collect::<Vec<_>>();
    if !manual && recorded.is_empty() {
        require_where_it_runs(conn, principal, thread, Permission::FilesView).await?;
    }
    Ok((manual, recorded))
}

async fn require_sources(conn: &mut Conn, principal: &Principal, window: &Window) -> ApiResult<()> {
    let ids = window
        .witnesses()
        .map(Source::governing_environment)
        .collect::<Vec<_>>();
    let allowed = permissions(conn, principal, &ids).await?;
    if ids.iter().any(|id| allowed.get(id) != Some(&true)) {
        return Err(refused());
    }
    Ok(())
}

impl native::Selected for ThreadSnapshots {
    type Source = Role;
    type Query = PageQuery;
    type Value = Window;
    type Reply = Page<Snapshot>;
    const DECLARATION: &'static Declaration<Role> = &sources::THREAD_SNAPSHOT_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Window, Vec<native::Facts<Role>>)>> {
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
                let ids = governors
                    .iter()
                    .chain(&origins)
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let targets = identities(conn, &ids).await?;
                for id in &governors {
                    if let Some(target) = targets.get(id)
                        && native::permits(
                            &selection,
                            Self::DECLARATION,
                            caller,
                            &Role::Governors,
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
                            &Role::Origins,
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
                            Role::Governors,
                            vec![(
                                ENVIRONMENT,
                                vec![targets.get(id).cloned().ok_or_else(ApiError::not_found)?],
                            )],
                        )],
                    )?
                } else {
                    // The actual native selector, with no eligible custodian, proves emptiness
                    // before any checked-empty collection is bound for ordinary preparation.
                    let selected = storage::visible_snapshots_of_thread_from(
                        conn,
                        thread,
                        after,
                        i64::from(query.limit()) + 1,
                        false,
                        &[],
                        Some(&permitted_origins),
                    )
                    .await?;
                    if !selected.is_empty() {
                        return Err(refused());
                    }
                    let empty = Window {
                        sources: Vec::new(),
                        continuation: None,
                        next_cursor: None,
                    };
                    native::bind(ctx, Self::DECLARATION, &facts(conn, &empty).await?)?
                };
                native::preparation_principal(ctx, conn, caller, &known).await?
            } else {
                native::preparation_principal(ctx, conn, caller, &selection).await?
            };
            let normal = ordinary_selection(conn, &principal, thread, current, &recorded).await;
            let (ordinary_manual, ordinary_recorded) = if native::modern(caller) {
                normal.map_err(authorization::problem_without_content)?
            } else {
                normal?
            };
            let manual = ordinary_manual
                && (!native::modern(caller)
                    || current.is_some_and(|id| permitted_governors.contains(&id)));
            let recorded = ordinary_recorded
                .into_iter()
                .filter(|id| !native::modern(caller) || permitted_governors.contains(id))
                .collect::<Vec<_>>();
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
            let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
            let continuation = if found.len() > limit {
                found.pop()
            } else {
                None
            };
            let next_cursor = found
                .last()
                .filter(|_| continuation.is_some())
                .map(cursor_of);
            let sources =
                Source::from_thread_rows(thread, current, found).ok_or_else(ApiError::not_found)?;
            let continuation = continuation
                .map(|row| {
                    Source::from_thread_rows(thread, current, vec![row])
                        .and_then(|mut rows| rows.pop())
                        .ok_or_else(ApiError::not_found)
                })
                .transpose()?;
            let window = Window {
                sources,
                continuation,
                next_cursor,
            };
            let facts = facts(conn, &window).await?;
            Ok((window, facts))
        })
    }

    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a PageQuery,
        opened: &'a Window,
    ) -> BoxFuture<'a, ApiResult<(Window, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            let current = environment_rows::thread_place(conn, thread)
                .await?
                .and_then(|place| place.environment_id);
            let ids = opened
                .witnesses()
                .map(|source| source.row.id)
                .collect::<Vec<_>>();
            let rows = storage::snapshots_by_ids(conn, &ids).await?;
            if rows.len() != ids.len() {
                return Err(refused());
            }
            let mut by_id = rows
                .into_iter()
                .map(|row| (row.id, row))
                .collect::<HashMap<_, _>>();
            let ordered = ids
                .into_iter()
                .map(|id| by_id.remove(&id).ok_or_else(refused))
                .collect::<ApiResult<Vec<_>>>()?;
            let mut sources =
                Source::from_thread_rows(thread, current, ordered).ok_or_else(refused)?;
            let continuation = if opened.continuation.is_some() {
                sources.pop()
            } else {
                None
            };
            let window = Window {
                sources,
                continuation,
                next_cursor: opened.next_cursor.clone(),
            };
            let facts = facts(conn, &window).await?;
            let plan = native::bind(ctx, Self::DECLARATION, &facts)?;
            let actor = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let recorded = orch_db::queries::source::snapshot_environments(conn, thread).await?;
            ordinary_selection(conn, &actor, thread, current, &recorded).await?;
            require_sources(conn, &actor, &window).await?;
            Ok((window, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }

    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        window: &'a Window,
    ) -> BoxFuture<'a, ApiResult<Page<Snapshot>>> {
        Box::pin(async move {
            require_sources(conn, principal, window).await?;
            Ok(Page {
                items: views(
                    conn,
                    window
                        .sources
                        .iter()
                        .map(|source| source.row.clone())
                        .collect(),
                    principal.now(),
                )
                .await?,
                next_cursor: window.next_cursor.clone(),
            })
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a PageQuery,
        opened: &'a Window,
        fresh: &'a Window,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            require_sources(conn, principal, fresh).await?;
            Ok(opened.sources.len() == fresh.sources.len()
                && opened.continuation.is_some() == fresh.continuation.is_some()
                && opened
                    .witnesses()
                    .zip(fresh.witnesses())
                    .all(|(old, current)| {
                        old.witness().matches(&current.row)
                            && old.governing_environment() == current.governing_environment()
                    }))
        })
    }
}
