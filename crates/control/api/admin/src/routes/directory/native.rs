//! Native directory subjects and row selections, consumed by shared declaration adapters.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::transaction_guard;

use crate::registry::OperationPlan;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::directory_sources as sources;

use orch_api_contract::directory_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use std::collections::BTreeSet;

use crate::key_operations::native::Stored;
const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

pub(super) enum Subject {
    Tag {
        row: TagRow,
        kind: NodeKind,
        org: Id,
    },
    Organization {
        id: Id,
    },
}

pub(super) struct Directory;

impl native::Stored for Directory {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[
        sources::TAG_UPDATE,
        sources::TAG_DELETE,
        sources::MEMBER_SUSPEND,
    ];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            match declaration.bindings.first().map(|binding| binding.source) {
                Some(Source::TagNode) => {
                    let (row, place, kind) = tag_and_place(conn, ctx.id("id")?).await?;
                    let target = targets::node(conn, row.node_id).await?;
                    let family = target.object.kind;
                    Ok((
                        Subject::Tag {
                            row,
                            kind,
                            org: place.org,
                        },
                        vec![native::Facts::one(Source::TagNode, family, target)],
                    ))
                }
                Some(Source::Organization) => {
                    let id = ctx.id("id")?;
                    the_org(conn, id).await?;
                    let target = targets::node(conn, id).await?;
                    Ok((
                        Subject::Organization { id },
                        vec![native::Facts::one(
                            Source::Organization,
                            target.object.kind,
                            target,
                        )],
                    ))
                }
                _ => Err(ApiError::internal(
                    "the directory stored source is undeclared",
                )),
            }
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match subject {
                Subject::Tag { row, kind, .. } => {
                    let permission = match kind {
                        NodeKind::Org => Permission::OrgManage,
                        NodeKind::Project => Permission::ProjectManage,
                        NodeKind::Topic => Permission::TopicManage,
                        NodeKind::Thread => Permission::ThreadManage,
                    };
                    authorization::require_projected(
                        conn,
                        principal,
                        row.node_id,
                        exact(permission),
                        Act::Use,
                        Policy::default(),
                        ctx.key_receipt_required(),
                    )
                    .await
                }
                Subject::Organization { id } => {
                    require_on_keeper_projected(
                        ctx,
                        conn,
                        principal,
                        *id,
                        exact(Permission::MembersManage),
                    )
                    .await
                }
            }
        })
    }
}

/// The native mutation footprint is locked before the common final current-authority pass.
pub(super) async fn lock_tag(ctx: &Ctx, conn: &mut Conn) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let keeper = tag_and_place(conn, id).await?.0.node_id;
    let node: Option<Id> =
        sqlx::query_scalar("SELECT id FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
            .bind(keeper)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let tag: Option<Id> =
        sqlx::query_scalar("SELECT id FROM tags WHERE id=$1 AND node_id=$2 FOR UPDATE")
            .bind(id)
            .bind(keeper)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    if node.is_none() || tag.is_none() {
        return Err(ApiError::not_found());
    }
    Ok(())
}

pub(super) async fn tag_after_wait(
    ctx: &Ctx,
    conn: &mut Conn,
    initial: &transaction_guard::Guard,
) -> ApiResult<(Ctx, transaction_guard::Guard, TagRow, Placed, NodeKind)> {
    // Preserve the ordinary early gate before contending for the native target.
    // It is preliminary only: the same authority is refreshed after serialization.
    let (row, place, kind) = tag_and_place(conn, ctx.id("id")?).await?;
    let gate = Directory::require(
        ctx,
        conn,
        initial.principal(),
        &Subject::Tag {
            row,
            kind,
            org: place.org,
        },
    )
    .await;
    initial.check_deadline(ctx)?;
    gate?;
    lock_tag(ctx, conn).await?;
    let (ctx, guard) = transaction_guard::after_wait(ctx, conn).await?;
    let (row, place, kind) = if native::modern(guard.principal().caller()) {
        let ready = guard.operation_data::<native::Prepared<Subject>>()?;
        let Subject::Tag { row, kind, org } = &ready.value else {
            return Err(ApiError::internal(
                "the prepared tag subject is unavailable",
            ));
        };
        (
            row.clone(),
            Placed {
                id: row.node_id,
                org: *org,
            },
            *kind,
        )
    } else {
        tag_and_place(conn, ctx.id("id")?).await?
    };
    Directory::require(
        &ctx,
        conn,
        guard.principal(),
        &Subject::Tag {
            row: row.clone(),
            kind,
            org: place.org,
        },
    )
    .await?;
    guard.check_deadline(&ctx)?;
    Ok((ctx, guard, row, place, kind))
}

/// A completed write is never retried because its optional content became unavailable.
pub(super) async fn tag_response(ctx: &Ctx, accepted: &Caller) -> ApiResult<Option<Tag>> {
    let projection_accepted = accepted.clone();
    native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let (row, place, kind) = tag_and_place(conn, ctx.id("id")?).await?;
            if !native::modern(current.caller()) {
                Directory::require(
                    ctx,
                    conn,
                    current,
                    &Subject::Tag {
                        row: row.clone(),
                        kind,
                        org: place.org,
                    },
                )
                .await?;
                return shown(conn, current, &[row])
                    .await?
                    .into_iter()
                    .next()
                    .map(Some)
                    .ok_or_else(ApiError::not_found);
            }
            let target = targets::node(conn, row.node_id).await?;
            let facts = [native::Facts::one(
                Source::TagNode,
                target.object.kind,
                target,
            )];
            let plan = native::bind(ctx, &sources::TAG_UPDATE, &facts)?;
            plan.read_response(ctx, conn, &projection_accepted, move |conn, reader| {
                Box::pin(async move {
                    authorization::require_projected(
                        conn,
                        reader,
                        row.node_id,
                        authorization::VIEW,
                        Act::Use,
                        Policy::default(),
                        true,
                    )
                    .await?;
                    shown(conn, reader, &[row])
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(ApiError::not_found)
                })
            })
            .await
        })
    })
    .await
}

pub(super) async fn suspension_response(
    ctx: &Ctx,
    accepted: &Caller,
    org: Id,
    user: Id,
) -> ApiResult<Option<Suspension>> {
    let projection_accepted = accepted.clone();
    native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            if !native::modern(current.caller()) {
                require_on_keeper_projected(
                    ctx,
                    conn,
                    current,
                    org,
                    exact(Permission::MembersManage),
                )
                .await?;
                let rows = access::suspensions_in(conn, org, Some(user)).await?;
                return suspensions_shown(conn, &rows)
                    .await?
                    .into_iter()
                    .next()
                    .map(Some)
                    .ok_or_else(ApiError::not_found);
            }
            let target = targets::node(conn, org).await?;
            let facts = [native::Facts::one(
                Source::Organization,
                target.object.kind,
                target,
            )];
            let plan = native::bind(ctx, &sources::MEMBER_SUSPEND, &facts)?;
            let projection = ctx.clone();
            plan.read_response(ctx, conn, &projection_accepted, move |conn, reader| {
                Box::pin(async move {
                    require_on_keeper_projected(
                        &projection,
                        conn,
                        reader,
                        org,
                        exact(Permission::MembersManage),
                    )
                    .await?;
                    let rows = access::suspensions_in(conn, org, Some(user)).await?;
                    suspensions_shown(conn, &rows)
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(ApiError::not_found)
                })
            })
            .await
        })
    })
    .await
}

/// Recheck the writer's actual stored Tag association at final transport publication.
pub(super) fn retain_tag_response(
    ctx: &Ctx,
    accepted: &Caller,
    tag: Id,
    governor: Id,
) -> ApiResult<()> {
    if !native::modern(accepted) {
        return Ok(());
    }
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        (tag, governor, accepted.clone()),
        |ctx, conn, _current, (tag, governor, accepted)| {
            Box::pin(async move {
                let (row, _, _) = tag_and_place(conn, *tag).await?;
                if row.node_id != *governor {
                    return Err(ApiError::not_found());
                }
                let target = targets::node(conn, *governor).await?;
                let plan = native::bind(
                    ctx,
                    &sources::TAG_UPDATE,
                    &[native::Facts::one(
                        Source::TagNode,
                        target.object.kind,
                        target,
                    )],
                )?;
                let governor = *governor;
                plan.read_response(ctx, conn, accepted, move |conn, reader| {
                    Box::pin(async move {
                        authorization::require_projected(
                            conn,
                            reader,
                            governor,
                            authorization::VIEW,
                            Act::Use,
                            Policy::default(),
                            true,
                        )
                        .await
                    })
                })
                .await?
                .ok_or_else(|| ApiError::new(ErrorCode::AuthzDenied, "the tag response is closed"))
            })
        },
    )
}

/// The native suspension keeps its actual org/account tuple after the completed effect.
pub(super) fn retain_suspension_response(
    ctx: &Ctx,
    accepted: &Caller,
    org: Id,
    user: Id,
) -> ApiResult<()> {
    if !native::modern(accepted) {
        return Ok(());
    }
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        (org, user, accepted.clone()),
        |ctx, conn, _current, (org, user, accepted)| {
            Box::pin(async move {
                if access::suspensions_in(conn, *org, Some(*user))
                    .await?
                    .is_empty()
                {
                    return Err(ApiError::not_found());
                }
                let target = targets::node(conn, *org).await?;
                let plan = native::bind(
                    ctx,
                    &sources::MEMBER_SUSPEND,
                    &[native::Facts::one(
                        Source::Organization,
                        target.object.kind,
                        target,
                    )],
                )?;
                let projection = ctx.clone();
                let org = *org;
                plan.read_response(ctx, conn, accepted, move |conn, reader| {
                    Box::pin(async move {
                        require_on_keeper_projected(
                            &projection,
                            conn,
                            reader,
                            org,
                            exact(Permission::MembersManage),
                        )
                        .await
                    })
                })
                .await?
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::AuthzDenied, "the suspension response is closed")
                })
            })
        },
    )
}

pub(super) struct Members {
    rows: Vec<access::MemberRow>,
    next_cursor: Option<String>,
}

async fn select_members(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &OrgMemberListQuery,
) -> ApiResult<(Members, Vec<native::Facts<Source>>)> {
    let org = ctx.id("id")?;
    let plan = OperationPlan::new(ctx.route, sources::MEMBER_LIST.operation)?;
    the_org(conn, org).await?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page.limit()).map_err(ApiError::internal)?;
    let mut after = query
        .cursor
        .as_deref()
        .map(|text| {
            let bad = || invalid("cursor", "is not a cursor of this list");
            let (name, user) = text.rsplit_once('|').ok_or_else(bad)?;
            Ok::<_, ApiError>((name.to_owned(), user.parse::<Id>().map_err(|_| bad())?))
        })
        .transpose()?;
    let mut selected = Vec::new();
    let mut accounts = Vec::new();
    let mut domain_facts = Vec::new();
    let mut domains_seen = BTreeSet::new();
    loop {
        // Refill only scoped selections. Ordinary reads keep the original one-window query.
        let rows = access::members_of(
            conn,
            org,
            query.direct.unwrap_or(false),
            query.q.as_deref().filter(|text| !text.is_empty()),
            after.as_ref().map(|(name, id)| (name.as_str(), *id)),
            i64::try_from(limit + 1).map_err(ApiError::internal)?,
            ctx.now,
        )
        .await?;
        let exhausted = rows.len() <= limit;
        let next = rows.last().map(|row| (row.username.clone(), row.user_id));
        let identities = rows
            .iter()
            .map(|row| (row.user_id, row.account_kind.as_str(), row.service_org_id))
            .collect::<Vec<_>>();
        let account_targets = targets::account_identities(conn, &identities).await?;
        let domain_ids: Vec<_> = rows.iter().map(|row| row.domain_id).collect();
        let graph = orch_authz::engine::Graph::load(conn, &domain_ids)
            .await
            .map_err(authorization::restoration_error)?;
        for row in rows {
            let account = account_targets
                .get(&row.user_id)
                .ok_or_else(ApiError::not_found)?;
            let path = graph
                .get(row.domain_id)
                .ok_or_else(ApiError::not_found)?
                .path
                .clone();
            let mut names = Vec::new();
            let mut allowed = !native::modern(caller)
                || native::permits(
                    &plan,
                    &sources::MEMBER_LIST,
                    caller,
                    &Source::Members,
                    ACCOUNT,
                    account,
                )?;
            for id in path {
                let target = orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                    .ok_or_else(ApiError::not_found)?;
                if target.object.kind
                    != KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org)
                {
                    return Err(ApiError::not_found());
                }
                allowed &= !native::modern(caller)
                    || native::permits(
                        &plan,
                        &sources::MEMBER_LIST,
                        caller,
                        &Source::MemberDomains,
                        target.object.kind,
                        &target,
                    )?;
                names.push(target);
            }
            if allowed {
                accounts.push(account.clone());
                for target in names {
                    if domains_seen.insert(target.object.id) {
                        domain_facts.push(target);
                    }
                }
                selected.push(row);
            }
            if selected.len() > limit {
                break;
            }
        }
        if selected.len() > limit || exhausted || !native::modern(caller) {
            break;
        }
        after = next;
    }
    let more = selected.len() > limit;
    selected.truncate(limit);
    accounts.truncate(limit);
    // A look-ahead row is cursor evidence only, never part of the returned footprint.
    let selected_domains: BTreeSet<_> = selected.iter().map(|row| row.domain_id).collect();
    let selected_graph = orch_authz::engine::Graph::load(
        conn,
        &selected_domains.iter().copied().collect::<Vec<_>>(),
    )
    .await
    .map_err(authorization::restoration_error)?;
    let actual_names: BTreeSet<_> = selected
        .iter()
        .flat_map(|row| {
            selected_graph
                .get(row.domain_id)
                .into_iter()
                .flat_map(|node| node.path.iter().copied())
        })
        .collect();
    domain_facts.retain(|target| {
        target
            .object
            .id
            .is_some_and(|id| actual_names.contains(&id))
    });
    let next_cursor = more
        .then(|| {
            selected
                .last()
                .map(|row| format!("{}|{}", row.username, row.user_id))
        })
        .flatten();
    let account_groups = if accounts.is_empty() {
        Vec::new()
    } else {
        vec![(ACCOUNT, accounts)]
    };
    let domain_groups = if domain_facts.is_empty() {
        Vec::new()
    } else {
        vec![(
            KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org),
            domain_facts,
        )]
    };
    Ok((
        Members {
            rows: selected,
            next_cursor,
        },
        vec![
            native::Facts::collection(Source::Members, account_groups),
            native::Facts::collection(Source::MemberDomains, domain_groups),
        ],
    ))
}

async fn render_members(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    selected: &Members,
) -> ApiResult<Page<Member>> {
    let org = ctx.id("id")?;
    let manages = orch_authz::nodes::check(
        conn,
        principal,
        org,
        Permission::MembersManage,
        Act::Use,
        Policy::default(),
    )
    .await
    .is_ok_and(|decision| decision.is_allowed());
    let users = selected
        .rows
        .iter()
        .map(|row| row.user_id)
        .collect::<Vec<_>>();
    let domains = selected
        .rows
        .iter()
        .map(|row| row.domain_id)
        .collect::<Vec<_>>();
    let people = refusals::user_refs(conn, &users).await?;
    let refs =
        crate::routes::nodes::view::refs(conn, principal, &domains, Policy::default()).await?;
    let mut items = selected
        .rows
        .iter()
        .filter_map(|row| member_of(row, &people, &refs, manages))
        .collect::<Vec<_>>();
    if manages {
        members::fill_managed(conn, org, &mut items).await?;
    }
    Ok(Page {
        items,
        next_cursor: selected.next_cursor.clone(),
    })
}

async fn select_services(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(Vec<identity::ServiceRow>, Vec<native::Facts<Source>>)> {
    let org = ctx.id("id")?;
    let plan = OperationPlan::new(ctx.route, sources::SERVICE_LIST.operation)?;
    the_org(conn, org).await?;
    let rows = identity::service_accounts_of(conn, org).await?;
    let identities = rows
        .iter()
        .map(|row| (row.id, "service", Some(row.org_id)))
        .collect::<Vec<_>>();
    let accounts = targets::account_identities(conn, &identities).await?;
    let mut selected = Vec::new();
    let mut facts = Vec::new();
    for row in rows {
        let target = accounts.get(&row.id).ok_or_else(ApiError::not_found)?;
        if !native::modern(caller)
            || native::permits(
                &plan,
                &sources::SERVICE_LIST,
                caller,
                &Source::ServiceAccounts,
                ACCOUNT,
                target,
            )?
        {
            facts.push(target.clone());
            selected.push(row);
        }
    }
    let groups = if facts.is_empty() {
        Vec::new()
    } else {
        vec![(ACCOUNT, facts)]
    };
    Ok((
        selected,
        vec![native::Facts::collection(Source::ServiceAccounts, groups)],
    ))
}

pub(super) struct MemberRows;

impl native::Selected for MemberRows {
    type Source = Source;
    type Query = OrgMemberListQuery;
    type Value = Members;
    type Reply = Page<Member>;
    const DECLARATION: &'static Declaration<Source> = &sources::MEMBER_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Members, Vec<native::Facts<Source>>)>> {
        Box::pin(select_members(ctx, conn, caller, query))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require_projected(
                conn,
                principal,
                ctx.id("id")?,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
                native::modern(principal.caller()),
            )
            .await
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Members,
    ) -> BoxFuture<'a, ApiResult<Page<Member>>> {
        Box::pin(render_members(ctx, conn, principal, value))
    }
}

pub(super) struct ServiceRows;

impl native::Selected for ServiceRows {
    type Source = Source;
    type Query = ();
    type Value = Vec<identity::ServiceRow>;
    type Reply = Vec<orch_api_contract::directory::ServiceAccount>;
    const DECLARATION: &'static Declaration<Source> = &sources::SERVICE_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_services(ctx, conn, caller))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require_projected(
                conn,
                principal,
                ctx.id("id")?,
                exact(Permission::MembersManage),
                Act::Use,
                Policy::default(),
                native::modern(principal.caller()),
            )
            .await
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        let _ = (ctx, principal);
        Box::pin(services_shown(conn, value))
    }
}
