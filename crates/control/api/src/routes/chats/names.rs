//! Typed live-name projection for chat replies and forwarded copies. Scope selection belongs to
//! the shared operation planner; these helpers only restrict the existing ordinary projections.

use super::{ApiError, ApiResult, Conn, Policy, Principal, refusals, view};
use orch_api_contract::chats::{ChatMentionTarget, ChatMessage, ChatPart, ChatRef};
use orch_api_contract::nodes::NodeRef;
use orch_authz::engine::Graph;
use orch_chat::{
    messages as stored_messages,
    views::{self, Lookups},
};
use orch_db::queries::{
    access,
    chats::{ChatRow, MessageRow},
};
use orch_types::Id;
use std::collections::{BTreeSet, HashMap};

/// A current protected label source, never a request-supplied authorization decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum NameSource {
    Node(Id),
    Group(Id),
}
pub(super) type AllowSet = BTreeSet<NameSource>;

/// The actual values used by the existing formatters, with their materialized protected sources.
#[derive(Debug, Clone)]
pub(super) struct Captured {
    pub lookups: Lookups,
    pub sources: AllowSet,
}
pub(super) struct MessageProjection {
    pub messages: Vec<ChatMessage>,
    pub sources: AllowSet,
}
pub(super) struct ChatReference {
    pub reference: ChatRef,
    pub sources: AllowSet,
}
pub(super) struct GroupNames {
    pub names: HashMap<Id, String>,
    pub sources: AllowSet,
}

#[derive(Default)]
struct Wanted {
    people: BTreeSet<Id>,
    nodes: BTreeSet<Id>,
    qualified: BTreeSet<Id>,
    groups: BTreeSet<Id>,
}
impl Wanted {
    fn of(rows: &[&MessageRow], parts: &[ChatPart], chat: Option<&ChatRow>) -> Self {
        let mut wanted = Self {
            people: views::people_of(rows).into_iter().collect(),
            nodes: views::threads_of(rows).into_iter().collect(),
            qualified: views::role_places_of(rows).into_iter().collect(),
            groups: views::groups_of(rows).into_iter().collect(),
        };
        for part in parts {
            if let ChatPart::Mention { target } = part {
                match target {
                    ChatMentionTarget::User { id } => {
                        wanted.people.insert(*id);
                    }
                    ChatMentionTarget::Agent { thread_id } => {
                        wanted.nodes.insert(*thread_id);
                    }
                    ChatMentionTarget::Role { node_id, .. } => {
                        wanted.qualified.insert(*node_id);
                    }
                    ChatMentionTarget::Group { id } => {
                        wanted.groups.insert(*id);
                    }
                }
            }
        }
        if let Some(chat) = chat {
            wanted.nodes.extend(chat.node_id);
            if chat.kind == orch_db::queries::chats::KIND_GROUP {
                wanted.nodes.insert(chat.home());
            }
        }
        wanted
    }
}

/// Actual current identities a stored message/reply or fresh typed mention could name. Callers
/// pass selected reply rows too; no placeholder MessageRow or historical snapshot traversal is used.
pub(super) async fn candidate_sources(
    conn: &mut Conn,
    rows: &[&MessageRow],
    parts: &[ChatPart],
    source_chat: Option<&ChatRow>,
) -> ApiResult<AllowSet> {
    let mut wanted = Wanted::of(rows, parts, source_chat);
    let groups =
        access::groups_full(conn, &wanted.groups.iter().copied().collect::<Vec<_>>()).await?;
    let mut sources: AllowSet = groups
        .iter()
        .map(|group| NameSource::Group(group.id))
        .collect();
    wanted
        .qualified
        .extend(groups.iter().map(|group| group.org_id));
    let seeds: Vec<_> = wanted
        .nodes
        .iter()
        .chain(&wanted.qualified)
        .copied()
        .collect();
    if seeds.is_empty() {
        return Ok(sources);
    }
    let graph = Graph::load(conn, &seeds)
        .await
        .map_err(ApiError::internal)?;
    sources.extend(
        wanted
            .nodes
            .iter()
            .filter(|id| graph.get(**id).is_some())
            .map(|id| NameSource::Node(*id)),
    );
    for id in wanted.qualified {
        if let Some(node) = graph.get(id) {
            sources.extend(node.path.iter().copied().map(NameSource::Node));
        }
    }
    Ok(sources)
}

fn allowed_nodes(allowed: &AllowSet) -> BTreeSet<Id> {
    allowed
        .iter()
        .filter_map(|source| match source {
            NameSource::Node(id) => Some(*id),
            NameSource::Group(_) => None,
        })
        .collect()
}

async fn node_refs(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    allowed: Option<&AllowSet>,
) -> ApiResult<(HashMap<Id, NodeRef>, AllowSet)> {
    let refs = match allowed {
        Some(allowed) => {
            view::refs_with_name_sources(
                conn,
                principal,
                ids,
                Policy::default(),
                &allowed_nodes(allowed),
            )
            .await?
        }
        None => view::refs(conn, principal, ids, Policy::default()).await?,
    };
    let sources = refs
        .values()
        .filter(|reference| reference.name.is_some() || reference.thread_display.is_some())
        .map(|reference| NameSource::Node(reference.id))
        .collect();
    Ok((refs, sources))
}

/// The existing qualifier over already bounded references. An unallowed leaf cannot be made
/// visible again by a removed-place placeholder or by an ordinary unbounded followup lookup.
async fn qualified_refs(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    allowed: Option<&AllowSet>,
) -> ApiResult<(HashMap<Id, NodeRef>, AllowSet)> {
    if ids.is_empty() {
        return Ok((HashMap::new(), AllowSet::new()));
    }
    let graph = Graph::load(conn, ids).await.map_err(ApiError::internal)?;
    let all: BTreeSet<_> = ids
        .iter()
        .filter_map(|id| graph.get(*id))
        .flat_map(|node| node.path.iter().copied())
        .collect();
    let (refs, _) = node_refs(
        conn,
        principal,
        &all.iter().copied().collect::<Vec<_>>(),
        allowed,
    )
    .await?;
    let mut qualified = HashMap::new();
    let mut sources = AllowSet::new();
    for id in ids {
        let Some(node) = graph.get(*id) else { continue };
        if allowed.is_some_and(|allowed| !allowed.contains(&NameSource::Node(*id))) {
            if let Some(reference) = refs.get(id) {
                qualified.insert(*id, reference.clone());
            }
            continue;
        }
        let Some(reference) = view::qualified_from(&graph, &refs, *id) else {
            continue;
        };
        if reference.thread_display.is_some() {
            sources.insert(NameSource::Node(*id));
        }
        if reference.name.is_some() {
            if node
                .path
                .iter()
                .any(|id| graph.get(*id).is_some_and(|node| node.deleted))
            {
                // The text is static, but it still discloses this referenced place's retained
                // lifecycle. Keep its admitted identity so replaying the bounded projection
                // preserves the ordinary fallback without enabling an unbound live name.
                sources.insert(NameSource::Node(*id));
            } else {
                sources.extend(
                    node.path
                        .iter()
                        .filter(|id| refs.get(id).is_some_and(|node| node.name.is_some()))
                        .copied()
                        .map(NameSource::Node),
                );
            }
        }
        qualified.insert(*id, reference);
    }
    Ok((qualified, sources))
}

/// Current group names retain their ordinary defining-org visibility gate. Qualified org labels
/// are independently bounded before formatting; an unavailable qualifier leaves the group name alone.
pub(super) async fn qualified_groups(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    allowed: Option<&AllowSet>,
) -> ApiResult<GroupNames> {
    let ids: Vec<_> = ids
        .iter()
        .copied()
        .filter(|id| allowed.is_none_or(|allowed| allowed.contains(&NameSource::Group(*id))))
        .collect();
    if ids.is_empty() {
        return Ok(GroupNames {
            names: HashMap::new(),
            sources: AllowSet::new(),
        });
    }
    let mut names = orch_chat::audiences::visible_group_names(conn, principal, &ids)
        .await
        .map_err(ApiError::internal)?;
    let rows = access::groups_full(conn, &names.keys().copied().collect::<Vec<_>>()).await?;
    let (places, mut sources) = qualified_refs(
        conn,
        principal,
        &rows.iter().map(|row| row.org_id).collect::<Vec<_>>(),
        allowed,
    )
    .await?;
    sources.extend(names.keys().copied().map(NameSource::Group));
    for group in rows {
        if let Some(place) = places
            .get(&group.org_id)
            .and_then(|place| place.name.as_deref())
        {
            names.insert(group.id, format!("{}, {place}", group.name));
        }
    }
    Ok(GroupNames { names, sources })
}

/// The same ordinary lookups with an optional trusted source bound. Public UserRefs are not
/// protected resource names and retain their established lookup path.
#[allow(
    clippy::too_many_arguments,
    reason = "existing lookup sets plus their role qualifiers and one trusted source bound"
)]
pub(super) async fn lookups(
    conn: &mut Conn,
    principal: &Principal,
    people: &[Id],
    nodes: &[Id],
    groups: &[Id],
    qualified: &[Id],
    allowed: Option<&AllowSet>,
) -> ApiResult<Captured> {
    let people = refusals::user_refs(conn, people).await?;
    let (mut nodes, mut sources) = node_refs(conn, principal, nodes, allowed).await?;
    let groups = qualified_groups(conn, principal, groups, allowed).await?;
    let (qualified, qualifiers) = qualified_refs(conn, principal, qualified, allowed).await?;
    // A qualified label replaces the same unqualified lookup. Record the sources of the value
    // actually retained, rather than a discarded earlier label (for example a removed-place fallback).
    for id in qualified.keys() {
        sources.remove(&NameSource::Node(*id));
    }
    nodes.extend(qualified);
    sources.extend(groups.sources);
    sources.extend(qualifiers);
    Ok(Captured {
        lookups: Lookups {
            reference_numbers: HashMap::new(),
            people,
            nodes,
            groups: groups.names,
        },
        sources,
    })
}

/// Capture names for actual stored rows and/or fresh typed parts. The caller's public identity is
/// available for a newly authored message; later ordinary recipient UserRefs may be loaded separately.
pub(super) async fn capture(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[&MessageRow],
    parts: &[ChatPart],
    source_chat: Option<&ChatRow>,
    allowed: Option<&AllowSet>,
) -> ApiResult<Captured> {
    let mut wanted = Wanted::of(rows, parts, source_chat);
    if !parts.is_empty() {
        wanted.people.insert(principal.user());
    }
    let mut captured = lookups(
        conn,
        principal,
        &wanted.people.into_iter().collect::<Vec<_>>(),
        &wanted.nodes.into_iter().collect::<Vec<_>>(),
        &wanted.groups.into_iter().collect::<Vec<_>>(),
        &wanted.qualified.into_iter().collect::<Vec<_>>(),
        allowed,
    )
    .await?;
    // These are actual selected stored rows, not page positions or inferred change sequences.
    // Use the same all-message numbering that reference preview and read_shared consume.
    let mut by_chat = std::collections::BTreeMap::<Id, Vec<Id>>::new();
    for row in rows {
        by_chat.entry(row.chat_id).or_default().push(row.id);
    }
    for (chat, mut ids) in by_chat {
        ids.sort_unstable();
        ids.dedup();
        captured
            .lookups
            .reference_numbers
            .extend(orch_db::queries::shares::chat_numbers(conn, chat, &ids).await?);
    }
    Ok(captured)
}

/// Full typed message projections, including their actual selected reply rows, with no new
/// protected lookup outside the supplied bound. Stored forwarded snapshots remain untouched.
pub(super) async fn messages(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
    rows: &[MessageRow],
    allowed: Option<&AllowSet>,
) -> ApiResult<MessageProjection> {
    let replies = stored_messages::replies_of(conn, chat, rows)
        .await
        .map_err(ApiError::internal)?;
    let all: Vec<_> = rows.iter().chain(replies.values()).collect();
    let captured = capture(conn, principal, &all, &[], None, allowed).await?;
    Ok(MessageProjection {
        messages: views::messages(rows, &replies, &captured.lookups),
        sources: captured.sources,
    })
}

/// Only the source ChatRef used by the copier: avoids loading discarded roster, preview and
/// inspection projections. The caller has already authorized the source chat itself.
pub(super) async fn chat_reference(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChatRow,
    allowed: Option<&AllowSet>,
) -> ApiResult<ChatReference> {
    let captured = capture(conn, principal, &[], &[], Some(row), allowed).await?;
    Ok(ChatReference {
        reference: views::chat_reference(row, &captured.lookups),
        sources: captured.sources,
    })
}
