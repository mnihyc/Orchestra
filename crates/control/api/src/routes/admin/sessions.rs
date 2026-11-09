//! Current browser-session metadata and exact native revocation for a selected person.
use crate::key_operations::{
    credentials,
    native::{self, effects},
    targets,
};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan, Registry};
use futures::future::BoxFuture;
use orch_access_contract::{
    audit::AuditAction,
    key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind},
};
use orch_api_contract::{
    admin::{
        AdminSession, AdminTokenListQuery, AdminUserSessionDelete, AdminUserSessionList,
        AdminUserSessionsDelete,
    },
    admin_sources::{self as sources, Source},
    auth::SessionInfo,
    common::{Page, PageQuery},
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        access,
        identity::{self, SessionListed},
        transaction_guard,
    },
};
use orch_types::{Id, Time};
use serde_json::json;
const SESSION: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Session);
/// Register one native session list and the same revoker's single/all selections.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            AdminUserSessionList,
            native::selected::<Sessions>,
            list,
        )
        .on_with_operation(
            AdminUserSessionDelete,
            effects::prepare::<Revoke<false>>,
            revoke::<false>,
        )
        .on_with_operation(
            AdminUserSessionsDelete,
            effects::prepare::<Revoke<true>>,
            revoke::<true>,
        );
}
async fn list(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<Page<AdminSession>> {
    native::selected_read::<Sessions>(ctx, query, ()).await
}
struct Sessions;
struct Selected {
    page: Page<AdminSession>,
    ids: Vec<Id>,
    owner: Id,
    plan: OperationPlan,
}
fn target(account: &KeyResolvedTarget, row: &SessionListed) -> ApiResult<KeyResolvedTarget> {
    targets::from_session_subject(account, row.id, row.user_id)
}
async fn project(
    ctx: &Ctx,
    conn: &mut Conn,
    _caller: &Caller,
    owner: Id,
    rows: Vec<SessionListed>,
    next_cursor: Option<String>,
) -> ApiResult<(Selected, Vec<native::Facts<Source>>)> {
    let account = targets::account(conn, owner).await?;
    let facts = rows
        .iter()
        .map(|row| target(&account, row))
        .collect::<ApiResult<Vec<_>>>()?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    if facts.is_empty() {
        plan.bind_empty(0)?;
    } else {
        plan.bind(0, SESSION, &facts)?;
    }
    let current = match ctx.user()?.via {
        crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
        _ => None,
    };
    let ids = rows.iter().map(|row| row.id).collect();
    let items = rows
        .into_iter()
        .map(|row| AdminSession {
            info: SessionInfo {
                id: row.id,
                current: current == Some(row.id),
                user_agent: row.user_agent,
                ip: row.ip,
                created_at: Time::new(row.created_at),
                last_used_at: Time::new(row.last_used_at),
                expires_at: Time::new(row.expires_at),
            },
            admin_access: true,
        })
        .collect();
    Ok((
        Selected {
            page: Page { items, next_cursor },
            ids,
            owner,
            plan,
        },
        vec![native::Facts::collection(
            Source::Results,
            if facts.is_empty() {
                vec![]
            } else {
                vec![(SESSION, facts)]
            },
        )],
    ))
}
impl native::Selected for Sessions {
    type Source = Source;
    type Query = PageQuery;
    type Value = Selected;
    type Reply = Page<AdminSession>;
    const DECLARATION: &'static Declaration<Source> = &sources::USER_SESSIONS;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let owner = ctx.id("id")?;
            super::native::person(conn, owner).await?;
            let known = OperationPlan::new(ctx.route, ctx.route.name)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            super::native::standing(ctx, &principal)?;
            let account = targets::account(conn, owner).await?;
            let cursor_query = AdminTokenListQuery {
                owner_id: Some(owner),
                cursor: query.cursor.clone(),
                limit: query.limit,
            };
            let mut after = super::tokens::after(ctx, &cursor_query)?;
            let size = super::page_size(query.limit);
            let mut rows = Vec::new();
            'pages: loop {
                let batch = identity::session_inventory(
                    conn,
                    owner,
                    ctx.app.clock.now(),
                    after,
                    super::read_ahead(size),
                )
                .await?;
                let exhausted = batch.len() < size.saturating_add(1);
                for row in batch {
                    after = Some((row.created_at, row.id));
                    let fact = target(&account, &row)?;
                    if !native::modern(caller)
                        || known.permits_binding(caller, 0, SESSION, &[fact])?
                    {
                        rows.push(row);
                        if rows.len() > size {
                            break 'pages;
                        }
                    }
                }
                if exhausted {
                    break;
                }
            }
            let more = rows.len() > size;
            rows.truncate(size);
            let next_cursor = if more {
                rows.last().map(|row| {
                    super::tokens::cursor(ctx.route.name, Some(owner), row.created_at, row.id)
                })
            } else {
                None
            };
            project(ctx, conn, caller, owner, rows, next_cursor).await
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a PageQuery,
        old: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let rows = identity::session_inventory_selected(
                conn,
                old.owner,
                &old.ids,
                ctx.app.clock.now(),
            )
            .await?;
            project(
                ctx,
                conn,
                caller,
                old.owner,
                rows,
                old.page.next_cursor.clone(),
            )
            .await
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            super::native::person(conn, ctx.id("id")?).await?;
            super::native::require(ctx, principal)
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Page<AdminSession>>> {
        Box::pin(async move { Ok(value.page.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a PageQuery,
        old: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.ids == fresh.ids && old.owner == fresh.owner) })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        _accepted: &'a Caller,
        query: &'a PageQuery,
        old: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (mut tx, guard) = super::action(ctx).await?;
            let (fresh, _) =
                Self::refresh(ctx, &mut tx, guard.principal().caller(), query, old).await?;
            if fresh.ids != old.ids
                || fresh.owner != old.owner
                || !fresh.plan.shared().covers_sources(old.plan.shared())
                || !fresh.plan.admit(guard.principal().caller())?.content
            {
                return Err(credentials::refused());
            }
            guard.check_deadline(ctx)?;
            super::record(
                ctx,
                &mut tx,
                guard.user().id(),
                orch_db::audit::action!(AuditAction::AdminOpened),
                Some(old.owner),
                "opened browser session metadata",
                json!({"operation":ctx.route.name,"owner_id":old.owner,"sessions":old.ids}),
            )
            .await?;
            guard.check_deadline(ctx)?;
            tx.commit().await.map_err(DbError::from)?;
            guard.check_deadline(ctx)
        })
    }
}
struct Revoke<const ALL: bool>;
struct PreparedRevoke {
    owner: Id,
    sessions: Vec<Id>,
}
impl<const ALL: bool> effects::Effect for Revoke<ALL> {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = PreparedRevoke;
    type Output = ();
    fn declaration(_value: &Self::Value) -> &'static Declaration<Source> {
        if ALL {
            &sources::USER_SESSIONS_REVOKE
        } else {
            &sources::USER_SESSION_REVOKE
        }
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(PreparedRevoke, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let known = OperationPlan::new(ctx.route, ctx.route.name)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            super::native::standing(ctx, &principal)?;
            let owner = ctx.id("id")?;
            let person = transaction_guard::lock_person(conn, owner)
                .await?
                .filter(|row| row.kind == "human")
                .ok_or_else(ApiError::not_found)?;
            let account = targets::from_account(conn, &person).await?;
            let subjects = if ALL {
                identity::unrevoked_session_subjects(conn, owner).await?
            } else {
                vec![
                    identity::unrevoked_session_subject(conn, ctx.id("session_id")?, owner)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                ]
            };
            let sessions = subjects.iter().map(|(id, _)| *id).collect();
            let targets = subjects
                .into_iter()
                .map(|(id, user)| targets::from_session_subject(&account, id, user))
                .collect::<ApiResult<Vec<_>>>()?;
            let facts = if ALL {
                vec![
                    native::Facts::one(Source::Account, SESSION, account.clone()),
                    native::Facts::collection(
                        Source::Sessions,
                        if targets.is_empty() {
                            vec![]
                        } else {
                            vec![(SESSION, targets)]
                        },
                    ),
                    native::Facts::one(Source::SessionContent, SESSION, account),
                ]
            } else {
                let target = targets.into_iter().next().ok_or_else(ApiError::not_found)?;
                vec![
                    native::Facts::one(Source::Session, SESSION, target.clone()),
                    native::Facts::one(Source::SessionContent, SESSION, target),
                ]
            };
            Ok((PreparedRevoke { owner, sessions }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _value: &'a PreparedRevoke,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::native::require(ctx, principal) })
    }
}
impl<const ALL: bool> effects::Apply for Revoke<ALL> {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedRevoke,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<()>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            before.check()?;
            let ended = identity::revoke_session_set(
                conn,
                value.owner,
                &value.sessions,
                ctx.app.clock.now(),
            )
            .await?;
            access::bump_epoch(conn).await?;
            deadline.check()?;
            super::record(ctx,conn,principal.user(),orch_db::audit::action!(AuditAction::AuthSessionRevoked),Some(value.owner),"ended browser sessions through system administration",json!({"owner_id":value.owner,"sessions":value.sessions,"sessions_ended":ended,"all":ALL,"admin_access":true})).await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(()))
        })
    }
}
async fn revoke<const ALL: bool>(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Revoke<ALL>>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let _ = completed;
    Ok(())
}
