//! Administrative credential metadata and revocation through the existing credential writers.

use super::invalid;

use super::page_size;

use super::read_ahead;

use crate::key_operations::credentials;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::registry::Registry;

use chrono::DateTime;

use chrono::Utc;

use futures::future::BoxFuture;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::admin::AdminToken;

use orch_api_contract::admin::AdminTokenDelete;

use orch_api_contract::admin::AdminTokenList;

use orch_api_contract::admin::AdminTokenListQuery;

use orch_api_contract::admin::AdminUserTokenDelete;

use orch_api_contract::admin::AdminUserTokenList;

use orch_api_contract::admin_sources as sources;

use orch_api_contract::admin_sources::Source;

use orch_api_contract::common::Page;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::access;

use orch_db::queries::identity;

use orch_db::queries::identity::ApiTokenInventory;

use orch_types::Id;

use serde_json::json;
const KEY: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Key);

const NAME_SOURCES: [Source; 4] = [
    Source::TokenOrgNames,
    Source::TokenProjectNames,
    Source::TokenTopicNames,
    Source::TokenThreadNames,
];

/// Register the native system token inventory and revocation.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            AdminTokenList,
            native::selected::<Tokens<false>>,
            list::<false>,
        )
        .on_with_optional_operation(
            AdminTokenDelete,
            native::stored::<Token<false>, _, _>,
            revoke::<false>,
        )
        .on_snapshot_read_with_optional_operation(
            AdminUserTokenList,
            native::selected::<Tokens<true>>,
            list::<true>,
        )
        .on_with_optional_operation(
            AdminUserTokenDelete,
            native::stored::<Token<true>, _, _>,
            revoke::<true>,
        );
}

async fn list<const PERSON: bool>(
    ctx: Ctx,
    query: AdminTokenListQuery,
    (): (),
) -> ApiResult<Page<AdminToken>> {
    native::selected_read::<Tokens<PERSON>>(ctx, query, ()).await
}

struct Tokens<const PERSON: bool>;

pub(super) struct Selected {
    page: Page<AdminToken>,
    plan: OperationPlan,
    ids: Vec<Id>,
    owners: Vec<(Id, Id)>,
}

pub(super) fn cursor(route: &str, owner: Option<Id>, at: DateTime<Utc>, id: Id) -> String {
    format!(
        "{route}|{}|{}|{id}",
        owner.map_or_else(|| "-".into(), |id| id.to_string()),
        at.timestamp_micros()
    )
}

pub(super) fn after(
    ctx: &Ctx,
    query: &AdminTokenListQuery,
) -> ApiResult<Option<(DateTime<Utc>, Id)>> {
    query
        .cursor
        .as_deref()
        .map(|text| {
            let bad = || invalid("cursor", "is not a cursor of this list");
            let parts: Vec<_> = text.split('|').collect();
            if parts.len() != 4
                || parts[0] != ctx.route.name
                || parts[1]
                    != query
                        .owner_id
                        .map_or_else(|| "-".into(), |id| id.to_string())
            {
                return Err(bad());
            }
            Ok((
                parts[2]
                    .parse::<i64>()
                    .ok()
                    .and_then(DateTime::<Utc>::from_timestamp_micros)
                    .ok_or_else(bad)?,
                parts[3].parse().map_err(|_| bad())?,
            ))
        })
        .transpose()
}

async fn row_targets(
    conn: &mut Conn,
    rows: &[ApiTokenInventory],
) -> ApiResult<Vec<KeyResolvedTarget>> {
    let accounts: Vec<_> = rows
        .iter()
        .map(|row| (row.key.user_id, row.owner_kind.as_str(), row.owner_org))
        .collect();
    let owners = targets::account_identities(conn, &accounts).await?;
    rows.iter()
        .map(|row| {
            let account = owners
                .get(&row.key.user_id)
                .ok_or_else(ApiError::not_found)?;
            targets::from_key_subject(account, row.key.id, row.key.user_id)
        })
        .collect()
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &AdminTokenListQuery,
) -> ApiResult<(Selected, Vec<native::Facts<Source>>)> {
    let known = OperationPlan::new(ctx.route, ctx.route.name)?;
    let principal = native::observed(
        ctx,
        "admin_tokens.discovery",
        native::preparation_principal(ctx, conn, caller, &known).await,
    )?;
    native::observed(
        ctx,
        "admin_tokens.discovery_standing",
        super::native::standing(ctx, &principal),
    )?;
    let size = page_size(query.limit);
    let mut after = after(ctx, query)?;
    let mut kept = Vec::new();
    'pages: loop {
        let batch =
            identity::api_token_inventory(conn, query.owner_id, after, read_ahead(size)).await?;
        let exhausted = batch.len() < size.saturating_add(1);
        let facts = row_targets(conn, &batch).await?;
        for (row, target) in batch.into_iter().zip(facts) {
            after = Some((row.key.created_at, row.key.id));
            if !native::modern(caller) || known.permits_binding(caller, 0, KEY, &[target])? {
                kept.push(row);
                if kept.len() > size {
                    break 'pages;
                }
            }
        }
        if exhausted {
            break;
        }
    }
    let more = kept.len() > size;
    kept.truncate(size);
    let next_cursor = if more {
        kept.last().map(|row| {
            cursor(
                ctx.route.name,
                query.owner_id,
                row.key.created_at,
                row.key.id,
            )
        })
    } else {
        None
    };
    project(ctx, conn, caller, kept, next_cursor).await
}

async fn project(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    rows: Vec<ApiTokenInventory>,
    next_cursor: Option<String>,
) -> ApiResult<(Selected, Vec<native::Facts<Source>>)> {
    let targets = row_targets(conn, &rows).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let mut facts = vec![native::Facts::collection(
        Source::Results,
        if targets.is_empty() {
            vec![]
        } else {
            vec![(KEY, targets.clone())]
        },
    )];
    if targets.is_empty() {
        plan.bind_empty(0)?;
    } else {
        plan.bind(0, KEY, &targets)?;
    }
    let ids = rows.iter().map(|r| r.key.id).collect();
    let owners = rows.iter().map(|r| (r.key.id, r.key.user_id)).collect();
    let accounts: Vec<_> = rows.iter().map(|r| r.key.user_id).collect();
    let people = authorization::refusals::user_refs(conn, &accounts).await?;
    let owner_ids: Vec<_> = rows.iter().map(|r| r.key.user_id).collect();
    let mut info = rows
        .into_iter()
        .map(|row| credentials::key_info(row.key))
        .collect::<ApiResult<Vec<_>>>()?;
    let names = credentials::key_names(conn, caller, &mut plan, &mut info).await?;
    for (source, named) in NAME_SOURCES.into_iter().zip(names) {
        facts.push(super::native::collection(
            source,
            named.into_iter().map(|t| (t.object.kind, t)).collect(),
        ));
    }
    let items = info
        .into_iter()
        .zip(owner_ids)
        .map(|(info, id)| {
            Ok(AdminToken {
                info,
                owner: people.get(&id).cloned().ok_or_else(ApiError::not_found)?,
                admin_access: true,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok((
        Selected {
            page: Page { items, next_cursor },
            plan,
            ids,
            owners,
        },
        facts,
    ))
}

impl<const PERSON: bool> native::Selected for Tokens<PERSON> {
    type Source = Source;
    type Query = AdminTokenListQuery;
    type Value = Selected;
    type Reply = Page<AdminToken>;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = if PERSON {
        &sources::USER_TOKENS
    } else {
        &sources::TOKENS
    };
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut query = query.clone();
            if PERSON {
                let owner = ctx.id("id")?;
                super::native::person(conn, owner).await?;
                if query.owner_id.is_some_and(|id| id != owner) {
                    return Err(invalid("owner_id", "must match the selected person"));
                }
                query.owner_id = Some(owner);
            }
            select(ctx, conn, caller, &query).await
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a Self::Query,
        opened: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let rows = identity::api_token_inventory_selected(conn, &opened.ids).await?;
            project(ctx, conn, caller, rows, opened.page.next_cursor.clone()).await
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if PERSON {
                super::native::person(_conn, ctx.id("id")?).await?;
            }
            native::observed(
                ctx,
                "admin_tokens.current_administration",
                super::native::require(ctx, principal),
            )
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Page<AdminToken>>> {
        Box::pin(async move { Ok(value.page.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        old: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.ids == fresh.ids && old.owners == fresh.owners) })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        _accepted: &'a Caller,
        query: &'a Self::Query,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (mut tx, guard) =
                native::observed(ctx, "admin_tokens.audit_guard", super::action(ctx).await)?;
            let (fresh, _) =
                Self::refresh(ctx, &mut tx, guard.principal().caller(), query, value).await?;
            if fresh.ids != value.ids
                || fresh.owners != value.owners
                || !fresh.plan.shared().covers_sources(value.plan.shared())
            {
                return Err(credentials::refused());
            }
            let admitted = native::observed(
                ctx,
                "admin_tokens.audit_admission",
                fresh.plan.admit(guard.principal().caller()),
            )?;
            super::native::standing(ctx, guard.principal())?;
            native::observed(
                ctx,
                "admin_tokens.audit_administration",
                ctx.ordinary_system_admin_with(guard.user(), &admitted.caller),
            )?;
            // Protected optional node names were already selected/masked and their exact
            // disclosure footprint checked above. They cannot veto the primary Key read.

            guard.check_deadline(ctx)?;
            super::record(
                ctx,
                &mut tx,
                guard.user().id(),
                orch_db::audit::action!(AuditAction::AdminOpened),
                query.owner_id,
                "opened API token metadata",
                json!({"operation":ctx.route.name,"tokens":value.ids,"owner_id":query.owner_id}),
            )
            .await?;
            guard.check_deadline(ctx)?;
            tx.commit().await.map_err(DbError::from)?;
            guard.check_deadline(ctx)
        })
    }
}

struct Token<const PERSON: bool>;

impl<const PERSON: bool> native::Stored for Token<PERSON> {
    type Source = Source;
    type Value = ApiTokenInventory;
    const ROWS: &'static [Declaration<Source>] = &[if PERSON {
        sources::USER_TOKEN_REVOKE
    } else {
        sources::TOKEN_REVOKE
    }];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _row: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(ApiTokenInventory, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = token_row::<PERSON>(ctx, conn).await?;
            let account = targets::account(conn, row.key.user_id).await?;
            let target = targets::from_key_subject(&account, row.key.id, row.key.user_id)?;
            Ok((
                row,
                vec![
                    native::Facts::one(Source::Token, KEY, target.clone()),
                    native::Facts::one(Source::TokenContent, KEY, target),
                ],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _value: &'a ApiTokenInventory,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if PERSON {
                super::native::person(_conn, ctx.id("id")?).await?;
            }
            native::observed(
                ctx,
                "admin_tokens.current_administration",
                super::native::require(ctx, principal),
            )
        })
    }
}

async fn revoke<const PERSON: bool>(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, guard) = super::action(&ctx).await?;
    let row = token_row::<PERSON>(&ctx, &mut tx).await?;
    guard.check_deadline(&ctx)?;
    if !identity::revoke_api_token(&mut tx, row.key.id, row.key.user_id, ctx.app.clock.now())
        .await?
    {
        return Err(ApiError::not_found());
    }
    access::bump_epoch(&mut tx).await?;
    super::record(
        &ctx,
        &mut tx,
        guard.user().id(),
        orch_db::audit::action!(AuditAction::AuthTokenRevoked),
        Some(row.key.user_id),
        "revoked an API token through system administration",
        json!({"token":row.key.id,"owner_id":row.key.user_id,"admin_access":true}),
    )
    .await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(&ctx)
}

async fn token_row<const PERSON: bool>(ctx: &Ctx, conn: &mut Conn) -> ApiResult<ApiTokenInventory> {
    let id = ctx.id(if PERSON { "token_id" } else { "id" })?;
    let row = identity::api_token_inventory_selected(conn, &[id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    if PERSON {
        super::native::person(conn, ctx.id("id")?).await?;
        if row.key.user_id != ctx.id("id")? {
            return Err(ApiError::not_found());
        }
    }
    Ok(row)
}
