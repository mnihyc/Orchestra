//! Current readers and complete role and group audiences for a deliberate chat mention.

use std::collections::{HashMap, HashSet};

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::Role;
use orch_api_contract::access::Role as ApiRole;
use orch_api_contract::chats::{ChatMentionTarget, ChatPart, MentionChoice};
use orch_authz::engine::{Frame, Graph};
use orch_authz::{Act, Policy, holders, nodes};
use orch_db::Conn;
use orch_db::queries::{access, chats as rows};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};

use crate::chats::Loaded;
use crate::{Acting, Error};

/// The shipped policy when no stored value is resolved.
const DEFAULT_GROUP_MENTIONS: &str = "writers";

/// A choice and the current people it actually names, before excluding its sender.
#[derive(Debug)]
pub struct Audience {
    /// The public choice, with its current distinct recipient count.
    pub choice: MentionChoice,
    /// Its current reader intersection, including a sender who belongs to it.
    pub users: Vec<Id>,
}

fn choice(
    target: ChatMentionTarget,
    name: String,
    users: &[Id],
    sender: Id,
) -> Result<MentionChoice, Error> {
    let count = i64::try_from(users.iter().filter(|id| **id != sender).count())
        .map_err(|_| Error::Internal("the audience count does not fit".into()))?;
    Ok(MentionChoice {
        user: None,
        target,
        name,
        count,
        available: true,
        reason: None,
        thread: None,
        agent: None,
        state: None,
        started_at: None,
    })
}

/// Every current participant who can read, including readers named outside the org.
pub async fn readers(conn: &mut Conn, act: &Acting<'_>, chat: &Loaded) -> Result<Vec<Id>, Error> {
    let candidates = if let Some(node) = chat.row.node_id {
        let graph = Graph::load(conn, &[node]).await?;
        let site = graph.site_of(node).ok_or(Error::NotFound)?;
        let frame = Frame::load(
            conn,
            act.principal,
            Policy::default(),
            graph,
            std::slice::from_ref(&site),
            &[Permission::ChatView],
        )
        .await?;
        holders::who_holds_for_notifications(conn, &frame, &site, Permission::ChatView).await?
    } else {
        chat.present()
    };
    let mut found = orch_authz::chats::readers_among(
        conn,
        &chat.row,
        &chat.members,
        &candidates,
        Time::new(act.now),
    )
    .await?;
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// Agent origins offered here, with current thread View applied before any names are read.
/// Local main threads are offered directly; elsewhere the agent still needs a live readable chat share.
pub async fn agent_threads(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: &Loaded,
) -> Result<Vec<orch_db::queries::threads::ThreadRow>, Error> {
    use orch_access_contract::permissions::PermissionName;
    use orch_authz::reach::{Agent, AgentReach, Reach, Things};
    let local = rows::posting::local_mention_origins(conn, chat.id()).await?;
    let mut candidates = rows::posting::origins(conn, chat.id(), act.now).await?;
    candidates.extend(local.iter().copied());
    candidates.sort_unstable();
    candidates.dedup();
    let visible = nodes::holding(
        conn,
        act.principal,
        &candidates,
        PermissionName::Exact(Permission::ThreadView),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let mut offered = Vec::new();
    for node in visible {
        let Some(thread) = orch_db::queries::threads::thread(conn, node.id).await? else {
            continue;
        };
        if thread.agent_id.is_none() {
            continue;
        }
        if !local.contains(&node.id) {
            let Some(agent) = Agent::of(conn, node.id).await? else {
                continue;
            };
            let things = Things {
                nodes: chat.row.node_id.into_iter().collect(),
                documents: Vec::new(),
                people_chats: chat
                    .row
                    .node_id
                    .is_none()
                    .then_some(chat.id())
                    .into_iter()
                    .collect(),
            };
            let reach =
                Reach::load(conn, agent, AgentReach::None, act.principal.now(), &things).await?;
            let standing = chat
                .row
                .node_id
                .map_or_else(|| reach.people_chat(chat.id()), |id| reach.node_chat(id));
            if !standing.reads() {
                continue;
            }
        }
        offered.push(thread);
    }
    Ok(offered)
}

/// The current followers who may read, frozen before writing or previewing a broad mention.
pub async fn followers(conn: &mut Conn, act: &Acting<'_>, chat: &Loaded) -> Result<Vec<Id>, Error> {
    let followed = crate::following::followers(conn, act, chat).await?;
    let candidates: Vec<Id> = followed
        .into_iter()
        .map(|row| row.user_id)
        .filter(|id| *id != act.user())
        .collect();
    let mut found = orch_authz::chats::readers_among(
        conn,
        &chat.row,
        &chat.members,
        &candidates,
        Time::new(act.now),
    )
    .await?;
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// Whether this writer may deliberately name the chat's roles and visible groups now.
pub async fn may_name_groups(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: &Loaded,
) -> Result<bool, Error> {
    if chat.row.kind == rows::KIND_DIRECT {
        return Ok(false);
    }
    let policy = orch_settings::get::<String>(
        conn,
        Place::Node(chat.row.node_id.unwrap_or(chat.row.org_id)),
        keys::CHAT_GROUP_MENTIONS,
    )
    .await?
    .unwrap_or_else(|| DEFAULT_GROUP_MENTIONS.into());
    Ok(match policy.as_str() {
        "writers" => true,
        "managers" if chat.row.node_id.is_some() => chat.standing.moderate,
        "managers" => chat.row.created_by == Some(act.user()),
        _ => return Err(Error::Internal("unknown group mention policy".into())),
    })
}

fn api_role(role: Role) -> ApiRole {
    match role {
        Role::Viewer => ApiRole::Viewer,
        Role::Interactor => ApiRole::Interactor,
        Role::Developer => ApiRole::Developer,
        Role::Admin => ApiRole::Admin,
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Viewer => "Viewers",
        Role::Interactor => "Interactors",
        Role::Developer => "Developers",
        Role::Admin => "Admins",
    }
}

async fn visible_org(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    org: Id,
) -> Result<bool, Error> {
    match nodes::check(
        conn,
        principal,
        org,
        Permission::OrgView,
        Act::Use,
        Policy::default(),
    )
    .await
    {
        Ok(decision) => Ok(decision.is_allowed()),
        Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::NotFound(_)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Group labels only where the caller may currently see their defining org.
pub async fn visible_group_names(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    ids: &[Id],
) -> Result<HashMap<Id, String>, Error> {
    let mut names = HashMap::new();
    for group in access::groups_full(conn, ids).await? {
        if visible_org(conn, principal, group.org_id).await? {
            names.insert(group.id, group.name);
        }
    }
    Ok(names)
}

/// Current people first and permitted nonempty role/group intersections second.
pub async fn list(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: &Loaded,
    only_followers: bool,
) -> Result<Vec<Audience>, Error> {
    chat.ensure_send()?;
    let people = if only_followers {
        followers(conn, act, chat).await?
    } else {
        readers(conn, act, chat).await?
    };
    let names = access::user_names(conn, &people).await?;
    let mut humans = Vec::new();
    for person in names {
        let users = vec![person.id];
        humans.push(Audience {
            choice: choice(
                ChatMentionTarget::User { id: person.id },
                person.name,
                &users,
                act.user(),
            )?,
            users,
        });
    }
    humans.sort_by(|a, b| {
        a.choice
            .name
            .to_lowercase()
            .cmp(&b.choice.name.to_lowercase())
            .then_with(|| a.users.cmp(&b.users))
    });
    if only_followers || !may_name_groups(conn, act, chat).await? {
        return Ok(humans);
    }
    let reader_set: HashSet<Id> = people.into_iter().collect();
    let scope = chat.row.node_id.unwrap_or(chat.row.org_id);
    let graph = Graph::load(conn, &[scope]).await?;
    let site = graph.site_of(scope).ok_or(Error::NotFound)?;
    let frame = Frame::load(
        conn,
        act.principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[Permission::ChatView],
    )
    .await?;
    // All roles at this place use the same ancestor Owners rows; resolve them once.
    let mut owners = holders::OwnersFound::default();
    for &role in Role::ALL {
        let mut users = holders::notification_holder_with(
            conn,
            &frame,
            &site,
            &Holder::Role {
                role,
                at: Some(scope),
            },
            &mut owners,
        )
        .await?;
        users.retain(|id| reader_set.contains(id));
        users.sort_unstable();
        users.dedup();
        if !users.is_empty() {
            humans.push(Audience {
                choice: choice(
                    ChatMentionTarget::Role {
                        node_id: scope,
                        role: api_role(role),
                    },
                    role_name(role).into(),
                    &users,
                    act.user(),
                )?,
                users,
            });
        }
    }
    let mut groups = Vec::new();
    for group in access::groups_of_orgs(conn, chat.row.org_id, true).await? {
        if visible_org(conn, act.principal, group.org_id).await? {
            groups.push(group);
        }
    }
    let ids: Vec<Id> = groups.iter().map(|group| group.id).collect();
    // This deliberate act resolves the whole audience, independently of roster pagination.
    let mut members: HashMap<Id, Vec<Id>> = HashMap::new();
    for (group, user) in access::group_people(conn, &ids, i64::MAX, act.now).await? {
        if reader_set.contains(&user) {
            members.entry(group).or_default().push(user);
        }
    }
    for group in groups {
        let mut users = members.remove(&group.id).unwrap_or_default();
        users.sort_unstable();
        users.dedup();
        if !users.is_empty() {
            humans.push(Audience {
                choice: choice(
                    ChatMentionTarget::Group { id: group.id },
                    group.name,
                    &users,
                    act.user(),
                )?,
                users,
            });
        }
    }
    Ok(humans)
}

/// The typed human targets' frozen recipients, refusing an invalid or unavailable audience.
pub async fn resolve(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: &Loaded,
    parts: &[ChatPart],
) -> Result<Vec<Id>, Error> {
    let targets: Vec<&ChatMentionTarget> = parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Mention { target } => Some(target),
            _ => None,
        })
        .collect();
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let broad = targets.iter().any(|target| {
        matches!(
            target,
            ChatMentionTarget::Role { .. } | ChatMentionTarget::Group { .. }
        )
    });
    if broad && chat.row.kind == rows::KIND_DIRECT {
        return Err(Error::invalid(
            "parts",
            "a direct chat has no role or group mention target",
        ));
    }
    if broad && !may_name_groups(conn, act, chat).await? {
        return Err(Error::MentionRestricted);
    }
    let audiences = list(conn, act, chat, false).await?;
    let agents = if targets
        .iter()
        .any(|target| matches!(target, ChatMentionTarget::Agent { .. }))
    {
        agent_threads(conn, act, chat).await?
    } else {
        Vec::new()
    };
    let mut users = Vec::new();
    for target in targets {
        if let ChatMentionTarget::Agent { thread_id } = target {
            if !agents.iter().any(|thread| thread.node_id == *thread_id) {
                return Err(Error::invalid(
                    "parts",
                    "this agent is not currently offered in this chat",
                ));
            }
            // Agent targets are explicit inputs, never human notification recipients.
            continue;
        }
        let audience = audiences
            .iter()
            .find(|audience| &audience.choice.target == target)
            .ok_or_else(|| {
                Error::invalid(
                    "parts",
                    "a mention target is invalid or is not currently readable here",
                )
            })?;
        users.extend(
            audience
                .users
                .iter()
                .copied()
                .filter(|id| *id != act.user()),
        );
    }
    users.sort_unstable();
    users.dedup();
    Ok(users)
}
