//! One chat per node; direct and group chats within a root org, with guests from outside it.
//!
//! [`load`] is where every request about a chat begins: it reads the chat,
//! the people in it and the two settings that decide whether it takes
//! messages, and asks the access engine once what the caller may do in it.
//! A direct or a group chat that the caller may not read does not exist for
//! them. The rest is the making of direct and group chats, a group's name,
//! and who is in it: any member adds another member of the root org, and may
//! leave; the person who made it removes others; somebody from outside the
//! org comes in as a guest only when the caller may bring an outsider in, and
//! sees that chat and nothing else of the tree.

use crate::messages;
use crate::notifications;
use crate::{Acting, Error, Why};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::chats::{CHAT_NAME_MAX_CHARS, ChatSystemEvent};
use orch_authz::chats::{Cannot, ChatPolicy, Standing};
use orch_authz::epoch;
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::chats::{self as rows, After, ChatRow, MemberRow, NewChat, NewMember};
use orch_db::{Conn, sequences};
use orch_realtime_contract::feeds::UserEvent;
use orch_settings::Place;
use orch_types::{Id, Seq, Time};

/// How a chat is read: for looking at, or to be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Nothing is locked.
    Read,
    /// The chat's row is locked until the transaction ends, so that two
    /// changes of one chat are made one after the other.
    Write,
}

/// A chat with the people in it and what the caller may do in it.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The chat.
    pub row: ChatRow,
    /// Every person who ever was in a direct or a group chat; nobody for a
    /// node's.
    pub members: Vec<MemberRow>,
    /// What the caller may do in it.
    pub standing: Standing,
    /// What the settings say about it.
    pub policy: ChatPolicy,
}

impl Loaded {
    /// The chat's identifier.
    #[must_use]
    pub fn id(&self) -> Id {
        self.row.id
    }

    /// The people who are in a direct or a group chat now.
    #[must_use]
    pub fn present(&self) -> Vec<Id> {
        self.members
            .iter()
            .filter(|row| row.is_in())
            .map(|row| row.user_id)
            .collect()
    }

    /// Whether the person is in the chat now.
    #[must_use]
    pub fn has(&self, user: Id) -> bool {
        self.members
            .iter()
            .any(|row| row.user_id == user && row.is_in())
    }

    /// Whether the caller may write in the chat; if not, why.
    pub fn ensure_send(&self) -> Result<(), Error> {
        match self.standing.sending {
            Ok(()) => Ok(()),
            Err(Cannot::Archived { node }) => Err(Error::Archived { node }),
            Err(Cannot::Disabled) => Err(Error::NotAllowed(Why::Disabled)),
            Err(Cannot::DirectOff) => Err(Error::NotAllowed(Why::DirectOff)),
            Err(Cannot::Closed) => Err(Error::NotAllowed(Why::Closed)),
            Err(Cannot::NoPermission) => Err(Error::Lacks {
                // The node of a node's chat; the root org for the chats among its people.
                node: self.row.node_id.unwrap_or(self.row.org_id),
                permission: Permission::ChatInteract,
            }),
            Err(Cannot::Inspecting) => Err(Error::Inspecting),
        }
    }

    /// Locks the chat's row and reads it again, as a change needs it. A
    /// writer that also needs the thread's sequence takes that first.
    pub async fn lock(&mut self, conn: &mut Conn) -> Result<(), Error> {
        self.row = rows::lock(conn, self.row.id)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(())
    }

    /// The people in the chat who are guests, who are not members of the org.
    #[must_use]
    pub fn guests(&self) -> Vec<Id> {
        self.members
            .iter()
            .filter(|row| row.is_in() && row.guest)
            .map(|row| row.user_id)
            .collect()
    }
}

/// What the settings say about a chat: whether the chats of its node take
/// messages (`chat.enabled`), and whether direct and group chats do in its
/// root org (`chat.direct` resolves to `members`).
pub async fn policy_of(conn: &mut Conn, chat: &ChatRow) -> Result<ChatPolicy, Error> {
    match chat.node_id {
        Some(node) => Ok(ChatPolicy {
            enabled: orch_settings::get::<bool>(conn, Place::Node(node), "chat.enabled")
                .await?
                .unwrap_or(true),
            direct: true,
        }),
        None => Ok(ChatPolicy {
            enabled: true,
            direct: direct_allowed(conn, chat.org_id).await?,
        }),
    }
}

/// Whether people of the root org may start direct and group chats and write
/// in them (`chat.direct` resolves to `members` there).
pub async fn direct_allowed(conn: &mut Conn, root: Id) -> Result<bool, Error> {
    Ok(
        orch_settings::get::<String>(conn, Place::Node(root), "chat.direct")
            .await?
            .is_none_or(|word| word == "members"),
    )
}

/// The chat `id` as the caller stands towards it. A direct or a group chat
/// that they may not read is [`Error::NotFound`]; the chat of a node that they
/// may not read lacks `chat.view` of the node.
pub async fn load(conn: &mut Conn, act: &Acting<'_>, id: Id, mode: Mode) -> Result<Loaded, Error> {
    let found = match mode {
        Mode::Read => rows::chat(conn, id).await?,
        Mode::Write => rows::lock(conn, id).await?,
    };
    let row = found.ok_or(Error::NotFound)?;
    let members = if row.is_people_chat() {
        rows::effective_members(&rows::members(conn, id).await?, act.now)
    } else {
        Vec::new()
    };
    let policy = policy_of(conn, &row).await?;
    let standing = orch_authz::chats::standing(conn, act.principal, &row, &members, policy).await?;
    if !standing.reads() {
        return Err(match row.node_id {
            Some(node) => Error::Lacks {
                node,
                permission: Permission::ChatView,
            },
            None => Error::NotFound,
        });
    }
    Ok(Loaded {
        row,
        members,
        standing,
        policy,
    })
}

/// The people of the chats `chats`, every row of each, by chat. A node's chat
/// has none.
pub async fn members_of(
    conn: &mut Conn,
    chats: &[Id],
    at: Time,
) -> Result<std::collections::HashMap<Id, Vec<MemberRow>>, Error> {
    let mut by_chat: std::collections::HashMap<Id, Vec<MemberRow>> =
        chats.iter().map(|chat| (*chat, Vec::new())).collect();
    for row in rows::effective_members(&rows::members_in(conn, chats).await?, at.get()) {
        by_chat.entry(row.chat_id).or_default().push(row);
    }
    Ok(by_chat)
}

// ------------------------------------------------------------------ making chats

/// A direct or a group chat that was opened.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The chat.
    pub chat: ChatRow,
    /// Whether it was made now; a direct chat that existed is not.
    pub created: bool,
}

fn name_of(name: Option<&str>) -> Result<Option<String>, Error> {
    let Some(name) = name else { return Ok(None) };
    let name = name.trim();
    let wanted = usize::try_from(CHAT_NAME_MAX_CHARS).unwrap_or(usize::MAX);
    if name.is_empty() || name.chars().count() > wanted {
        return Err(Error::invalid(
            "name",
            format!("is a name of 1 to {CHAT_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(Some(name.to_owned()))
}

/// The key of the direct chat of two people: the smaller identifier, a colon,
/// the larger.
#[must_use]
pub fn direct_key(one: Id, other: Id) -> String {
    format!("{}:{}", one.min(other), one.max(other))
}

/// Tells the people of a chat, on all their screens, that it has a new message
/// or came to be.
pub(crate) async fn tell_members(
    conn: &mut Conn,
    chat: Id,
    seq: Seq,
    at: DateTime<Utc>,
    members: &[Id],
) -> Result<(), Error> {
    notifications::tell_users(
        conn,
        members,
        &UserEvent::Chat {
            chat_id: chat,
            seq,
            at: Time::new(at),
        },
    )
    .await
}

async fn audit_chat(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: &ChatRow,
    action: AuditAction,
    summary: &str,
    details: serde_json::Value,
) -> Result<(), Error> {
    let mut entry = Entry::new(
        Actor::User(act.user()),
        action.as_str(),
        "chat",
        Some(chat.id),
        summary,
    );
    entry.org_id = Some(chat.org_id);
    entry.details = details;
    audit::append(conn, &entry, act.origin, act.now).await?;
    Ok(())
}

/// Opens a chat of the caller with other people in a root org. With one other
/// person and no name it is the direct chat with them, which is the one they
/// have if they have one; otherwise it is a new group, named or named after
/// its members.
///
/// The caller and everybody named are members of the org, none is suspended,
/// and `chat.direct` allows it there; a group has at most
/// `chat.group_max_members` people, the caller among them.
pub async fn open(
    conn: &mut Conn,
    act: &Acting<'_>,
    root: Id,
    users: &[Id],
    name: Option<&str>,
) -> Result<Opened, Error> {
    if !rows::is_root_org(conn, root).await? {
        return Err(Error::NotFound);
    }
    let me = act.user();
    if let Some(closing) = act.principal.suspension().closing(&[root])
        && act.principal.is_in_tree(root)
    {
        return Err(Error::Suspended { org: closing.org() });
    }
    if !act.principal.is_in_tree(root) {
        return Err(Error::NotFound);
    }
    let graph = orch_authz::engine::Graph::load(conn, &[root]).await?;
    if let Some(node) = graph.site_of(root).and_then(|site| site.archived_node) {
        return Err(Error::Archived { node });
    }
    if !direct_allowed(conn, root).await? {
        return Err(Error::NotAllowed(Why::DirectOff));
    }
    let name = name_of(name)?;
    let mut others: Vec<Id> = Vec::new();
    for user in users {
        if *user != me && !others.contains(user) {
            others.push(*user);
        }
    }
    if others.is_empty() {
        return Err(Error::invalid("user_ids", "name at least one other person"));
    }
    let people = rows::standing(conn, root, &others).await?;
    for (at, user) in others.iter().enumerate() {
        let Some(person) = people.iter().find(|person| person.id == *user) else {
            return Err(Error::invalid(
                format!("user_ids[{at}]"),
                "is no person here",
            ));
        };
        if person.kind != "human" || !person.member {
            return Err(Error::NotAllowed(Why::NotMember));
        }
        if !person.is_present() {
            return Err(Error::NotAllowed(Why::Closed));
        }
    }
    let direct = others.len() == 1 && name.is_none();
    if direct {
        open_direct(conn, act, root, others[0]).await
    } else {
        open_group(conn, act, root, &others, name.as_deref()).await
    }
}

async fn open_direct(
    conn: &mut Conn,
    act: &Acting<'_>,
    root: Id,
    other: Id,
) -> Result<Opened, Error> {
    let me = act.user();
    let key = direct_key(me, other);
    let made = rows::insert_chat(
        conn,
        &NewChat {
            id: orch_types::new_id(),
            kind: rows::KIND_DIRECT,
            org_id: root,
            direct_key: Some(&key),
            name: None,
            created_by: me,
            at: act.now,
        },
    )
    .await?;
    let Some(chat) = made else {
        let chat = rows::direct_chat(conn, root, &key)
            .await?
            .ok_or_else(|| Error::Internal("a direct chat is neither there nor made".to_owned()))?;
        // Opening it again puts it back on the list of whoever took it off.
        if rows::state(conn, me, chat.id)
            .await?
            .is_some_and(|state| state.hidden)
        {
            rows::set_hidden(conn, me, chat.id, false, act.now).await?;
        }
        return Ok(Opened {
            chat,
            created: false,
        });
    };
    let seq = take(conn, chat.id).await?;
    for user in [me, other] {
        rows::add_member(
            conn,
            &NewMember {
                expires_at: None,
                chat_id: chat.id,
                user_id: user,
                added_by: Some(me),
                guest: false,
                seq,
                at: act.now,
            },
        )
        .await?;
    }
    rows::stamp_state(conn, chat.id, seq).await?;
    rows::notify(conn, chat.id, seq).await?;
    tell_members(conn, chat.id, seq, act.now, &[me, other]).await?;
    Ok(Opened {
        chat: rows::chat(conn, chat.id).await?.unwrap_or(chat),
        created: true,
    })
}

async fn open_group(
    conn: &mut Conn,
    act: &Acting<'_>,
    root: Id,
    others: &[Id],
    name: Option<&str>,
) -> Result<Opened, Error> {
    let me = act.user();
    let most = group_max(conn).await?;
    if i64::try_from(others.len() + 1).unwrap_or(i64::MAX) > most {
        return Err(Error::NotAllowed(Why::Full));
    }
    let chat = rows::insert_chat(
        conn,
        &NewChat {
            id: orch_types::new_id(),
            kind: rows::KIND_GROUP,
            org_id: root,
            direct_key: None,
            name,
            created_by: me,
            at: act.now,
        },
    )
    .await?
    .ok_or_else(|| Error::Internal("a group chat was not made".to_owned()))?;
    let page = orch_db::queries::access::PageKey::new("chat", chat.id, "live");
    orch_db::queries::access::insert_entry(
        conn,
        &orch_db::queries::access::NewEntry {
            id: orch_types::new_id(),
            page: &page,
            profile: "any",
            permission: "access.own",
            holder_kind: "user",
            holder_id: Some(me),
            holder_role: None,
            options: &serde_json::json!({}),
            expires_at: None,
            note: "",
            source: "creation",
            created_by: Some(me),
            at: act.now,
        },
    )
    .await?;
    let seq = take(conn, chat.id).await?;
    let mut everyone = vec![me];
    everyone.extend_from_slice(others);
    for user in &everyone {
        rows::add_member(
            conn,
            &NewMember {
                expires_at: None,
                chat_id: chat.id,
                user_id: *user,
                added_by: Some(me),
                guest: false,
                seq,
                at: act.now,
            },
        )
        .await?;
    }
    rows::stamp_state(conn, chat.id, seq).await?;
    let line = messages::system(
        conn,
        &chat,
        seq,
        &messages::Line {
            event: ChatSystemEvent::Created,
            by: Some(me),
            user: None,
            name,
        },
        act.now,
    )
    .await?;
    audit_chat(
        conn,
        act,
        &chat,
        orch_db::audit::action!(AuditAction::ChatCreated),
        "started a group chat",
        serde_json::json!({ "people": everyone, "name": name }),
    )
    .await?;
    rows::notify(conn, chat.id, line.seq_created).await?;
    tell_members(conn, chat.id, seq, act.now, &everyone).await?;
    Ok(Opened {
        chat: rows::chat(conn, chat.id).await?.unwrap_or(chat),
        created: true,
    })
}

/// The next number of a chat's sequence.
pub(crate) async fn take(conn: &mut Conn, chat: Id) -> Result<Seq, Error> {
    rows::next_seq(conn, chat).await?.ok_or(Error::NotFound)
}

/// The most people one group has: `chat.group_max_members`, a setting of the
/// system.
pub async fn group_max(conn: &mut Conn) -> Result<i64, Error> {
    Ok(
        orch_settings::get::<i64>(conn, Place::System, "chat.group_max_members")
            .await?
            .unwrap_or(DEFAULT_GROUP_MAX),
    )
}

/// How many people a group holds where the system says nothing: what the
/// setting ships as.
const DEFAULT_GROUP_MAX: i64 = 10;

// ------------------------------------------------------------------ a group's name and people

fn ensure_group(loaded: &Loaded) -> Result<(), Error> {
    if loaded.row.kind == rows::KIND_GROUP {
        Ok(())
    } else {
        Err(Error::invalid(
            "",
            "only a group chat has a name and people to add or take out",
        ))
    }
}

/// Only somebody who is in the group may change it, and a guest may not add
/// anybody.
fn ensure_in(loaded: &Loaded, act: &Acting<'_>) -> Result<(), Error> {
    if loaded.has(act.user()) || loaded.standing.moderate {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

/// Gives a group a name, or names it after its members again.
pub async fn rename(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    name: Option<&str>,
) -> Result<ChatRow, Error> {
    ensure_group(loaded)?;
    ensure_in(loaded, act)?;
    loaded.ensure_send()?;
    let name = name_of(name)?;
    loaded.lock(conn).await?;
    if loaded.row.name == name {
        return Ok(loaded.row.clone());
    }
    let seq = take(conn, loaded.id()).await?;
    rows::rename(conn, loaded.id(), name.as_deref()).await?;
    rows::stamp_state(conn, loaded.id(), seq).await?;
    let line = messages::system(
        conn,
        &loaded.row,
        seq,
        &messages::Line {
            event: ChatSystemEvent::Renamed,
            by: Some(act.user()),
            user: None,
            name: name.as_deref(),
        },
        act.now,
    )
    .await?;
    audit_chat(
        conn,
        act,
        &loaded.row,
        orch_db::audit::action!(AuditAction::ChatRenamed),
        "renamed a group chat",
        serde_json::json!({ "name": name }),
    )
    .await?;
    rows::notify(conn, loaded.id(), line.seq_created).await?;
    let row = rows::chat(conn, loaded.id())
        .await?
        .ok_or(Error::NotFound)?;
    loaded.row = row.clone();
    Ok(row)
}

/// Puts a person in a group. A member of the root org is let in by any member
/// of the group who is one too; somebody from outside it comes in as a guest
/// only when the caller may bring an outsider in
/// ([`orch_authz::chats::may_bring_in_outsider`], with `owners_name_outsiders`
/// the policy `sharing.allow_external` there).
pub async fn add_member(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    user: Id,
    owners_name_outsiders: bool,
) -> Result<(), Error> {
    add_member_until(conn, act, loaded, user, owners_name_outsiders, None).await
}

/// Checks the ordinary add-member rule without changing membership. Returns whether the newcomer is a guest.
pub async fn can_add_member(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    user: Id,
    owners_name_outsiders: bool,
) -> Result<bool, Error> {
    ensure_group(loaded)?;
    ensure_in(loaded, act)?;
    if matches!(
        loaded.standing.reading,
        Some(orch_authz::chats::Reading::Guest)
    ) {
        return Err(Error::NotFound);
    }
    loaded.ensure_send()?;
    let people = rows::standing(conn, loaded.row.org_id, &[user]).await?;
    let person = people
        .first()
        .ok_or_else(|| Error::invalid("user_id", "is no person here"))?;
    if person.kind != "human" || person.suspended {
        return Err(Error::NotAllowed(Why::NotMember));
    }
    if person.member && person.suspended_in_org {
        return Err(Error::NotAllowed(Why::Closed));
    }
    let guest = !person.member;
    if guest {
        let naming = orch_authz::chats::may_bring_in_outsider(
            conn,
            act.principal,
            &loaded.row,
            owners_name_outsiders,
        )
        .await?;
        if !naming.allowed() {
            return Err(Error::Outsider {
                ask: naming.ask,
                ask_more: naming.ask_more,
            });
        }
    }
    Ok(guest)
}

/// Adds the same ordinary membership with an optional deadline. Existing live membership is never shortened.
pub async fn add_member_until(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    user: Id,
    owners_name_outsiders: bool,
    expires_at: Option<DateTime<Utc>>,
) -> Result<(), Error> {
    if expires_at.is_some_and(|end| end <= act.now) {
        return Err(Error::invalid("expires_at", "must be in the future"));
    }
    let guest = can_add_member(conn, act, loaded, user, owners_name_outsiders).await?;
    loaded.lock(conn).await?;
    let mut members = rows::effective_members(&rows::members(conn, loaded.id()).await?, act.now);
    if members.iter().any(|row| row.user_id == user && row.is_in()) {
        loaded.members = members;
        return Ok(());
    }
    let present = members.iter().filter(|row| row.is_in()).count();
    if i64::try_from(present).unwrap_or(i64::MAX) >= group_max(conn).await? {
        return Err(Error::NotAllowed(Why::Full));
    }
    let seq = take(conn, loaded.id()).await?;
    rows::add_member(
        conn,
        &NewMember {
            expires_at,
            chat_id: loaded.id(),
            user_id: user,
            added_by: Some(act.user()),
            guest,
            seq,
            at: act.now,
        },
    )
    .await?;
    rows::stamp_state(conn, loaded.id(), seq).await?;
    messages::system(
        conn,
        &loaded.row,
        seq,
        &messages::Line {
            event: ChatSystemEvent::MemberAdded,
            by: Some(act.user()),
            user: Some(user),
            name: None,
        },
        act.now,
    )
    .await?;
    audit_chat(
        conn,
        act,
        &loaded.row,
        orch_db::audit::action!(AuditAction::ChatMemberAdded),
        "added somebody to a group chat",
        serde_json::json!({ "user": user, "guest": guest, "expires_at": expires_at }),
    )
    .await?;
    changed_people(conn, loaded.id(), seq, act.now, &[user]).await?;
    members = rows::effective_members(&rows::members(conn, loaded.id()).await?, act.now);
    loaded.members = members;
    Ok(())
}

/// Takes a person out of a group: the person themself, to leave, or the one
/// who made the group.
pub async fn remove_member(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    user: Id,
) -> Result<(), Error> {
    ensure_group(loaded)?;
    ensure_in(loaded, act)?;
    let leaving = user == act.user();
    if !leaving && loaded.row.created_by != Some(act.user()) && !loaded.standing.moderate {
        return Err(Error::NotTheAuthor);
    }
    if let Err(archived @ Error::Archived { .. }) = loaded.ensure_send() {
        return Err(archived);
    }
    loaded.lock(conn).await?;
    let seq = take(conn, loaded.id()).await?;
    if !rows::remove_member(conn, loaded.id(), user, seq, act.now).await? {
        return Err(Error::NotFound);
    }
    rows::stamp_state(conn, loaded.id(), seq).await?;
    messages::system(
        conn,
        &loaded.row,
        seq,
        &messages::Line {
            event: if leaving {
                ChatSystemEvent::MemberLeft
            } else {
                ChatSystemEvent::MemberRemoved
            },
            by: Some(act.user()),
            user: (!leaving).then_some(user),
            name: None,
        },
        act.now,
    )
    .await?;
    audit_chat(
        conn,
        act,
        &loaded.row,
        orch_db::audit::action!(AuditAction::ChatMemberRemoved),
        if leaving {
            "left a group chat"
        } else {
            "took somebody out of a group chat"
        },
        serde_json::json!({ "user": user }),
    )
    .await?;
    changed_people(conn, loaded.id(), seq, act.now, &[user]).await?;
    loaded.members = rows::members(conn, loaded.id()).await?;
    Ok(())
}

/// What follows from a change of who is in a chat: the authorization epoch
/// moves, so that whoever follows the chat is looked at again, and the people
/// it concerns are told on their screens.
async fn changed_people(
    conn: &mut Conn,
    chat: Id,
    seq: Seq,
    at: DateTime<Utc>,
    concerned: &[Id],
) -> Result<(), Error> {
    let epoch = epoch::bump(conn).await?;
    sequences::notify_epoch(conn, epoch).await?;
    rows::notify(conn, chat, seq).await?;
    let present: Vec<Id> = rows::members(conn, chat)
        .await?
        .into_iter()
        .filter(MemberRow::is_in)
        .map(|row| row.user_id)
        .chain(concerned.iter().copied())
        .collect();
    tell_members(conn, chat, seq, at, &present).await
}

// ------------------------------------------------------------------ lists

/// Which chats of a root org a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The caller's direct and group chats that are not off their list.
    Mine,
    /// The direct and group chats the caller is not in and may read through
    /// `directs.view`.
    Others {
        /// The org on which the caller holds it; the highest of the tree on
        /// which they hold it when none is given.
        of: Option<Id>,
    },
    /// The chats of the tree's nodes that the caller can read and that have
    /// messages.
    Nodes,
}

/// A page of a list of chats.
#[derive(Debug, Clone)]
pub struct Listed {
    /// The chats, the most recently active first.
    pub chats: Vec<ChatRow>,
    /// The chat after which the next page begins; none at the end.
    pub next: Option<After>,
}

/// How many chats a list looks at in one step when it has to filter them by
/// what the caller may read: enough that a page of mostly readable chats is
/// one step.
const STEP: i64 = 100;

/// The chats of the root org `root` that the caller may list, `limit` of them
/// from `after`.
pub async fn list(
    conn: &mut Conn,
    act: &Acting<'_>,
    root: Id,
    scope: Scope,
    after: Option<After>,
    limit: i64,
) -> Result<Listed, Error> {
    if !rows::is_root_org(conn, root).await? {
        return Err(Error::NotFound);
    }
    let me = act.user();
    let in_tree = act.principal.is_in_tree(root);
    if let Some(closing) = act.principal.suspension().closing(&[root])
        && in_tree
    {
        return Err(Error::Suspended { org: closing.org() });
    }
    let mut chats = match scope {
        Scope::Mine => rows::mine(conn, me, root, !in_tree, after, limit + 1).await?,
        Scope::Others { of } => {
            let tree = rows::orgs_of_tree(conn, root).await?;
            let held = orch_authz::chats::inspection_orgs(conn, act.principal, &tree).await?;
            let orgs = match of {
                Some(of) if !tree.contains(&of) => return Err(Error::NotFound),
                Some(of) if !held.contains(&of) => {
                    return Err(Error::Lacks {
                        node: of,
                        permission: Permission::DirectsView,
                    });
                }
                Some(of) => vec![of],
                None => held,
            };
            rows::others_among(conn, me, root, &orgs, after, limit + 1).await?
        }
        Scope::Nodes => {
            return list_nodes(conn, act, root, after, limit).await;
        }
    };
    let more = i64::try_from(chats.len()).unwrap_or(i64::MAX) > limit;
    chats.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next = more.then(|| chats.last()).flatten().map(|last| After {
        at: last.active_at(),
        id: last.id,
    });
    Ok(Listed { chats, next })
}

/// The same list, focused by its ordinary Frame facts before the page fills.
/// Muting a chat does not affect whether it may be read.
pub async fn list_participating(
    conn: &mut Conn,
    act: &Acting<'_>,
    root: Id,
    scope: Scope,
    after: Option<After>,
    limit: i64,
) -> Result<Listed, Error> {
    let wanted = usize::try_from(limit).unwrap_or(0);
    let mut kept = Vec::new();
    let mut cursor = after;
    loop {
        let page = list(conn, act, root, scope, cursor, STEP).await?;
        let access = orch_authz::chats::listed_access(conn, act.principal, &page.chats).await?;
        kept.extend(page.chats.into_iter().filter(|chat| {
            access
                .get(&chat.id)
                .is_some_and(|access| access.participating)
        }));
        if kept.len() > wanted || page.next.is_none() {
            break;
        }
        cursor = page.next;
    }
    let more = kept.len() > wanted;
    kept.truncate(wanted);
    let next = if more {
        kept.last().map(|last| After {
            at: last.active_at(),
            id: last.id,
        })
    } else {
        None
    };
    Ok(Listed { chats: kept, next })
}

async fn list_nodes(
    conn: &mut Conn,
    act: &Acting<'_>,
    root: Id,
    after: Option<After>,
    limit: i64,
) -> Result<Listed, Error> {
    use orch_access_contract::permissions::PermissionName;
    use orch_authz::{Act, Policy, nodes};
    let wanted = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut kept: Vec<ChatRow> = Vec::new();
    let mut cursor = after;
    loop {
        let batch = rows::node_chats_with_messages(conn, root, cursor, STEP).await?;
        let exhausted = i64::try_from(batch.len()).is_ok_and(|count| count < STEP);
        let Some(last) = batch.last() else { break };
        cursor = last.last_message_at.map(|at| After { at, id: last.id });
        let ids: Vec<Id> = batch.iter().filter_map(|chat| chat.node_id).collect();
        let readable: Vec<Id> = nodes::holding(
            conn,
            act.principal,
            &ids,
            PermissionName::Exact(Permission::ChatView),
            Act::Use,
            Policy::default(),
        )
        .await?
        .into_iter()
        .map(|listed| listed.id)
        .collect();
        kept.extend(
            batch
                .into_iter()
                .filter(|chat| chat.node_id.is_some_and(|node| readable.contains(&node))),
        );
        if kept.len() > wanted || exhausted {
            break;
        }
    }
    let more = kept.len() > wanted;
    kept.truncate(wanted);
    let next = more.then(|| kept.last()).flatten().map(|last| After {
        at: last.active_at(),
        id: last.id,
    });
    Ok(Listed { chats: kept, next })
}

/// Records overdue memberships as removals. Access checks already enforce the deadline before this sweep runs.
pub async fn expire_memberships(
    conn: &mut Conn,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<(), Error> {
    let due = rows::expired_group_memberships(conn, now, limit).await?;
    for (id, user) in due {
        let Some(chat) = rows::lock(conn, id).await? else {
            continue;
        };
        if !rows::members(conn, id).await?.iter().any(|m| {
            m.user_id == user && m.left_at.is_none() && m.expires_at.is_some_and(|end| end <= now)
        }) {
            continue;
        }
        let seq = take(conn, id).await?;
        if !rows::remove_member(conn, id, user, seq, now).await? {
            continue;
        }
        rows::stamp_state(conn, id, seq).await?;
        messages::system(
            conn,
            &chat,
            seq,
            &messages::Line {
                event: ChatSystemEvent::MemberExpired,
                by: None,
                user: Some(user),
                name: None,
            },
            now,
        )
        .await?;
        let mut entry = Entry::new(
            Actor::System,
            orch_db::audit::action!(AuditAction::ChatMemberRemoved).as_str(),
            "chat",
            Some(id),
            "a timed group membership ended",
        );
        entry.org_id = Some(chat.org_id);
        entry.details = serde_json::json!({"user":user,"reason":"expired"});
        audit::append(conn, &entry, &orch_db::audit::Origin::default(), now).await?;
        changed_people(conn, id, seq, now, &[user]).await?;
    }
    Ok(())
}
