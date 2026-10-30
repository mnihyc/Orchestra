//! What one person holds on a thing and what gives it to them, the people behind a holder of its page, and what was done to it and by whom: the same three requests for a node, a resource and a document.
//!
//! Each is the engine's, asked of the kind of thing the path names, and put
//! into the words of the API here. A caller who may view a thing reads its
//! history and the people behind the holders its page shows; what another
//! person holds on it is for whoever manages its page, and for that person
//! themself. What a person holds inside a place through a role they hold above it is told as that
//! role: the entry that gives it names the role, and the place whose defaults hold the entry is
//! told beside it. A thing that does not exist is not found, and a kind that has no
//! page to read is answered plainly by [`subject`], never as a
//! fault of the server.

use super::page::{entries_named, same};
use super::subject::{self, Named};
use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::{invalid, view};
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::Through;
use orch_access_contract::pages as stored;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access as api;
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::nodes::NodeOrSystem;
use orch_api_contract::users::UserRef;
use orch_authz::engine::{Frame, Graph, Site};
use orch_authz::pages;
use orch_authz::things::{self, Thing};
use orch_authz::{Act, Caller, Policy, Principal, holders, nodes};
use orch_db::Conn;
use orch_db::queries::access;
use orch_db::queries::access::everything::{self, HistoryFilter};
use orch_db::queries::hierarchy::{self, AuditCursor, AuditRow};
use orch_engine::history;
use orch_types::{Id, Time};
use std::collections::HashSet;

/// The thing the request names, found, with where its page stands. A thing that is not there is
/// not found.
async fn found(conn: &mut Conn, named: &Named) -> ApiResult<(Graph, Site, Policy)> {
    let (graph, site) = things::site(conn, named.thing)
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => ApiError::not_found(),
            other => ApiError::internal(other),
        })?;
    let policy = subject::policy_of(conn, named.thing, &site).await?;
    Ok((graph, site, policy))
}

// ---- what one person holds ----

/// What one person holds on a thing and what gives it to them.
pub async fn effective(
    ctx: Ctx,
    query: api::EffectiveAccessGetQuery,
    (): (),
) -> ApiResult<api::EffectiveAccess> {
    let named = subject::named(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (graph, site, policy) = found(&mut conn, &named).await?;
    if query.user_id == principal.user() {
        subject::require_page(&mut conn, &principal, named.thing, policy).await?;
    } else {
        subject::require_manage(&mut conn, &principal, named.thing, policy).await?;
    }
    let person = if query.user_id == principal.user() {
        principal.clone()
    } else {
        Principal::load(
            &mut conn,
            Caller::session(query.user_id),
            Time::new(ctx.now),
        )
        .await
        .map_err(|_| ApiError::not_found())?
    };
    let held = things::effective_at(&mut conn, &person, graph, &site, policy)
        .await
        .map_err(ApiError::internal)?;
    let entry_ids: Vec<Id> = held
        .held
        .iter()
        .flat_map(|held| held.through.iter())
        .filter_map(|through| match through {
            Through::Entry(via) => Some(via.entry),
            _ => None,
        })
        .collect();
    let rows = access::entries_by_id(&mut conn, &entry_ids).await?;
    let entries: Vec<stored::Entry> = rows
        .iter()
        .filter_map(|row| orch_authz::engine::entry_of(row).ok())
        .collect();
    let described = entries_named(&mut conn, &principal, held.root, &entries).await?;
    // The defaults give a role one entry for each permission, and every entry of one holder has the
    // same chain: each holder's chain is found once, and all of them are named together.
    let mut paths: Vec<(stored::Holder, Vec<stored::Holder>)> = Vec::new();
    for entry in &entries {
        if paths.iter().any(|(holder, _)| *holder == entry.holder) {
            continue;
        }
        let path = orch_authz::holders::chain(&mut conn, &person, entry.holder, site.role_place)
            .await
            .map_err(ApiError::internal)?;
        paths.push((entry.holder, path));
    }
    let lists: Vec<Vec<stored::Holder>> = paths.iter().map(|(_, path)| path.clone()).collect();
    let lists_named =
        super::page::Names::holder_lists(&mut conn, &principal, held.root, site.role_place, &lists)
            .await?;
    let by_holder: std::collections::HashMap<stored::Holder, Vec<api::Holder>> = paths
        .into_iter()
        .map(|(holder, _)| holder)
        .zip(lists_named)
        .collect();
    let chains: std::collections::HashMap<Id, Vec<api::Holder>> = entries
        .iter()
        .filter_map(|entry| {
            by_holder
                .get(&entry.holder)
                .map(|named| (entry.id, named.clone()))
        })
        .collect();
    let user = refusals::user_refs(&mut conn, &[query.user_id])
        .await?
        .remove(&query.user_id)
        .ok_or_else(ApiError::not_found)?;
    let mut places: Vec<Id> = Vec::new();
    let mut container_ids: Vec<Id> = Vec::new();
    if let Some(role) = held.role
        && let PlaceRef::Node(place) = role.at
    {
        places.push(place);
    }
    for through in held.held.iter().flat_map(|held| held.through.iter()) {
        if let Through::Entry(via) = through {
            match via.from {
                stored::Origin::Node(node) | stored::Origin::Defaults(PlaceRef::Node(node)) => {
                    places.push(node);
                }
                stored::Origin::Own | stored::Origin::Defaults(PlaceRef::System) => {}
                stored::Origin::DocumentContainer(id) => {
                    container_ids.push(id);
                }
            }
        }
    }
    let refs = view::qualified_refs(&mut conn, &principal, &places, Policy::default()).await?;
    let container_refs =
        crate::routes::document_tree::container_refs(&mut conn, &principal, &container_ids).await?;
    let role_from = held.role.and_then(|role| match role.at {
        PlaceRef::Node(place) => refs.get(&place).cloned().map(NodeOrSystem::Node),
        PlaceRef::System => Some(NodeOrSystem::SYSTEM),
    });
    let permissions =
        held.held
            .iter()
            .map(|each| api::EffectivePermission {
                permission: each.permission.as_str().to_owned(),
                through: each
                    .through
                    .iter()
                    .filter_map(|through| match through {
                        Through::Owner => Some(api::AccessThrough::Owner),
                        Through::SourceOwner { .. } => Some(api::AccessThrough::SourceOwner),
                        Through::Level { of } => Some(api::AccessThrough::Level {
                            of: of.as_str().to_owned(),
                        }),
                        Through::Entry(via) => described.get(&via.entry).cloned().map(|entry| {
                            api::AccessThrough::Entry {
                                chain: chains.get(&via.entry).cloned().unwrap_or_default(),
                                entry,
                                from: match via.from {
                                    stored::Origin::Own => None,
                                    stored::Origin::DocumentContainer(id) => container_refs
                                        .get(&id)
                                        .cloned()
                                        .map(NodeOrSystem::DocumentContainer),
                                    stored::Origin::Node(node)
                                    | stored::Origin::Defaults(PlaceRef::Node(node)) => {
                                        refs.get(&node).cloned().map(NodeOrSystem::Node)
                                    }
                                    stored::Origin::Defaults(PlaceRef::System) => {
                                        Some(NodeOrSystem::SYSTEM)
                                    }
                                },
                            }
                        }),
                        Through::Admin => Some(api::AccessThrough::Admin {
                            from: role_from.clone(),
                        }),
                        Through::SystemAdmin => Some(api::AccessThrough::SystemAdmin),
                    })
                    .collect(),
            })
            .collect();
    Ok(api::EffectiveAccess {
        admin_only: held
            .admin_only
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect(),
        user,
        role: held.role.and_then(|held| same(&held.role)),
        role_from,
        administers: held.administers,
        permissions,
    })
}

// ---- the people behind a holder ----

/// What `holder` names in the form of a page chip: a role at a place, a group, the work in a place,
/// or the admins.
enum Chip {
    Held(stored::Holder),
    Admins,
    SystemAdmins,
}

fn parse_chip(text: &str) -> Option<Chip> {
    let mut parts = text.split(':');
    match (parts.next()?, parts.next(), parts.next(), parts.next()) {
        ("admins", None, ..) => Some(Chip::Admins),
        ("role", Some("system"), Some("admin"), None) => Some(Chip::SystemAdmins),
        ("role", Some("local"), Some(role), None) => Some(Chip::Held(stored::Holder::Role {
            role: role.parse().ok()?,
            at: None,
        })),
        ("role", Some(place), Some(role), None) => Some(Chip::Held(stored::Holder::Role {
            role: role.parse().ok()?,
            at: Some(place.parse().ok()?),
        })),
        ("group", Some(id), None, _) => Some(Chip::Held(stored::Holder::Group {
            id: id.parse().ok()?,
        })),
        ("members", Some(id), None, _) => Some(Chip::Held(stored::Holder::Members {
            id: id.parse().ok()?,
        })),
        ("node", Some(id), None, _) => Some(Chip::Held(stored::Holder::Node {
            id: id.parse().ok()?,
        })),
        _ => None,
    }
}

/// The people behind one holder that the page of the thing shows.
pub async fn people(
    ctx: Ctx,
    query: api::PermissionPeopleListQuery,
    (): (),
) -> ApiResult<api::PermissionPeopleListReply> {
    let named = subject::named(&ctx)?;
    let page_query = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page_query.limit()).unwrap_or(usize::MAX);
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (graph, site, policy) = found(&mut conn, &named).await?;
    subject::require_page(&mut conn, &principal, named.thing, policy).await?;
    let chip = parse_chip(&query.holder).ok_or_else(ApiError::not_found)?;
    let wanted = view::every_permission(std::slice::from_ref(&site));
    let frame = Frame::load(
        &mut conn,
        &principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &wanted,
    )
    .await
    .map_err(ApiError::internal)?;
    let page = pages::read(&mut conn, &principal, named.thing.target(), policy)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let shown = |holder: &stored::Holder| -> bool {
        let same_holder = |other: &stored::Holder| match (holder, other) {
            (
                stored::Holder::Role { role, at },
                stored::Holder::Role {
                    role: other_role,
                    at: other_at,
                },
            ) => role == other_role && at.or(site.role_place) == other_at.or(site.role_place),
            _ => holder == other,
        };
        page.owners.iter().any(|entry| same_holder(&entry.holder))
            || page.rows.iter().any(|row| {
                row.entries.iter().any(|entry| same_holder(&entry.holder))
                    || row
                        .inherited
                        .iter()
                        .flatten()
                        .any(|inherited| same_holder(&inherited.entry.holder))
            })
    };
    let people: Vec<Id> = match chip {
        Chip::Admins => {
            let mut people: Vec<Id> = holders::admins_at(&mut conn, &frame, &site.admin_places)
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .map(|(_, user)| user)
                .collect();
            people.extend(access::system_admin_people(&mut conn, holders::SCAN_MOST).await?);
            people
        }
        Chip::SystemAdmins => access::system_admin_people(&mut conn, holders::SCAN_MOST).await?,
        Chip::Held(holder) => {
            if !shown(&holder) {
                return Err(ApiError::not_found());
            }
            holders::people_of(&mut conn, &frame, &site, &holder)
                .await
                .map_err(ApiError::internal)?
        }
    };
    let mut distinct: Vec<Id> = people;
    distinct.sort_unstable();
    distinct.dedup();
    let described = refusals::user_refs(&mut conn, &distinct).await?;
    let mut listed: Vec<UserRef> = described.into_values().collect();
    listed.sort_by(|first, second| {
        (first.display_name.to_lowercase(), first.id)
            .cmp(&(second.display_name.to_lowercase(), second.id))
    });
    let start = match query.cursor.as_deref() {
        None => 0,
        Some(cursor) => {
            let last: Id = cursor
                .parse()
                .map_err(|_| invalid("cursor", "is not a cursor of this list"))?;
            listed
                .iter()
                .position(|user| user.id == last)
                .map_or(0, |at| at + 1)
        }
    };
    let rest: Vec<UserRef> = listed.into_iter().skip(start).collect();
    let more = rest.len() > limit;
    let items: Vec<UserRef> = rest.into_iter().take(limit).collect();
    // One native ownership expansion for the whole response, including nested owner groups.
    // A person's place in a Viewer chip says nothing about their highest access.
    let owner_people: HashSet<Id> = holders::owner_people(&mut conn, &frame, site.id, site.root)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .collect();
    Ok(api::PermissionPeopleListReply {
        owners: items
            .iter()
            .filter(|user| owner_people.contains(&user.id))
            .map(|user| user.id)
            .collect(),
        next_cursor: more
            .then(|| items.last().map(|user| user.id.to_string()))
            .flatten(),
        items,
    })
}

// ---- what was done to a thing ----

fn history_cursor(row: &AuditRow) -> String {
    format!("{}|{}", row.at.timestamp_micros(), row.id)
}

/// The kinds a comma list says, each a value of [`api::HistoryKind`].
fn kinds_of(text: &str) -> ApiResult<Vec<api::HistoryKind>> {
    text.split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| {
            same::<_, api::HistoryKind>(&word)
                .ok_or_else(|| invalid("kinds", &format!("{word} is no kind of line")))
        })
        .collect()
}

/// The thread a line is about.
fn thread_of(row: &AuditRow) -> Option<Id> {
    history::thread_of(&row.object_kind, row.object_id, &row.details)
}

/// The lines for the rows, newest first as they came, with who did each as the reader may know
/// them and what each points at; a line about a thread the reader may not view is left out.
async fn lines(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[AuditRow],
) -> ApiResult<Vec<api::HistoryLine>> {
    let threads: Vec<Id> = rows
        .iter()
        .filter_map(thread_of)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let viewable: HashSet<Id> = if threads.is_empty() {
        HashSet::new()
    } else {
        nodes::holding(
            conn,
            principal,
            &threads,
            exact(Permission::ThreadView),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|listed| listed.id)
        .collect()
    };
    let named_threads = view::refs(conn, principal, &threads, Policy::default()).await?;
    let users: Vec<Id> = rows
        .iter()
        .filter(|row| row.actor_kind == "user")
        .filter_map(|row| row.actor_id)
        .collect();
    let people = refusals::user_refs(conn, &users).await?;
    let target_ids: Vec<Id> = rows.iter().filter_map(|row| row.node_id).collect();
    let targets = view::refs(conn, principal, &target_ids, Policy::default()).await?;
    Ok(rows
        .iter()
        .filter(|row| thread_of(row).is_none_or(|thread| viewable.contains(&thread)))
        .map(|row| api::HistoryLine {
            id: row.id,
            at: Time::new(row.at),
            actor: match (row.actor_kind.as_str(), row.actor_id) {
                ("user", Some(user)) => people.get(&user).cloned().map(api::HistoryActor::User),
                ("agent", Some(thread)) => named_threads
                    .get(&thread)
                    .cloned()
                    .map(|agent| api::HistoryActor::Agent(api::HistoryAgent { agent })),
                _ => None,
            },
            action: row.action.clone(),
            text: row.summary.clone(),
            wording: history::wording_of(
                &row.action,
                &row.details,
                row.node_id.and_then(|node| targets.get(&node).cloned()),
            ),
            about: history::about_of(
                &row.details,
                thread_of(row).and_then(|thread| named_threads.get(&thread).cloned()),
            ),
            upload: row
                .details
                .get("upload")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
        })
        .collect())
}

/// What was done to a thing and by whom, newest first, for whoever may view it.
pub async fn history(
    ctx: Ctx,
    query: api::HistoryListQuery,
    (): (),
) -> ApiResult<Page<api::HistoryLine>> {
    let named = subject::named(&ctx)?;
    let page_query = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page_query.limit()).unwrap_or(usize::MAX);
    let after = match query.cursor.as_deref() {
        None => None,
        Some(text) => {
            let bad = || invalid("cursor", "is not a cursor of this list");
            let (micros, row) = text.split_once('|').ok_or_else(bad)?;
            Some(AuditCursor {
                at: micros
                    .parse::<i64>()
                    .ok()
                    .and_then(DateTime::<Utc>::from_timestamp_micros)
                    .ok_or_else(bad)?,
                id: row.parse().map_err(|_| bad())?,
            })
        }
    };
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (_, _, policy) = found(&mut conn, &named).await?;
    subject::require_view(&mut conn, &principal, named.thing, policy).await?;
    let batch = i64::try_from(limit + 1).unwrap_or(i64::MAX);
    let summary = query.depth == Some(orch_api_contract::nodes::ActivityDepth::Summary);
    let meaningful: Option<Vec<String>> = summary.then(|| {
        orch_access_contract::audit::AuditAction::ALL
            .iter()
            .copied()
            .filter(|action| crate::routes::nodes::activity::meaningful(*action))
            .map(|action| action.as_str().to_owned())
            .collect()
    });
    let mut rows = match named.thing {
        Thing::Node(id) => {
            hierarchy::node_history(
                &mut conn,
                id,
                query.actor,
                after,
                batch,
                meaningful.as_deref(),
            )
            .await?
        }
        Thing::Other(object) => {
            let environment = object.kind == ObjectKind::Environment;
            let mut actions: Option<Vec<String>> = match (environment, query.kinds.as_deref()) {
                (true, Some(kinds)) => Some(history::actions_of(&kinds_of(kinds)?)),
                _ => None,
            };
            if let Some(meaningful) = &meaningful {
                match &mut actions {
                    Some(actions) => actions.retain(|action| meaningful.contains(action)),
                    None => actions = Some(meaningful.clone()),
                }
            }
            everything::history_of(
                &mut conn,
                object.id,
                environment,
                &HistoryFilter {
                    actor: query.actor,
                    actions: actions.as_deref(),
                    thread: query.thread_id,
                    path: query.path.as_deref().filter(|_| environment),
                },
                after,
                batch,
            )
            .await?
        }
    };
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = more.then(|| rows.last().map(history_cursor)).flatten();
    let items = lines(&mut conn, &principal, &rows).await?;
    Ok(Page { next_cursor, items })
}
