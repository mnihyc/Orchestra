//! Authorized transcript and node search, the search inside one conversation, and the finder's names.
//!
//! Names are checked in ranked batches, and paths are built with the same
//! redaction rules as the tree. A request sees one database snapshot, so a
//! permission change cannot race the content its answer carries.

mod content;
mod meaning;
mod stream;
mod words;

use crate::middleware::authorization::{self, refusals::user_refs};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, view};
use orch_api_contract::search::{
    NAME_SEARCH_LIMIT_DEFAULT, NAME_SEARCH_LIMIT_MAX, NameHit, NameKind, NameSearchList,
    NameSearchListQuery, NameSearchPage, SearchList, SearchListQuery,
};
use orch_authz::Policy;
use orch_db::queries::search::{self as rows, Names};
use orch_types::Id;
use serde::de::DeserializeOwned;

/// Names keep a two-character minimum; content streams have a separate short-gram entrance.
const NAME_QUERY_MIN_CHARS: usize = 2;

/// One extra page avoids a second round when half the candidates are hidden,
/// without evaluating hundreds of pages for the usual twenty-name request.
const NAME_BATCH_PAGES: i64 = 2;

/// At most four bulk access checks: a narrow reader never walks the installation to fill twenty rows.
pub const NAME_BATCHES_MAX: usize = 4;

const NAME_KINDS: &[&str] = &[
    "org",
    "project",
    "topic",
    "thread",
    "document",
    "document_container",
    "chat",
    "tag",
    "user",
    "group",
    "provider",
    "agent",
    "environment",
    "host",
    "template",
    "mcp_server",
    "mount",
    "machine",
    "skill",
];

/// Binds the requests that find things and what was written in them.
pub fn mount(registry: &mut Registry) {
    registry
        .on(NameSearchList, names)
        .on(orch_api_contract::search::DocumentRelatedList, related)
        .on(SearchList, content::search)
        .on_raw(orch_api_contract::search::SearchStreamList, stream::search);
}

/// The complete lookup, including locks and access checks, shares the agent search deadline.
const RELATED_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

async fn related(
    ctx: Ctx,
    query: orch_api_contract::common::PageQuery,
    (): (),
) -> ApiResult<orch_api_contract::search::RelatedItems> {
    tokio::time::timeout(RELATED_LOOKUP_TIMEOUT, related_page(ctx, query))
        .await
        .unwrap_or_else(|_| {
            Err(ApiError::internal(
                "The related lookup timed out. Try again or narrow its candidate setting.",
            ))
        })
}

async fn related_page(
    ctx: Ctx,
    query: orch_api_contract::common::PageQuery,
) -> ApiResult<orch_api_contract::search::RelatedItems> {
    let document = ctx.id("id")?;
    let continued = query
        .cursor
        .as_deref()
        .map(crate::routes::links::related_after)
        .transpose()?;
    let after = continued.as_ref().map(|(position, _)| *position);
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let mut cancellation = rows::cancel_on_drop(&ctx.app.db, &mut tx).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let page = ctx
        .app
        .engine
        .related_page_for_person(
            &mut tx,
            &principal,
            document,
            after,
            query.limit.map(|limit| {
                usize::try_from(limit.clamp(1, orch_api_contract::common::MAX_LIMIT)).unwrap_or(1)
            }),
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    if continued.is_some_and(|(_, fingerprint)| fingerprint != page.fingerprint) {
        return Err(invalid("cursor", "the related list changed; start again"));
    }
    let mut answer = page.answer;
    answer.next_cursor = page
        .next
        .map(|position| crate::routes::links::related_cursor(position, page.fingerprint))
        .transpose()?;
    let ids: Vec<_> = answer.items.iter().map(|item| item.id).collect();
    let items = orch_db::queries::related::items(&mut tx, &ids).await?;
    let nodes: Vec<_> = items.iter().map(|item| item.node_id).collect();
    let ways = view::ways(&mut tx, &principal, &nodes, Policy::default()).await?;
    let documents: Vec<_> = answer
        .items
        .iter()
        .filter(|item| item.kind == orch_api_contract::search::RelatedKind::Document)
        .map(|item| orch_engine::document_tree::Item::Document(item.id))
        .collect();
    let folders = orch_engine::document_tree::load_entries_in(
        &mut tx,
        orch_engine::document_tree::Actor::Person(&principal),
        &[],
        &documents,
        ctx.now.into(),
    )
    .await
    .map_err(ApiError::internal)?;
    let ancestors: std::collections::HashMap<_, _> = folders
        .ancestors
        .iter()
        .map(|ancestor| (ancestor.id, ancestor))
        .collect();
    for result in &mut answer.items {
        let Some(item) = items.iter().find(|item| item.source_id == result.id) else {
            continue;
        };
        if result.kind == orch_api_contract::search::RelatedKind::Chat
            && result.name.is_empty()
            && let Some(chat) = orch_db::queries::chats::chat(&mut tx, result.id).await?
        {
            let members = orch_db::queries::chats::members(&mut tx, chat.id).await?;
            if members
                .iter()
                .any(|member| member.active_at(ctx.now) && member.user_id != principal.user())
            {
                result.name =
                    crate::routes::chats::people_label(&mut tx, &principal, &chat, &members)
                        .await?;
            }
        }
        result.context = ways
            .get(&item.node_id)
            .into_iter()
            .flatten()
            .filter(|step| step.id != result.id)
            .map(|step| orch_api_contract::search::RelatedAncestor {
                id: step.id,
                name: step.name.clone(),
            })
            .collect();
        let mut parent = item.parent_container_id;
        let mut path = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = parent {
            if !seen.insert(id) {
                break;
            }
            let Some(folder) = ancestors.get(&id) else {
                break;
            };
            path.push(orch_api_contract::search::RelatedAncestor {
                id,
                name: folder.title.clone(),
            });
            parent = match folder.parent {
                orch_engine::document_tree::Directory::Container(id) => Some(id),
                _ => None,
            };
        }
        result.context.extend(path.into_iter().rev());
    }
    cancellation.disarm();
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(answer)
}

fn word<T: DeserializeOwned>(text: &str) -> ApiResult<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).map_err(ApiError::internal)
}

fn kinds(text: Option<&str>, allowed: &[&str]) -> ApiResult<Vec<String>> {
    let mut found: Vec<String> = match text {
        None => allowed.iter().map(|kind| (*kind).to_owned()).collect(),
        Some(text) if text.trim().is_empty() => Vec::new(),
        Some(text) => text.split(',').map(str::trim).map(str::to_owned).collect(),
    };
    if found.iter().any(|kind| !allowed.contains(&kind.as_str())) {
        return Err(invalid("kinds", "contains an unknown kind"));
    }
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// Resolve ordinary holders once per snapshot when the shared filter is requested.
async fn advanced_selection(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: &SearchListQuery,
    at: chrono::DateTime<chrono::Utc>,
) -> ApiResult<orch_types::Json> {
    let mut selection = serde_json::to_value(query).map_err(ApiError::internal)?;
    if query.shared_with_me == Some(true) {
        let holders = orch_authz::holders::held(conn, principal.user(), orch_types::Time::new(at))
            .await
            .map_err(ApiError::internal)?;
        let held = &holders.held;
        let holders: Vec<_> = held
            .kinds
            .iter()
            .zip(&held.ids)
            .zip(&held.roles)
            .map(|((kind, id), role)| serde_json::json!({"kind":kind,"id":id,"role":role}))
            .collect();
        selection["search_user"] = serde_json::json!(principal.user());
        selection["search_at"] = serde_json::json!(orch_types::Time::new(at));
        selection["search_holders"] = serde_json::json!(holders);
    }
    Ok(selection)
}

async fn names(ctx: Ctx, query: NameSearchListQuery, (): ()) -> ApiResult<NameSearchPage> {
    let limit = query.limit.unwrap_or(NAME_SEARCH_LIMIT_DEFAULT);
    if limit == 0 || limit > NAME_SEARCH_LIMIT_MAX {
        return Err(invalid("limit", "is outside the names page limits"));
    }
    let batch = i64::from(limit.max(NAME_SEARCH_LIMIT_DEFAULT)) * NAME_BATCH_PAGES;
    let limit = usize::try_from(limit).map_err(ApiError::internal)?;
    let kinds = kinds(query.kinds.as_deref(), NAME_KINDS)?;
    let text = query.q.as_deref().unwrap_or_default().trim();
    if !text.is_empty() && text.chars().count() < NAME_QUERY_MIN_CHARS {
        return Err(invalid(
            "q",
            "type at least two characters, or leave it empty for recent visits",
        ));
    }
    if !text.is_empty() && rows::Query::parse(text).terms().is_empty() {
        return Err(invalid("q", "give at least one word or quoted phrase"));
    }
    let mut tx = ctx.app.db.begin().await?;
    rows::snapshot(&mut tx).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    // The identities the roots are found from, as kept between requests (a system admin's roots are all of them).
    let held = if principal.is_system_admin() {
        None
    } else {
        Some(
            orch_authz::holders::held(&mut tx, principal.user(), orch_types::Time::new(ctx.now))
                .await
                .map_err(ApiError::internal)?,
        )
    };
    let roots = rows::name_roots(
        &mut tx,
        principal.user(),
        principal.is_system_admin(),
        ctx.now,
        held.as_deref().map(|sets| &sets.held),
    )
    .await?;
    let mut offset = 0;
    let mut selected = Vec::new();
    let mut admin_places = std::collections::HashMap::new();
    let mut participation = std::collections::HashSet::new();
    let mut partial = false;
    for _ in 0..NAME_BATCHES_MAX {
        let candidates = rows::names(
            &mut tx,
            &Names {
                identifiers: None,
                starred: query.starred.unwrap_or(false),
                at: ctx.now,
                user: principal.user(),
                trees: principal.member_of(),
                roots: &roots,
                system_admin: principal.is_system_admin(),
                text,
                kinds: &kinds,
                org: query.org_id,
                offset,
                limit: batch,
            },
        )
        .await?;
        let chat_ids: Vec<Id> = candidates
            .iter()
            .filter(|row| row.kind == "chat")
            .map(|row| row.id)
            .collect();
        let chats = orch_db::queries::chats::chats(&mut tx, &chat_ids).await?;
        let members = rows::participant_chats(&mut tx, principal.user(), &chat_ids).await?;
        let searchable_chats: std::collections::HashSet<Id> = chats
            .iter()
            .filter(|chat| {
                chat.node_id.is_some()
                    || members.iter().any(|member| {
                        member.id == chat.id
                            && !principal.suspension().applies(&[member.org_id])
                            && (principal.is_in_tree(member.org_id) || member.guest)
                            && principal
                                .caller()
                                .permits(orch_access_contract::permissions::Permission::ChatView)
                            && principal.caller().reaches(Some(&[member.org_id]))
                    })
            })
            .map(|chat| chat.id)
            .collect();
        let count = i64::try_from(candidates.len()).map_err(ApiError::internal)?;
        let visible = orch_engine::projections::visible_names(&mut tx, &principal, &candidates)
            .await
            .map_err(ApiError::internal)?;
        admin_places.extend(
            visible
                .admin_access
                .iter()
                .map(|(key, place)| (key.clone(), *place)),
        );
        participation.extend(visible.participating.iter().cloned());
        selected.extend(candidates.into_iter().filter(|row| {
            visible.contains(&(row.kind.clone(), row.id))
                && (row.kind != "chat" || searchable_chats.contains(&row.id))
                && (!query.participating.unwrap_or(false)
                    || visible.participating.contains(&(row.kind.clone(), row.id)))
        }));
        partial = count == batch || selected.len() > limit;
        if selected.len() >= limit || count < batch {
            break;
        }
        offset += count;
    }
    selected.truncate(limit);
    let admin_ids: Vec<Id> = selected
        .iter()
        .filter_map(|row| {
            admin_places
                .get(&(row.kind.clone(), row.id))
                .and_then(|reach| reach.place.node())
        })
        .collect();
    let admin_refs =
        view::qualified_refs(&mut tx, &principal, &admin_ids, Policy::default()).await?;
    let tag_ids: Vec<Id> = selected
        .iter()
        .filter(|row| row.kind == "tag")
        .map(|row| row.id)
        .collect();
    let tag_rows = orch_db::queries::hierarchy::tags_by_id(&mut tx, &tag_ids).await?;
    let defining: Vec<Id> = tag_rows.iter().map(|tag| tag.node_id).collect();
    let tag_places = view::refs(&mut tx, &principal, &defining, Policy::default()).await?;
    let tags: std::collections::HashMap<Id, orch_api_contract::nodes::Tag> = tag_rows
        .into_iter()
        .filter_map(|tag| {
            tag_places.get(&tag.node_id).cloned().map(|node| {
                (
                    tag.id,
                    orch_api_contract::nodes::Tag {
                        id: tag.id,
                        node,
                        name: tag.name,
                        color: u32::try_from(tag.color).unwrap_or_default(),
                    },
                )
            })
        })
        .collect();
    let place_ids: Vec<Id> = selected
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "org" | "project" | "topic" | "thread"))
        .map(|row| row.id)
        .collect();
    let colours = if place_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        orch_db::queries::hierarchy::details(&mut tx, &place_ids, ctx.now)
            .await?
            .into_iter()
            .map(|row| {
                (
                    row.id,
                    (
                        row.color.and_then(|value| u32::try_from(value).ok()),
                        row.custom_color,
                    ),
                )
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let paths: Vec<Id> = selected
        .iter()
        .filter(|row| {
            matches!(
                row.kind.as_str(),
                "org"
                    | "project"
                    | "topic"
                    | "thread"
                    | "document"
                    | "document_container"
                    | "chat"
                    | "tag"
            )
        })
        .filter_map(|row| row.node_id)
        .collect();
    let ways = view::ways(&mut tx, &principal, &paths, Policy::default()).await?;
    let mut people: Vec<Id> = selected.iter().filter_map(|row| row.owner_id).collect();
    people.extend(
        selected
            .iter()
            .filter(|row| row.kind == "user")
            .map(|row| row.id),
    );
    let people = user_refs(&mut tx, &people).await?;
    let thread_ids: Vec<Id> = selected
        .iter()
        .filter(|row| row.kind == "thread")
        .map(|row| row.id)
        .collect();
    let attention = orch_db::queries::source::attentions(&mut tx, &thread_ids)
        .await?
        .into_iter()
        .map(|(id, value)| {
            serde_json::from_value::<orch_api_contract::source::ThreadAttention>(value)
                .map(|value| (id, value))
                .map_err(ApiError::internal)
        })
        .collect::<ApiResult<std::collections::HashMap<_, _>>>()?;
    let found = selected
        .into_iter()
        .map(|row| {
            let person = (row.kind == "user").then(|| people.get(&row.id)).flatten();
            let path = if matches!(
                row.kind.as_str(),
                "org"
                    | "project"
                    | "topic"
                    | "thread"
                    | "document"
                    | "document_container"
                    | "chat"
                    | "tag"
            ) {
                row.node_id
                    .and_then(|node| ways.get(&node).cloned())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let href = super::destinations::href(
                &row.kind,
                &row.id.to_string(),
                row.node_id,
                row.org_id,
                &path,
                person.map(|user| user.username.as_str()),
            );
            Ok(NameHit {
                thread_display: path
                    .iter()
                    .find(|part| part.id == row.id)
                    .and_then(|part| part.thread_display.as_deref().cloned()),
                href,
                state: row.state,
                last_activity: row.last_activity.map(orch_types::Time::new),
                participating: participation.contains(&(row.kind.clone(), row.id)),
                starred: row.starred,
                admin_access: admin_places
                    .get(&(row.kind.clone(), row.id))
                    .and_then(|place| view::admin_access(*place, &admin_refs)),
                thread_attention: attention.get(&row.id).cloned(),
                kind: word::<NameKind>(&row.kind)?,
                id: row.id,
                name: row.name,
                color: tags
                    .get(&row.id)
                    .map(|tag| tag.color)
                    .or_else(|| colours.get(&row.id).and_then(|(color, _)| *color)),
                tag: tags.get(&row.id).cloned(),
                custom_color: colours.get(&row.id).and_then(|(_, own)| own.clone()),
                username: person.map(|user| user.username.clone()),
                avatar: person.map(|user| user.avatar.clone()),
                address: row.address,
                path,
                owner: row.owner_id.and_then(|id| people.get(&id).cloned()),
                archived: row.archived,
                recent: row.visited_at.is_some(),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(NameSearchPage {
        items: found,
        partial,
    })
}
