//! Chat access by a node's page, by membership or guest status within a root org, and by inspection for whoever holds that row on an org.
//!
//! A node's chat is an area of its node's page like any other: whoever holds
//! `chat.view` there reads it, `chat.interact` writes in it, and `chat.manage`
//! deletes what others wrote. It is read-only while the node is archived, and
//! is closed to writing where `chat.enabled` resolves false. A direct or a
//! group chat belongs to a root org and has no page: it is read and written by
//! the people in it who are still members of that org, and by a guest of a
//! group, and it is read, and only read, by whoever holds `directs.view` on
//! the root org, or on an org inside it which everybody in the chat is a member
//! of.
//!
//! [`standing`] says all of it for one caller and one chat: how they read it,
//! whether they may write in it and why not, whether they may moderate it.
//! What settings say (`chat.enabled`, `chat.direct`) is given in as a
//! [`ChatPolicy`], because settings are not read here.

use crate::engine::{Frame, Graph};
use crate::external::{self, Naming};
use crate::{Caller, Error, Policy, Principal};
use orch_access_contract::decisions::{ObjectRef, Through};
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::PlaceRef;
use orch_db::Conn;
use orch_db::queries::chats::{self as rows, ChatRow, MemberRow};
use orch_types::{Id, Time};
use std::collections::HashMap;

/// A people's chat read by an org's inspection row, without membership or subscription.
#[derive(Debug, Clone, Copy)]
pub struct PeopleChatAccess {
    /// View exists only through the enabled native reach.
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    /// Native Manage is available even when an ordinary inspection row already gave View.
    pub admin_place: Option<PlaceRef>,
}

/// Native administration reaches a private group; ordinary home View or moderation never joins it.
#[must_use]
pub fn group_administration(frame: &Frame<'_>, chat: &ChatRow) -> Option<PeopleChatAccess> {
    if chat.kind != rows::KIND_GROUP {
        return None;
    }
    let site = frame.graph().site_of(chat.home())?;
    if frame.principal().suspension().applies(&site.orgs)
        || frame.ways(&site, Permission::ChatManage).is_empty()
    {
        return None;
    }
    let place = frame.admin_place(&site)?;
    Some(PeopleChatAccess {
        admin_access: Some(orch_access_contract::roles::AdminReach {
            place,
            private: true,
        }),
        admin_place: Some(place),
    })
}

/// A chat's ordinary participation and native-only View, for every compact result.
#[derive(Debug, Clone, Copy)]
pub struct ListedChatAccess {
    /// The same marker as an opened chat; never inherited by an agent.
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    /// Explicit participation at the node or a current follow of this chat.
    pub participating: bool,
}

/// One authority frame for a whole set of node and people's chats. Lists, the
/// finder and search use the same result; no per-chat permission request is made.
pub async fn listed_access(
    conn: &mut Conn,
    principal: &Principal,
    chats: &[ChatRow],
) -> Result<HashMap<Id, ListedChatAccess>, Error> {
    if chats.is_empty() {
        return Ok(HashMap::new());
    }
    let people: Vec<_> = chats
        .iter()
        .filter(|chat| chat.node_id.is_none())
        .map(|chat| chat.id)
        .collect();
    let roots: Vec<_> = chats
        .iter()
        .filter(|chat| chat.node_id.is_none())
        .map(|chat| chat.org_id)
        .collect();
    let mut nodes = rows::orgs_of_trees(conn, &roots).await?;
    nodes.extend(
        chats
            .iter()
            .flat_map(|chat| std::iter::once(chat.org_id).chain(Some(chat.home()))),
    );
    let graph = Graph::load(conn, &nodes).await?;
    let sites: Vec<_> = graph
        .nodes()
        .filter_map(|node| graph.site_of(node.id))
        .collect();
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        &sites,
        &[
            Permission::ChatView,
            Permission::ChatManage,
            Permission::DirectsView,
        ],
    )
    .await?;
    let inspection = people_chat_access_in(conn, &frame, &people).await?;
    let members =
        orch_db::queries::search::participant_chats(conn, principal.user(), &people).await?;
    let mut found = HashMap::new();
    for chat in chats {
        if let Some(node) = chat.node_id {
            if let Some(site) = frame.graph().site_of(node)
                && !frame.ways(&site, Permission::ChatView).is_empty()
            {
                found.insert(
                    chat.id,
                    ListedChatAccess {
                        admin_access: frame.admin_access_for(&site, Permission::ChatView),
                        participating: frame.participates_chat(&site),
                    },
                );
            }
        } else {
            let Some(root) = frame.graph().site_of(chat.home()) else {
                continue;
            };
            if principal.suspension().applies(&root.orgs)
                || !principal.caller().reaches(root.path.as_deref())
            {
                continue;
            }
            let member_access = members.iter().any(|member| {
                member.id == chat.id
                    && ordinary_participant(principal, &root, member, Permission::ChatView)
            });
            let public = public_reader(&frame, chat);
            let administration = group_administration(&frame, chat);
            if member_access
                || public
                || administration.is_some()
                || inspection.contains_key(&chat.id)
            {
                found.insert(
                    chat.id,
                    ListedChatAccess {
                        admin_access: if member_access
                            || public
                            || inspection
                                .get(&chat.id)
                                .is_some_and(|access| access.admin_access.is_none())
                        {
                            None
                        } else {
                            administration
                                .as_ref()
                                .or_else(|| inspection.get(&chat.id))
                                .and_then(|access| access.admin_access)
                        },
                        participating: member_access && frame.follows_chat(chat.id),
                    },
                );
            }
        }
    }
    Ok(found)
}

/// The ordinary membership right, shared by inventories and current agent-loan checks.
pub(crate) fn ordinary_participant(
    principal: &Principal,
    root: &crate::engine::Site,
    member: &orch_db::queries::search::ParticipantChat,
    permission: Permission,
) -> bool {
    !principal.suspension().applies(&root.orgs)
        && principal.caller().permits(permission)
        && principal.caller().reaches(root.path.as_deref())
        && member
            .expires_at
            .is_none_or(|end| end > principal.now().get())
        && (principal.is_in_tree(member.org_id) || member.guest)
}

/// All ordinary or native inspection access in a batched frame. Every current
/// member must lie inside the qualifying department; a root covers its tree.
pub async fn people_chat_access(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> Result<HashMap<Id, PeopleChatAccess>, Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let chats = rows::chats(conn, ids).await?;
    let roots: Vec<_> = chats.iter().map(|chat| chat.org_id).collect();
    let places = rows::orgs_of_trees(conn, &roots).await?;
    let graph = Graph::load(conn, &places).await?;
    let sites: Vec<_> = places
        .iter()
        .filter_map(|place| graph.site_of(*place))
        .collect();
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        &sites,
        &[Permission::DirectsView],
    )
    .await?;
    people_chat_access_in(conn, &frame, ids).await
}

/// The same common-domain query using a frame already prepared for an inventory.
pub async fn people_chat_access_in(
    conn: &mut Conn,
    frame: &Frame<'_>,
    ids: &[Id],
) -> Result<HashMap<Id, PeopleChatAccess>, Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let administered: HashMap<_, _> = frame
        .graph()
        .nodes()
        .filter_map(|node| {
            let site = frame.graph().site_of(node.id)?;
            (node.kind == orch_access_contract::roles::NodeKind::Org
                && !frame.principal().suspension().applies(&site.orgs)
                && !frame.ways(&site, Permission::DirectsView).is_empty())
            .then(|| {
                (
                    node.id,
                    PeopleChatAccess {
                        admin_access: frame.admin_access_for(&site, Permission::DirectsView).map(
                            |access| orch_access_contract::roles::AdminReach {
                                private: true,
                                ..access
                            },
                        ),
                        admin_place: frame.admin_place(&site),
                    },
                )
            })
        })
        .collect();
    let places: Vec<_> = administered.keys().copied().collect();
    let mut found: HashMap<Id, PeopleChatAccess> = HashMap::new();
    for (chat, org) in rows::people_chats_in(conn, ids, &places).await? {
        if let Some(access) = administered.get(&org) {
            found
                .entry(chat)
                .and_modify(|previous| {
                    if previous.admin_place.is_none() && access.admin_place.is_some() {
                        *previous = *access;
                    }
                    // Any ordinary inspection row makes the object readable with the mode off.
                    if access.admin_access.is_none() {
                        previous.admin_access = None;
                    }
                })
                .or_insert(*access);
        }
    }
    Ok(found)
}

/// What the settings say about a chat, resolved by whoever asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatPolicy {
    /// `chat.enabled` as it resolves at the node of a node's chat. Ignored for
    /// a direct or a group chat.
    pub enabled: bool,
    /// `chat.direct` resolves to `members` at the chat's root org: people may
    /// start direct and group chats and write in them. Ignored for a node's
    /// chat.
    pub direct: bool,
}

impl Default for ChatPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            direct: true,
        }
    }
}

/// How a person reads a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// By the page of the node it belongs to.
    Page,
    /// Reading a public group through its actual home, before joining it.
    Public,
    /// As somebody who is in it and still a member of its root org.
    Member,
    /// As a guest of a group: in it, and no member of the org.
    Guest,
    /// As an admin who is not in it: they hold `directs.view` on this org.
    Inspector {
        /// The org on which they hold it.
        org: Id,
    },
}

/// Why a person cannot write in a chat they read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cannot {
    /// The node, or the root org, is archived.
    Archived {
        /// The archived node, the nearest one on the way up.
        node: Id,
    },
    /// Chats are off where the node lies (`chat.enabled`).
    Disabled,
    /// Direct and group chats are off in the root org (`chat.direct`).
    DirectOff,
    /// The other person of a direct chat is suspended or has left the org.
    Closed,
    /// The page does not give them `chat.interact`.
    NoPermission,
    /// They read it as an admin and are not in it.
    Inspecting,
}

/// What a person may do with a chat, now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// The native administration source even when Read is ordinary.
    pub admin_place: Option<PlaceRef>,
    /// Native reach alone gives Read, using the same ordinary-frame comparison as other things.
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    /// Held chat permissions absent from ordinary access.
    pub admin_only: Vec<Permission>,
    /// Explicit participation at the node, or a current follow of this chat.
    pub participating: bool,
    /// How they read it; none when they may not: it does not exist for them.
    pub reading: Option<Reading>,
    /// Ordinary public-home eligibility before membership, independent of native inspection.
    pub join: bool,
    /// Whether they may write in it, or why not.
    pub sending: Result<(), Cannot>,
    /// Whether they may delete what others wrote.
    pub moderate: bool,
    /// Whether this reader currently manages a group or its chat area.
    pub manage: bool,
    /// Whether they may take it away as a file: they read a node's chat and hold its Export row (`chat.export`), or
    /// they are in a direct or a group chat. Somebody who only inspects one as an admin may not.
    pub export: bool,
    /// Whether they are in a direct or a group chat now.
    pub in_chat: bool,
    /// For a node's chat: whether they may know the node's name, which they
    /// may when they hold anything on it.
    pub names_node: bool,
}

impl Standing {
    /// Whether they read the chat at all.
    #[must_use]
    pub fn reads(&self) -> bool {
        self.reading.is_some()
    }

    /// Whether they read it as an admin who is not in it.
    #[must_use]
    pub fn inspects(&self) -> bool {
        matches!(self.reading, Some(Reading::Inspector { .. }))
    }
}

fn nothing() -> Standing {
    Standing {
        admin_place: None,
        admin_access: None,
        admin_only: Vec::new(),
        participating: false,
        reading: None,
        join: false,
        sending: Err(Cannot::NoPermission),
        moderate: false,
        manage: false,
        export: false,
        in_chat: false,
        names_node: false,
    }
}

/// What `principal` may do with `chat`, whose people are `members` (every row
/// of the chat, those who left included).
///
/// A caller who is suspended where the chat lies, and who is in it or in its
/// tree, gets [`Error::Suspended`]; the page of a node's chat decides that for
/// the node.
pub async fn standing(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
    members: &[MemberRow],
    policy: ChatPolicy,
) -> Result<Standing, Error> {
    match chat.node_id {
        Some(node) => node_standing(conn, principal, node, policy).await,
        None => {
            let members = rows::effective_members(members, principal.now().get());
            people_standing(conn, principal, chat, &members, policy).await
        }
    }
}

async fn node_standing(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: ChatPolicy,
) -> Result<Standing, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    let site = graph.site_of(node).ok_or(Error::NotFound(node))?;
    if let Some(closing) = principal.suspension().closing(&site.orgs) {
        return Err(Error::Suspended { org: closing.org() });
    }
    let everything = crate::nodes::every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    let holds = |permission| !frame.ways(&site, permission).is_empty();
    let reads = holds(Permission::ChatView);
    if !reads {
        return Ok(Standing {
            names_node: frame.holds_any(&site),
            ..nothing()
        });
    }
    // Permission before archive, and archive before the switch, so that
    // nobody learns of either who would be refused in any case.
    let sending = if !holds(Permission::ChatInteract) {
        Err(Cannot::NoPermission)
    } else if let Some(node) = site.archived_node {
        Err(Cannot::Archived { node })
    } else if !policy.enabled {
        Err(Cannot::Disabled)
    } else {
        Ok(())
    };
    Ok(Standing {
        reading: Some(Reading::Page),
        admin_place: frame.admin_place(&site),
        admin_access: frame.admin_access_for(&site, Permission::ChatView),
        admin_only: frame
            .admin_only(&site)
            .into_iter()
            .filter(|permission| permission.area() == orch_access_contract::permissions::Area::Chat)
            .collect(),
        participating: frame.participates_chat(&site),
        join: false,
        sending,
        moderate: holds(Permission::ChatManage) && site.archived_node.is_none(),
        manage: holds(Permission::ChatManage) && site.archived_node.is_none(),
        // Taking a chat away only reads it: it stands under an archived node as reading does.
        export: holds(Permission::ChatExport),
        in_chat: false,
        names_node: frame.holds_any(&site),
    })
}

async fn people_standing(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
    members: &[MemberRow],
    policy: ChatPolicy,
) -> Result<Standing, Error> {
    let root = chat.org_id;
    let me = principal.user();
    let mine = members.iter().find(|row| row.user_id == me && row.is_in());
    let in_tree = principal.is_in_tree(root);
    let graph = Graph::load(conn, &[chat.home()]).await?;
    let site = graph
        .site_of(chat.home())
        .ok_or(Error::NotFound(chat.home()))?;
    if let Some(closing) = principal.suspension().closing(&site.orgs)
        && (mine.is_some() || in_tree)
    {
        return Err(Error::Suspended { org: closing.org() });
    }
    // Membership grants the group's chat; legacy subtree ceilings follow its actual home.
    let caller = principal.caller();
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[Permission::ChatView, Permission::ChatManage],
    )
    .await?;
    let administration = group_administration(&frame, chat);
    let manager = chat.kind == rows::KIND_GROUP && mine.is_some_and(|member| member.manager);
    let token_reads = caller.permits(Permission::ChatView) && caller.reaches(site.path.as_deref());
    let reading = match mine {
        Some(_) if in_tree && token_reads => Some(Reading::Member),
        Some(row) if row.guest && !in_tree && token_reads => Some(Reading::Guest),
        _ if public_reader(&frame, chat) => Some(Reading::Public),
        _ => None,
    };
    let Some(reading) = reading else {
        let org = match administration {
            Some(access) => Some((root, access.admin_place)),
            None => inspector_org(conn, principal, chat, members).await?,
        };
        let ordinary_reads = if principal.caller().as_admin() {
            let ordinary = principal.ordinary(conn).await?;
            inspector_org(conn, &ordinary, chat, members)
                .await?
                .is_some()
        } else {
            org.is_some()
        };
        return Ok(match org {
            Some((org, native_place)) => Standing {
                admin_place: native_place,
                admin_access: if ordinary_reads {
                    None
                } else {
                    native_place.map(|place| orch_access_contract::roles::AdminReach {
                        place,
                        private: true,
                    })
                },
                admin_only: if native_place.is_some() {
                    [
                        Permission::ChatView,
                        Permission::ChatInteract,
                        Permission::ChatManage,
                        Permission::ChatExport,
                    ]
                    .into_iter()
                    .filter(|permission| match permission {
                        Permission::ChatView => !ordinary_reads,
                        Permission::ChatInteract => policy.direct && site.archived_node.is_none(),
                        Permission::ChatManage => site.archived_node.is_none(),
                        _ => true,
                    })
                    .collect()
                } else {
                    Vec::new()
                },
                participating: false,
                reading: Some(Reading::Inspector { org }),
                join: false,
                sending: if native_place.is_none() && administration.is_none() {
                    Err(Cannot::Inspecting)
                } else if !policy.direct {
                    Err(Cannot::DirectOff)
                } else if let Some(node) = site.archived_node {
                    Err(Cannot::Archived { node })
                } else {
                    Ok(())
                },
                moderate: (administration.is_some() || native_place.is_some())
                    && site.archived_node.is_none(),
                manage: chat.kind == rows::KIND_GROUP
                    && (administration.is_some() || native_place.is_some())
                    && site.archived_node.is_none(),
                export: native_place.is_some() || administration.is_some(),
                in_chat: false,
                names_node: false,
            },
            None => nothing(),
        });
    };
    let in_chat = matches!(reading, Reading::Member | Reading::Guest);
    let sending = if !in_chat || !caller.permits(Permission::ChatInteract) {
        Err(Cannot::NoPermission)
    } else if let Some(node) = site.archived_node {
        Err(Cannot::Archived { node })
    } else if chat.kind == rows::KIND_DIRECT && other_is_gone(conn, chat, members, me).await? {
        Err(Cannot::Closed)
    } else if !policy.direct {
        Err(Cannot::DirectOff)
    } else {
        Ok(())
    };
    let admin_place = if let Some(administration) = administration {
        administration.admin_place
    } else if principal.caller().as_admin() {
        inspector_org(conn, principal, chat, members)
            .await?
            .and_then(|(_, place)| place)
    } else {
        None
    };
    let native_manage = admin_place.is_some();
    Ok(Standing {
        admin_place,
        admin_access: None,
        admin_only: if native_manage && !manager && site.archived_node.is_none() {
            vec![Permission::ChatManage]
        } else {
            Vec::new()
        },
        participating: in_chat && frame.follows_chat(chat.id),
        reading: Some(reading),
        join: !in_chat
            && matches!(reading, Reading::Public)
            && policy.direct
            && site.archived_node.is_none(),
        sending,
        moderate: (native_manage
            || administration.is_some()
            || manager
            || chat.kind == rows::KIND_GROUP
                && !frame.ways(&site, Permission::ChatManage).is_empty())
            && caller.permits(Permission::ChatManage)
            && site.archived_node.is_none(),
        manage: chat.kind == rows::KIND_GROUP
            && (native_manage || administration.is_some() || manager)
            && site.archived_node.is_none(),
        // Being in a direct or a group chat is what lets one take it away, within the scopes of a token.
        export: in_chat && caller.permits(Permission::ChatExport),
        in_chat,
        names_node: false,
    })
}

/// Whether the other person of a direct chat can no longer be written to: the
/// system suspended them, or the root org did, or they are no member of it.
async fn other_is_gone(
    conn: &mut Conn,
    chat: &ChatRow,
    members: &[MemberRow],
    me: Id,
) -> Result<bool, Error> {
    let others: Vec<Id> = members
        .iter()
        .filter(|row| row.is_in() && row.user_id != me)
        .map(|row| row.user_id)
        .collect();
    let standing = rows::standing(conn, chat.org_id, &others).await?;
    Ok(others.iter().any(|other| {
        !standing
            .iter()
            .any(|each| each.id == *other && each.is_present())
    }))
}

/// The org on which the caller may read this chat without being in it, the
/// highest of them: `directs.view` on the root org, or on an org inside it
/// which everybody in the chat is a member of.
async fn inspector_org(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
    members: &[MemberRow],
) -> Result<Option<(Id, Option<PlaceRef>)>, Error> {
    let present: Vec<Id> = members
        .iter()
        .filter(|row| row.is_in())
        .map(|row| row.user_id)
        .collect();
    let people = rows::standing(conn, chat.org_id, &present).await?;
    let candidates = shared_orgs(chat.org_id, &present, &people);
    let graph = Graph::load(conn, &candidates).await?;
    let sites: Vec<_> = candidates
        .iter()
        .filter_map(|id| graph.site_of(*id))
        .collect();
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        &sites,
        &[Permission::DirectsView],
    )
    .await?;
    let readable: Vec<_> = sites
        .iter()
        .filter(|site| {
            !principal.suspension().applies(&site.orgs)
                && !frame.ways(site, Permission::DirectsView).is_empty()
        })
        .collect();
    Ok(readable
        .iter()
        .copied()
        .find(|site| frame.administers(site))
        .or_else(|| readable.first().copied())
        .map(|site| (site.id, frame.admin_place(site))))
}

/// The orgs which everybody in a chat is a member of, the root org first: all
/// the orgs from the root down to the one that lies deepest in the way of each
/// of them. The root org alone where somebody is no member of the tree (a
/// guest, a person who left) or where nobody is in the chat.
#[must_use]
pub fn shared_orgs(root: Id, present: &[Id], people: &[rows::Standing]) -> Vec<Id> {
    let mut shared: Option<Vec<Id>> = None;
    for user in present {
        let path = people
            .iter()
            .find(|person| person.id == *user)
            .map(|person| person.domain_path.clone())
            .unwrap_or_default();
        if path.is_empty() {
            return vec![root];
        }
        shared = Some(match shared {
            None => path,
            Some(held) => held
                .into_iter()
                .zip(path)
                .take_while(|(left, right)| left == right)
                .map(|(org, _)| org)
                .collect(),
        });
    }
    match shared {
        Some(orgs) if !orgs.is_empty() => orgs,
        _ => vec![root],
    }
}

/// Which of the orgs `orgs` the caller holds `directs.view` on, in the order
/// given. An org the caller is suspended in is not one.
pub async fn inspection_orgs(
    conn: &mut Conn,
    principal: &Principal,
    orgs: &[Id],
) -> Result<Vec<Id>, Error> {
    if orgs.is_empty() {
        return Ok(Vec::new());
    }
    let graph = Graph::load(conn, orgs).await?;
    let sites: Vec<_> = orgs.iter().filter_map(|org| graph.site_of(*org)).collect();
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        &sites,
        &[Permission::DirectsView],
    )
    .await?;
    Ok(sites
        .iter()
        .filter(|site| {
            !principal.suspension().applies(&site.orgs)
                && !frame.ways(site, Permission::DirectsView).is_empty()
        })
        .map(|site| site.id)
        .collect())
}

/// Which of the people `candidates` read the chat now. A node's chat is read
/// by whoever its page lets read it, asked of each person as themself; a
/// direct or a group chat by the people in it who are present: members of the
/// org who are not suspended, and the guests of a group.
pub async fn readers_among(
    conn: &mut Conn,
    chat: &ChatRow,
    members: &[MemberRow],
    candidates: &[Id],
    now: Time,
) -> Result<Vec<Id>, Error> {
    match chat.node_id {
        Some(_) => {
            let policy = ChatPolicy::default();
            let mut readers = Vec::new();
            for candidate in candidates {
                let principal = match Principal::load(
                    conn,
                    Caller::session(*candidate).with_admin_mode(false),
                    now,
                )
                .await
                {
                    Ok(principal) => principal,
                    Err(Error::NoSuchUser(_)) => continue,
                    Err(error) => return Err(error),
                };
                match standing(conn, &principal, chat, members, policy).await {
                    Ok(standing) if standing.reads() => readers.push(*candidate),
                    Ok(_) | Err(Error::Suspended { .. }) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(readers)
        }
        None => {
            let present: Vec<&MemberRow> = members
                .iter()
                .filter(|row| row.active_at(now.get()) && candidates.contains(&row.user_id))
                .collect();
            let ids: Vec<Id> = present.iter().map(|row| row.user_id).collect();
            let people = rows::standing(conn, chat.org_id, &ids).await?;
            Ok(present
                .into_iter()
                .filter(|row| {
                    people.iter().any(|person| {
                        person.id == row.user_id
                            && !person.suspended
                            && !person.suspended_in_org
                            && (person.member || row.guest)
                    })
                })
                .map(|row| row.user_id)
                .collect())
        }
    }
}

/// Whether the caller may bring somebody from outside the root org into a
/// group chat, and if not who may: they hold `access.external` at its home,
/// or are a current manager where `sharing.allow_external` resolves true
/// (`owners_name_outsiders`).
pub async fn may_bring_in_outsider(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
    owners_name_outsiders: bool,
) -> Result<Naming, Error> {
    let graph = Graph::load(conn, &[chat.home()]).await?;
    let home = graph
        .site_of(chat.home())
        .ok_or(Error::NotFound(chat.home()))?;
    let root = ObjectRef {
        kind: home.kind,
        id: home.id,
    };
    let naming = external::may_name(conn, principal, root, Policy::default()).await?;
    if !naming.allowed()
        && owners_name_outsiders
        && rows::is_manager(conn, chat.id, principal.user(), principal.now().get()).await?
    {
        return Ok(Naming {
            through: vec![Through::Level {
                of: Permission::ChatManage,
            }],
            ask: Vec::new(),
            ask_more: 0,
        });
    }
    Ok(naming)
}

/// Public groups reuse the home's ordinary ChatView frame, never membership inferred from a name.
pub(crate) fn public_reader(frame: &Frame<'_>, chat: &ChatRow) -> bool {
    chat.kind == rows::KIND_GROUP
        && chat.visibility == "public"
        && frame.principal().is_in_tree(chat.org_id)
        && frame.graph().site_of(chat.home()).is_some_and(|site| {
            !frame.principal().suspension().applies(&site.orgs)
                && frame.principal().caller().reaches(site.path.as_deref())
                && frame
                    .admin_access_for(&site, Permission::ChatView)
                    .is_none()
                && !frame.ways(&site, Permission::ChatView).is_empty()
        })
}
