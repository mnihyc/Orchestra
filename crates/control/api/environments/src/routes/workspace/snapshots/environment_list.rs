//! A whole-workspace export remains a real snapshot in its environment's existing Files reader.

use super::after_of;

use super::cursor_of;

use super::read;

use super::snapshot_read;

use super::storage;

use super::views;

use crate::key_operations::native;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::workspace::Snapshot;

use orch_api_contract::workspace_sources::ENVIRONMENT_SNAPSHOT_LIST;

use orch_api_contract::workspace_sources::SnapshotListSource as Role;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_engine::archive::snapshot_authority::Source;
pub(super) struct Snapshots;

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

async fn root(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(Principal, Vec<native::Facts<Role>>)> {
    let id = ctx.id("id")?;
    let target = read::identities(conn, &[id])
        .await?
        .remove(&id)
        .ok_or_else(ApiError::not_found)?;
    let facts = vec![native::Facts::one(
        Role::Governors,
        KeyTargetKind::Object(ObjectKind::Environment),
        target,
    )];
    let plan = native::bind_known(ctx, &ENVIRONMENT_SNAPSHOT_LIST, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    Ok((principal, facts))
}

async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    authorization::require_resource(
        conn,
        principal,
        ResourceKind::Environment,
        ctx.id("id")?,
        Permission::FilesView,
        Policy::default(),
    )
    .await
}

async fn require_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    window: &Window,
) -> ApiResult<()> {
    require(ctx, conn, principal).await?;
    for source in window.witnesses() {
        if source.row.environment_id != ctx.id("id")? || source.row.thread_id.is_some() {
            return Err(ApiError::not_found());
        }
        snapshot_read::require(conn, principal, source, Permission::FilesView).await?;
    }
    Ok(())
}

async fn sources(conn: &mut Conn, rows: Vec<storage::SnapshotRow>) -> ApiResult<Vec<Source>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(
            Source::from_row(conn, row)
                .await?
                .ok_or_else(ApiError::not_found)?,
        );
    }
    Ok(out)
}

impl native::Selected for Snapshots {
    type Source = Role;
    type Query = PageQuery;
    type Value = Window;
    type Reply = Page<Snapshot>;
    const DECLARATION: &'static Declaration<Role> = &ENVIRONMENT_SNAPSHOT_LIST;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Window, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let (principal, facts) = root(ctx, conn, caller).await?;
            require(ctx, conn, &principal).await?;
            let after = query.cursor.as_deref().map(after_of).transpose()?;
            let administrative =
                orch_engine::archive::custody_inspection(&principal, None).is_some();
            let mut rows = storage::snapshots_of_environment(
                conn,
                ctx.id("id")?,
                principal.user(),
                administrative,
                after,
                i64::from(query.limit()) + 1,
            )
            .await?;
            let continuation =
                if rows.len() > usize::try_from(query.limit()).map_err(ApiError::internal)? {
                    rows.pop()
                } else {
                    None
                };
            let next_cursor = rows
                .last()
                .filter(|_| continuation.is_some())
                .map(cursor_of);
            let continuation = match continuation {
                Some(row) => Some(
                    Source::from_row(conn, row)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                ),
                None => None,
            };
            let window = Window {
                sources: sources(conn, rows).await?,
                continuation,
                next_cursor,
            };
            require_sources(ctx, conn, &principal, &window).await?;
            Ok((window, facts))
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a PageQuery,
        opened: &'a Window,
    ) -> BoxFuture<'a, ApiResult<(Window, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let (principal, facts) = root(ctx, conn, caller).await?;
            let mut refreshed = Vec::new();
            for old in opened.witnesses() {
                refreshed.push(
                    Source::load(conn, old.row.id)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                );
            }
            let continuation = if opened.continuation.is_some() {
                refreshed.pop()
            } else {
                None
            };
            let window = Window {
                sources: refreshed,
                continuation,
                next_cursor: opened.next_cursor.clone(),
            };
            require_sources(ctx, conn, &principal, &window).await?;
            Ok((window, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal))
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        window: &'a Window,
    ) -> BoxFuture<'a, ApiResult<Page<Snapshot>>> {
        Box::pin(async move {
            require_sources(ctx, conn, principal, window).await?;
            let mut items = views(
                conn,
                window
                    .sources
                    .iter()
                    .map(|source| source.row.clone())
                    .collect(),
                principal.now(),
            )
            .await?;
            for (item, source) in items.iter_mut().zip(&window.sources) {
                item.admin_access =
                    orch_engine::archive::custody_inspection(principal, source.row.created_by);
            }
            Ok(Page {
                items,
                next_cursor: window.next_cursor.clone(),
            })
        })
    }
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _query: &'a PageQuery,
        opened: &'a Window,
        fresh: &'a Window,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            require_sources(ctx, conn, principal, fresh).await?;
            Ok(opened.sources.len() == fresh.sources.len()
                && opened.continuation.is_some() == fresh.continuation.is_some()
                && opened
                    .witnesses()
                    .zip(fresh.witnesses())
                    .all(|(old, now)| old.witness().matches(&now.row)))
        })
    }
}
