//! The native resource-use people window, with separately bounded domain-derived addresses.

use super::api;
use crate::key_operations::native;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use crate::routes::access::page::{self, NameBounds};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{NodePermission, ObjectKind, PermissionName};
use orch_api_contract::library_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Act, Caller, Policy, Principal, engine::Graph};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, Time};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

pub(super) struct People;
pub(super) struct Ready {
    target: api::LibraryTarget,
    ids: Vec<Id>,
    hidden: i64,
    next_cursor: Option<Id>,
    at: Time,
    names: NameBounds,
    addresses: BTreeMap<Id, (Id, Id)>,
    people: Vec<orch_api_contract::users::UserRef>,
}

fn refused() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the original resource-use people source is no longer readable",
    )
}

/// The library names an explicit native resource page. Owned MCP uses its real parent page.
async fn target_facts(
    conn: &mut Conn,
    target: &api::LibraryTarget,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let subject = super::subjects::select(conn, &[target.object])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    Ok(vec![
        native::Facts::one(
            Source::Resource,
            KeyTargetKind::Object(subject.object.kind),
            subject.target,
        ),
        native::Facts::collection(
            Source::OwnedEnvironments,
            subject
                .environment
                .map(|target| vec![(KeyTargetKind::Object(ObjectKind::Environment), vec![target])])
                .unwrap_or_default(),
        ),
    ])
}

/// The same native View and immutable version membership that guard both library readers.
async fn require_target(
    conn: &mut Conn,
    principal: &Principal,
    target: &api::LibraryTarget,
) -> ApiResult<()> {
    super::require_targets(conn, principal, std::slice::from_ref(target)).await
}

fn source(target: &api::LibraryTarget) -> String {
    let column = match target.object.kind {
        ObjectKind::Provider => Some("provider_id"),
        ObjectKind::Agent => Some("agent_id"),
        ObjectKind::Environment => Some("environment_id"),
        _ => None,
    };
    column.map_or_else(||include_str!("../library_people_reported.sql").trim().to_owned(),|column|format!("SELECT user_id id FROM usage_events WHERE {column}=$2 AND turn_id IS NOT NULL AND at<=$6 AND (NOT $9 OR at >= (($6::timestamptz AT TIME ZONE 'UTC')::date-29)::timestamp AT TIME ZONE 'UTC') UNION SELECT t.audit_for_user_id FROM turns t JOIN segments s ON s.id=t.segment_id WHERE s.{column}=$2 AND $1<>'provider' AND t.started_at IS NOT NULL AND t.started_at<=$6 AND (NOT $9 OR t.started_at >= (($6::timestamptz AT TIME ZONE 'UTC')::date-29)::timestamp AT TIME ZONE 'UTC') AND (t.native_turn_ref IS NULL OR t.native_turn_ref NOT LIKE 'fork-copy:%')"))
}

fn scope_sql(target: &api::LibraryTarget) -> String {
    include_str!("../library_people_scope.sql").replace("__SOURCE__", &source(target))
}

struct Window {
    ids: Vec<Id>,
    hidden: i64,
    next_cursor: Option<Id>,
}

async fn select_window(
    conn: &mut Conn,
    principal: &Principal,
    body: &api::LibraryPeopleBody,
    at: Time,
) -> ApiResult<Window> {
    let sql = format!(
        "{} {}",
        scope_sql(&body.target),
        include_str!("../library_people.sql")
    );
    let limit = orch_api_contract::common::DEFAULT_LIMIT;
    let result: Json = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(body.target.object.kind.as_str())
        .bind(body.target.object.id)
        .bind(body.target.version_id)
        .bind(principal.user())
        .bind(principal.memberships().roots())
        .bind(at.get())
        .bind(body.cursor)
        .bind(i64::from(limit) + 1)
        .bind(body.recent)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let mut ids: Vec<Id> =
        serde_json::from_value(result["ids"].clone()).map_err(ApiError::internal)?;
    let more = ids.len() > usize::try_from(limit).map_err(ApiError::internal)?;
    ids.truncate(usize::try_from(limit).map_err(ApiError::internal)?);
    let next_cursor = more.then(|| ids.last().copied()).flatten();
    Ok(Window {
        ids,
        hidden: result["hidden"].as_i64().unwrap_or(0),
        next_cursor,
    })
}

async fn address_sources(
    conn: &mut Conn,
    principal: &Principal,
    caller: &Caller,
    plan: &OperationPlan,
    ids: &[Id],
) -> ApiResult<(NameBounds, HashSet<(Id, Id, Id)>)> {
    let addresses = orch_db::queries::access::holder_user_addresses(conn, ids).await?;
    let wanted: Vec<_> = addresses.iter().map(|(_, _, domain, _)| *domain).collect();
    let visible: BTreeSet<_> = orch_authz::nodes::holding(
        conn,
        principal,
        &wanted,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|node| node.id)
    .collect();
    let graph = Graph::load(conn, &visible.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let (_, nodes) = native::node_name_facts(
        plan,
        &sources::PEOPLE,
        caller,
        &graph,
        &visible,
        &[
            (ObjectKind::Org, Source::Orgs),
            (ObjectKind::Project, Source::Projects),
            (ObjectKind::Topic, Source::Topics),
            (ObjectKind::Thread, Source::Threads),
        ],
    )?;
    let addresses = addresses
        .into_iter()
        .filter(|(_, _, domain, _)| nodes.contains(domain))
        .map(|(user, tree, domain, _)| (user, tree, domain))
        .collect();
    Ok((
        NameBounds {
            limited: native::modern(caller),
            nodes,
            ..Default::default()
        },
        addresses,
    ))
}

async fn named_facts(
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    nodes: &BTreeSet<Id>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let (facts, allowed) = native::node_name_facts(
        plan,
        &sources::PEOPLE,
        caller,
        &graph,
        nodes,
        &[
            (ObjectKind::Org, Source::Orgs),
            (ObjectKind::Project, Source::Projects),
            (ObjectKind::Topic, Source::Topics),
            (ObjectKind::Thread, Source::Threads),
        ],
    )?;
    if !nodes.is_subset(&allowed) {
        return Err(refused());
    }
    Ok(facts)
}

/// Recheck only original captured identities and the same profile audience, never another window.
async fn captured_people(
    conn: &mut Conn,
    principal: &Principal,
    body: &api::LibraryPeopleBody,
    opened: &Ready,
) -> ApiResult<HashSet<Id>> {
    let sql = format!(
        "{} SELECT id FROM people WHERE visible AND id=ANY($10::uuid[])",
        scope_sql(&body.target)
    );
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(body.target.object.kind.as_str())
        .bind(body.target.object.id)
        .bind(body.target.version_id)
        .bind(principal.user())
        .bind(principal.memberships().roots())
        .bind(opened.at.get())
        .bind(body.cursor)
        .bind(i64::from(orch_api_contract::common::DEFAULT_LIMIT) + 1)
        .bind(body.recent)
        .bind(&opened.ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?
        .into_iter()
        .collect())
}

impl native::Selected for People {
    type Source = Source;
    type Query = ((), api::LibraryPeopleBody);
    type Value = Ready;
    type Reply = api::LibraryPeopleReply;
    const DECLARATION: &'static Declaration<Source> = &sources::PEOPLE;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (_, body): &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut facts = target_facts(conn, &body.target).await?;
            let known = native::bind_known(ctx, &sources::PEOPLE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_target(conn, &principal, &body.target).await?;
            let at = principal.now();
            let window = select_window(conn, &principal, body, at).await?;
            let (mut names, _) =
                address_sources(conn, &principal, caller, &known, &window.ids).await?;
            let mut projected =
                page::project_user_refs(conn, &principal, None, &window.ids, Some(&names)).await?;
            if names.limited {
                // Public UserRefs can have a username-address fallback. This projection's
                // address field is optional content from the actual admitted domain source.
                for (id, person) in &mut projected.users {
                    if !projected.addresses.contains_key(id) {
                        person.address = None;
                    }
                }
            }
            names.nodes = projected
                .addresses
                .values()
                .map(|(_, domain)| *domain)
                .collect();
            facts.extend(named_facts(conn, caller, &known, &names.nodes).await?);
            let people = window
                .ids
                .iter()
                .filter_map(|id| projected.users.get(id).cloned())
                .collect();
            Ok((
                Ready {
                    target: body.target.clone(),
                    ids: window.ids,
                    hidden: window.hidden,
                    next_cursor: window.next_cursor,
                    at,
                    names,
                    addresses: projected.addresses,
                    people,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // The actual typed resource and optional version are checked in select and render.
        Box::pin(async { Ok(()) })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<api::LibraryPeopleReply>> {
        Box::pin(async move {
            require_target(conn, principal, &value.target).await?;
            Ok(api::LibraryPeopleReply {
                items: value.people.clone(),
                hidden: value.hidden,
                next_cursor: value.next_cursor,
            })
        })
    }
}

async fn check_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    body: &api::LibraryPeopleBody,
    opened: &native::Prepared<Ready>,
) -> ApiResult<()> {
    let value = &opened.value;
    if value.target != body.target {
        return Err(refused());
    }
    let mut facts = target_facts(conn, &value.target).await?;
    let known = native::bind_known(ctx, &sources::PEOPLE, &facts)?;
    let reader = native::preparation_principal(ctx, conn, principal.caller(), &known).await?;
    require_target(conn, &reader, &value.target).await?;
    let eligible = captured_people(conn, &reader, body, value).await?;
    if value.ids.iter().any(|id| !eligible.contains(id)) {
        return Err(refused());
    }
    let (names, addresses) =
        address_sources(conn, &reader, principal.caller(), &known, &value.ids).await?;
    if !value.names.nodes.is_subset(&names.nodes)
        || !value
            .addresses
            .iter()
            .all(|(user, (tree, domain))| addresses.contains(&(*user, *tree, *domain)))
    {
        return Err(refused());
    }
    facts.extend(named_facts(conn, principal.caller(), &known, &value.names.nodes).await?);
    let plan = native::bind(ctx, &sources::PEOPLE, &facts)?;
    if !plan.shared().covers_sources(opened.response.shared()) {
        return Err(refused());
    }
    if native::modern(principal.caller()) && !plan.admit(principal.caller())?.content {
        return Err(refused());
    }
    ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Full);
    if ctx.key_receipt_required() {
        return Err(refused());
    }
    Ok(())
}

pub(super) async fn get(
    ctx: Ctx,
    (): (),
    body: api::LibraryPeopleBody,
) -> ApiResult<api::LibraryPeopleReply> {
    use native::Selected;
    let retained = body.clone();
    let (reply, producing, accepted, opened) = native::read(ctx, move |ctx, conn, principal| {
        let body = body.clone();
        Box::pin(async move {
            let opened = if native::modern(principal.caller()) {
                ctx.operation_data::<native::Prepared<Ready>>()?
            } else {
                let (value, facts) =
                    People::select(ctx, conn, principal.caller(), &((), body.clone())).await?;
                Arc::new(native::Prepared {
                    response: native::bind(ctx, &sources::PEOPLE, &facts)?,
                    value,
                })
            };
            let reply = if native::modern(principal.caller()) {
                let producing = ctx.clone();
                let value = Arc::clone(&opened);
                let response = opened.response.clone();
                response
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            People::render(&producing, conn, reader, &value.value).await
                        })
                    })
                    .await?
                    .ok_or_else(refused)?
            } else {
                People::render(ctx, conn, principal, &opened.value).await?
            };
            Ok((reply, ctx.clone(), principal.caller().clone(), opened))
        })
    })
    .await?;
    crate::key_operations::publication::retain(
        &producing,
        &accepted,
        (retained, opened),
        |ctx, conn, reader, (body, opened)| {
            Box::pin(check_sources(ctx, conn, reader, body, opened))
        },
    )?;
    Ok(reply)
}
