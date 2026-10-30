//! Chats, messages, states, attachments, inspection and forwarding.
//!
//! The rules of chats are `orch-chat`'s and who may do what in one is the access
//! engine's; the handlers here read a request, begin the transaction that a
//! change and everything it causes are made in, ask [`chats::load`] once what
//! the caller may do in the chat, and turn the answer into the types of the
//! API and the refusal into the problem a client acts on. A direct or a group
//! chat that the caller may not read does not exist for them.
//!
//! The bytes of images and files are in `files`: a file stored to be named by
//! an `attachment` part, and the images and files of a chat's messages given
//! back to whoever reads the chat. A forward to an agent's thread is an input
//! of the forwarder's, taken by the thread engine in the transaction of the
//! forward.

mod agents;
mod asking;
mod files;
mod mentions;
mod readers;

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx, Registry};
use crate::routes::nodes::view;
use crate::routes::resources::sealing;
use crate::sign_in::origin;
use chrono::{DateTime, Utc};
use http::StatusCode;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::MissingAccess;
use orch_api_contract::chats::{
    Chat, ChatDraft, ChatDraftDelete, ChatDraftGet, ChatDraftSet, ChatDraftSetBody, ChatGet,
    ChatListScope, ChatMemberAdd, ChatMemberRemove, ChatMessage, ChatMessageCreate,
    ChatMessageCreateBody, ChatMessageCreateReply, ChatMessageDelete, ChatMessageList,
    ChatMessageListQuery, ChatMessageListReply, ChatMessagePinDelete, ChatMessagePinSet,
    ChatMessageReactionAdd, ChatMessageReactionRemove, ChatMessageUpdate, ChatMessageUpdateBody,
    ChatOpening, ChatOpeningGet, ChatRead, ChatReadBody, ChatState, ChatStateSet, ChatStateSetBody,
    ChatSync, ChatSyncGet, ChatSyncGetQuery, ChatUpdate, ChatUpdateBody, ForwardChatMessage,
    ForwardCreate, ForwardCreateBody, ForwardCreateReply, ForwardDestination, ForwardSource,
    NodeChatOpening, NodeChatOpeningGet, NodeChatOpeningGetQuery, NodeChatScope, OrgChatCreate,
    OrgChatCreateBody, OrgChatList, OrgChatListQuery, OrgChatsRead,
};
use orch_api_contract::chats::{
    ChatPart, FORWARD_MAX_DESTINATIONS, FORWARD_MAX_PARTS, ForwardThreadInput,
};
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::nodes::{Node, NodeKind, NodeRef, TreeChat};
use orch_api_contract::personal::MeChatStateList;
use orch_api_contract::routes::Endpoint;
use orch_api_contract::threads::{
    AgentName, ContentPart, Forwarded, ForwardedAuthor, ForwardedFrom,
};
use orch_authz::nodes;
use orch_authz::{Act, Policy, Principal};
use orch_chat::chats::{self, Loaded, Mode, Scope};
use orch_chat::forward::{self, Delivery, Destination};
use orch_chat::messages::{self, Post, Window};
use orch_chat::views::{self, Lookups, Seen};
use orch_chat::{Acting, Error as ChatError, following, inspection, redaction};
use orch_db::queries::chats::{self as rows, After, ChatRow, MessageRow};
use orch_db::queries::hierarchy;
use orch_db::secrets::MasterKey;
use orch_db::{Conn, DbError, Tx};
use orch_engine::written;
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde_json::json;
use std::collections::HashMap;

mod export;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    mentions::mount(registry);
    agents::mount(registry);
    export::mount(registry);
    registry
        .on(OrgChatList, org_chat_list)
        .on(OrgChatCreate, org_chat_create)
        .on(ChatGet, chat_get)
        .on(ChatUpdate, chat_update)
        .on(ChatMemberAdd, member_add)
        .on(orch_api_contract::chats::ChatJoinRequest, join_request)
        .on(ChatMemberRemove, member_remove)
        .on(ChatSyncGet, sync_get)
        .on(ChatOpeningGet, opening_get)
        .on(NodeChatOpeningGet, node_opening_get)
        .on(ChatMessageList, message_list)
        .on(ChatMessageCreate, message_create)
        .on(ChatMessageUpdate, message_update)
        .on(ChatMessageDelete, message_delete)
        .on(ChatMessageReactionAdd, reaction_add)
        .on(ChatMessageReactionRemove, reaction_remove)
        .on(ChatMessagePinSet, pin_set)
        .on(ChatMessagePinDelete, pin_delete)
        .on(ChatRead, chat_read)
        .on(OrgChatsRead, org_chats_read)
        .on(ChatStateSet, state_set)
        .on(ChatDraftGet, draft_get)
        .on(ChatDraftSet, draft_set)
        .on(ChatDraftDelete, draft_delete)
        .on(ForwardCreate, forward_create)
        .on(MeChatStateList, me_chat_states);
    files::mount(registry);
}

// ------------------------------------------------------------------ what every handler needs

/// The key that opens secrets, for masking text; asked only where a node has
/// secrets.
fn keys() -> Result<&'static MasterKey, String> {
    sealing::master_key().map_err(|error| error.problem.detail)
}

/// A refusal of something only the author does, or that nobody may do.
fn denied_for(chat: Option<Id>, required: &str, detail: &str) -> ApiError {
    ApiError::new(ErrorCode::AuthzDenied, detail)
        .with("required", required)
        .with("object", serde_json::json!({ "kind": "chat", "id": chat }))
        .with("requestable", false)
        .with("override", false)
        .with("missing", Vec::<MissingAccess>::new())
}

/// What went wrong in a chat, as the problem a client acts on.
pub async fn failed(
    conn: &mut Conn,
    principal: &Principal,
    chat: Option<Id>,
    error: ChatError,
) -> ApiError {
    match error {
        ChatError::Db(error) => error.into(),
        ChatError::Authz(error) => ApiError::internal(error),
        ChatError::Settings(error) => ApiError::internal(error),
        ChatError::NotFound => ApiError::not_found(),
        ChatError::MentionRestricted => ApiError::new(
            ErrorCode::AuthzDenied,
            "this chat reserves role and group mentions to its current managers",
        )
        .with("policy", "chat.group_mentions"),
        ChatError::StaleRead { revision } => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the chat's read position was changed on another device",
        )
        .with("read_revision", revision),
        ChatError::StaleMessage { revision } => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the message changed while this action was waiting",
        )
        .with("seq_updated", revision),
        ChatError::Suspended { org } => {
            refusals::problem(conn, orch_authz::Error::Suspended { org }).await
        }
        ChatError::Archived { node } => {
            refusals::problem(conn, orch_authz::Error::Archived { node }).await
        }
        ChatError::Lacks { node, permission } => {
            match authorization::require(
                conn,
                principal,
                node,
                authorization::exact(permission),
                Act::Use,
                Policy::default(),
            )
            .await
            {
                Err(error) => error,
                Ok(()) => denied_for(
                    chat,
                    permission.as_str(),
                    &format!("{} is required here", permission.as_str()),
                ),
            }
        }
        ChatError::NotAllowed(why) => ApiError::new(
            ErrorCode::ChatNotAllowed,
            format!("this chat does not allow it ({})", why.word()),
        )
        .with("reason", why.word()),
        ChatError::Inspecting => denied_for(
            chat,
            Permission::ChatInteract.as_str(),
            "an admin who reads a chat they are not in only reads it",
        ),
        ChatError::Outsider { ask, ask_more } => {
            refusals::external_forbidden(conn, &ask, ask_more).await
        }
        ChatError::Invalid { path, message } => ApiError::invalid(&path, &message),
        ChatError::NotTheAuthor => denied_for(
            chat,
            Permission::ChatInteract.as_str(),
            "only the author of a message does that",
        ),
        ChatError::Unmasked(cause) | ChatError::Internal(cause) => ApiError::internal(cause),
        ChatError::Unsupported(what) => ApiError::new(ErrorCode::CapabilityUnsupported, what)
            .with("capability", "forward.thread"),
        ChatError::Destination { to, error } => Box::pin(failed(conn, principal, chat, *error))
            .await
            .with("object", to),
    }
}

/// What a handler works with: the connection or transaction, who acts and when.
struct Work {
    principal: Principal,
    origin: orch_db::audit::Origin,
    now: DateTime<Utc>,
}

impl Work {
    async fn begin(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Self> {
        let principal = authorization::principal(ctx, conn).await?;
        let now = principal.now().get();
        Ok(Self {
            principal,
            origin: origin(ctx),
            now,
        })
    }

    fn acting(&self) -> Acting<'_> {
        Acting {
            principal: &self.principal,
            now: self.now,
            origin: &self.origin,
        }
    }

    async fn refuse<T>(
        &self,
        conn: &mut Conn,
        chat: Option<Id>,
        result: Result<T, ChatError>,
    ) -> ApiResult<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => Err(failed(conn, &self.principal, chat, error).await),
        }
    }
}

async fn transaction(ctx: &Ctx) -> ApiResult<Tx> {
    if crate::middleware::transaction_guard::changes(ctx) {
        let (tx, _) = crate::middleware::transaction_guard::begin(ctx).await?;
        Ok(tx)
    } else {
        Ok(ctx.app.db.begin().await?)
    }
}

/// The people and nodes that views name, as the caller may know them.
async fn lookups(
    conn: &mut Conn,
    principal: &Principal,
    people: &[Id],
    nodes: &[Id],
    groups: &[Id],
) -> ApiResult<Lookups> {
    Ok(Lookups {
        people: refusals::user_refs(conn, people).await?,
        nodes: view::refs(conn, principal, nodes, Policy::default()).await?,
        groups: mentions::qualified_groups(conn, principal, groups).await?,
    })
}

/// The current title of a people's chat, called only after its reader has been authorized.
pub(crate) async fn people_label(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChatRow,
    members: &[orch_db::queries::chats::MemberRow],
) -> ApiResult<String> {
    if let Some(name) = &row.name {
        return Ok(name.clone());
    }
    let ids: Vec<_> = members
        .iter()
        .filter(|member| {
            member.active_at(principal.now().get()) && member.user_id != principal.user()
        })
        .map(|member| member.user_id)
        .collect();
    let people = refusals::user_refs(conn, &ids).await?;
    let name = ids
        .iter()
        .filter_map(|person| people.get(person).map(|person| person.display_name.clone()))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(if name.is_empty() {
        "Chat".to_owned()
    } else {
        name
    })
}

/// A chat as the caller sees it, with their own state in it.
pub async fn chat_view(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChatRow,
    members: &[orch_db::queries::chats::MemberRow],
    standing: &orch_authz::chats::Standing,
) -> ApiResult<Chat> {
    let mut chat = chat_view_without_preview(conn, principal, row, members, standing).await?;
    chat.agents = Some(agents::roster(conn, principal, row).await?);
    if row.node_id.is_none() {
        chat.inspection_holders = Some(readers::inspection(conn, principal, row).await?);
    }
    add_previews(conn, principal, std::slice::from_mut(&mut chat)).await?;
    Ok(chat)
}

async fn chat_view_without_preview(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChatRow,
    members: &[orch_db::queries::chats::MemberRow],
    standing: &orch_authz::chats::Standing,
) -> ApiResult<Chat> {
    let my = if standing.inspects() || !standing.participating {
        None
    } else {
        following::my_state(
            conn,
            principal.user(),
            row,
            standing.in_chat,
            principal.now().get(),
        )
        .await
        .map_err(ApiError::internal)?
    };
    let people = views::people_in(row, members);
    let mut nodes: Vec<Id> = row.node_id.into_iter().collect();
    // The place an admin's reach comes from is named with its path; the chat's own place keeps
    // its name, which is what the chat is called.
    let admin_places: Vec<Id> = standing
        .admin_place
        .and_then(orch_access_contract::roles::PlaceRef::node)
        .into_iter()
        .chain(standing.admin_access.and_then(|reach| reach.place.node()))
        .filter(|place| row.node_id != Some(*place))
        .collect();
    nodes.extend(admin_places.iter().copied());
    let mut names = lookups(conn, principal, &people, &nodes, &[]).await?;
    names
        .nodes
        .extend(view::qualified_refs(conn, principal, &admin_places, Policy::default()).await?);
    Ok(views::chat(
        &Seen {
            chat: row,
            members,
            standing,
            my: my.as_ref(),
        },
        &names,
    ))
}

/// Only the chats that survived the list's authorization enter this query.
/// Authors and agent-thread names are looked up once for the whole page too.
pub(crate) async fn add_previews(
    conn: &mut Conn,
    principal: &Principal,
    chats: &mut [Chat],
) -> ApiResult<()> {
    if chats.is_empty() {
        return Ok(());
    }
    let ids: Vec<_> = chats.iter().map(|chat| chat.base.id).collect();
    let messages = rows::last_messages(conn, &ids).await?;
    let refs: Vec<_> = messages.iter().collect();
    let names = lookups(
        conn,
        principal,
        &views::people_of(&refs),
        &views::threads_of(&refs),
        &views::groups_of(&refs),
    )
    .await?;
    let by_chat: HashMap<_, _> = messages
        .iter()
        .map(|message| (message.chat_id, message))
        .collect();
    for chat in chats {
        chat.preview = by_chat
            .get(&chat.base.id)
            .map(|row| views::preview(row, &names));
    }
    Ok(())
}

/// A chat just loaded, as the caller sees it.
async fn shown(conn: &mut Conn, principal: &Principal, loaded: &Loaded) -> ApiResult<Chat> {
    chat_view(
        conn,
        principal,
        &loaded.row,
        &loaded.members,
        &loaded.standing,
    )
    .await
}

/// The messages as the caller sees them.
pub async fn messages_view(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
    list: &[MessageRow],
) -> ApiResult<Vec<ChatMessage>> {
    let replies = messages::replies_of(conn, chat, list)
        .await
        .map_err(ApiError::internal)?;
    let everything: Vec<&MessageRow> = list.iter().chain(replies.values()).collect();
    let people = views::people_of(&everything);
    let threads = views::threads_of(&everything);
    let mut names = lookups(
        conn,
        principal,
        &people,
        &threads,
        &views::groups_of(&everything),
    )
    .await?;
    names.nodes.extend(
        view::qualified_refs(
            conn,
            principal,
            &views::role_places_of(&everything),
            Policy::default(),
        )
        .await?,
    );
    Ok(views::messages(list, &replies, &names))
}

async fn message_view(
    conn: &mut Conn,
    principal: &Principal,
    row: &MessageRow,
) -> ApiResult<ChatMessage> {
    messages_view(conn, principal, row.chat_id, std::slice::from_ref(row))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a message was not shown"))
}

fn cursor_text(after: &After) -> String {
    format!("{}|{}", after.at.timestamp_micros(), after.id)
}

fn cursor_of(text: Option<&str>) -> ApiResult<Option<After>> {
    let Some(text) = text else { return Ok(None) };
    let bad = || ApiError::invalid("cursor", "is not a cursor of this list");
    let (micros, id) = text.split_once('|').ok_or_else(bad)?;
    Ok(Some(After {
        at: micros
            .parse::<i64>()
            .ok()
            .and_then(DateTime::<Utc>::from_timestamp_micros)
            .ok_or_else(bad)?,
        id: id.parse().map_err(|_| bad())?,
    }))
}

/// Whether the policy `sharing.allow_external` lets the maker of a group bring
/// somebody from outside its root org into it.
async fn owners_name_outsiders(conn: &mut Conn, root: Id) -> ApiResult<bool> {
    Ok(
        setting::<bool>(conn, Place::Node(root), keys::SHARING_ALLOW_EXTERNAL)
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(true),
    )
}

/// Loads the chat for the handler; a refusal is the problem it comes to.
async fn load(conn: &mut Conn, work: &Work, id: Id, mode: Mode) -> ApiResult<Loaded> {
    let loaded = chats::load(conn, &work.acting(), id, mode).await;
    work.refuse(conn, Some(id), loaded).await
}

/// Notes that an admin opened a chat they are not in.
async fn note_opened(conn: &mut Conn, work: &Work, loaded: &Loaded) -> ApiResult<()> {
    let noted = inspection::note_opened(conn, &work.acting(), loaded).await;
    work.refuse(conn, Some(loaded.id()), noted)
        .await
        .map(|_| ())
}

// ------------------------------------------------------------------ lists and chats

/// The direct and group chats the caller may see in a root org, or the chats
/// of its nodes.
async fn org_chat_list(ctx: Ctx, query: OrgChatListQuery, (): ()) -> ApiResult<Page<Chat>> {
    let root = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let scope = query.scope.unwrap_or(ChatListScope::Mine);
    if query.of.is_some() && scope != ChatListScope::Others {
        return Err(ApiError::invalid(
            "of",
            "only the chats of others are listed by an org",
        ));
    }
    let scope = match scope {
        ChatListScope::Mine => Scope::Mine,
        ChatListScope::Others => Scope::Others { of: query.of },
        ChatListScope::Nodes => Scope::Nodes,
    };
    let limit = i64::from(
        PageQuery {
            limit: query.limit,
            cursor: None,
        }
        .limit(),
    );
    let after = cursor_of(query.cursor.as_deref())?;
    let list_act = Acting {
        principal: &work.principal,
        now: work.now,
        origin: &work.origin,
    };
    let listed = if query.participating.unwrap_or(false) && matches!(scope, Scope::Others { .. }) {
        Ok(chats::Listed {
            chats: Vec::new(),
            next: None,
        })
    } else if query.participating.unwrap_or(false) {
        chats::list_participating(&mut tx, &list_act, root, scope, after, limit).await
    } else {
        chats::list(&mut tx, &list_act, root, scope, after, limit).await
    };
    let listed = work.refuse(&mut tx, None, listed).await?;
    let ids: Vec<Id> = listed.chats.iter().map(|chat| chat.id).collect();
    let mut members = chats::members_of(&mut tx, &ids, work.principal.now())
        .await
        .map_err(ApiError::internal)?;
    let direct = chats::direct_allowed(&mut tx, root)
        .await
        .map_err(ApiError::internal)?;
    let mut items = Vec::new();
    for row in &listed.chats {
        let people = members.remove(&row.id).unwrap_or_default();
        let policy = if row.node_id.is_some() {
            chats::policy_of(&mut tx, row)
                .await
                .map_err(ApiError::internal)?
        } else {
            orch_authz::chats::ChatPolicy {
                enabled: true,
                direct,
            }
        };
        let standing = orch_authz::chats::standing(&mut tx, &work.principal, row, &people, policy)
            .await
            .map_err(ApiError::internal)?;
        if !standing.reads() {
            continue;
        }
        items.push(
            chat_view_without_preview(&mut tx, &work.principal, row, &people, &standing).await?,
        );
    }
    add_previews(&mut tx, &work.principal, &mut items).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Page {
        items,
        next_cursor: listed.next.as_ref().map(cursor_text),
    })
}

/// Opens a chat among people.
async fn org_chat_create(ctx: Ctx, (): (), body: OrgChatCreateBody) -> ApiResult<Answer<Chat>> {
    let root = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let opened = chats::open(
        &mut tx,
        &work.acting(),
        root,
        &body.user_ids,
        body.name.as_deref(),
    )
    .await;
    let opened = work.refuse(&mut tx, None, opened).await?;
    let loaded = load(&mut tx, &work, opened.chat.id, Mode::Read).await?;
    let chat = shown(&mut tx, &work.principal, &loaded).await?;
    tx.commit().await.map_err(DbError::from)?;
    let answer = Answer::from(chat);
    Ok(if opened.created {
        answer
    } else {
        answer.status(StatusCode::OK)
    })
}

/// Reads a chat.
async fn chat_get(ctx: Ctx, (): (), (): ()) -> ApiResult<Chat> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    note_opened(&mut tx, &work, &loaded).await?;
    let chat = shown(&mut tx, &work.principal, &loaded).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(chat)
}

/// Renames a group.
async fn chat_update(ctx: Ctx, (): (), body: ChatUpdateBody) -> ApiResult<Chat> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Write).await?;
    let renamed = chats::rename(&mut tx, &work.acting(), &mut loaded, body.name.as_deref()).await;
    let row = work.refuse(&mut tx, Some(id), renamed).await?;
    loaded.row = row;
    let chat = shown(&mut tx, &work.principal, &loaded).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(chat)
}

/// Puts a person in a group.
async fn join_request(
    mut ctx: Ctx,
    (): (),
    body: orch_api_contract::chats::ChatJoinRequestBody,
) -> ApiResult<orch_api_contract::access::AccessRequestCreateReply> {
    if body.reason.chars().count() > orch_authz::requests::REASON_MAX_CHARS {
        return Err(ApiError::invalid("reason", "is too long"));
    }
    let chat = ctx.id("id")?;
    let (tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    if body.expires_at.is_some_and(|end| end <= principal.now()) {
        return Err(ApiError::invalid("expires_at", "must be in the future"));
    }
    let raised = ctx
        .app
        .engine
        .ask_to_join_guarded(
            tx,
            principal.clone(),
            chat,
            body.reason.trim(),
            body.label.as_deref(),
            body.expires_at,
            &origin(&ctx),
        )
        .await
        .map_err(crate::routes::access::requests::raise_failed)?;
    let mut conn = ctx.app.db.acquire().await?;
    let approvals = crate::routes::approvals::views(
        &mut conn,
        &principal,
        &raised.approvals,
        ctx.now,
        &ctx.app.engine,
    )
    .await?;
    Ok(orch_api_contract::access::AccessRequestCreateReply {
        batch_id: raised.batch_id,
        approvals,
    })
}

async fn member_add(ctx: Ctx, (): (), (): ()) -> ApiResult<Chat> {
    let (id, user) = (ctx.id("id")?, ctx.id("user_id")?);
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Write).await?;
    let outsiders = owners_name_outsiders(&mut tx, loaded.row.org_id).await?;
    let added = chats::add_member(&mut tx, &work.acting(), &mut loaded, user, outsiders).await;
    work.refuse(&mut tx, Some(id), added).await?;
    loaded.row = rows::chat(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let chat = shown(&mut tx, &work.principal, &loaded).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(chat)
}

/// Takes a person out of a group, or lets them leave.
async fn member_remove(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (id, user) = (ctx.id("id")?, ctx.id("user_id")?);
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Write).await?;
    let removed = chats::remove_member(&mut tx, &work.acting(), &mut loaded, user).await;
    work.refuse(&mut tx, Some(id), removed).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ------------------------------------------------------------------ reading messages

/// Keep the native conversation and draft checks, including inspection's private-draft refusal.
async fn opening_get(ctx: Ctx, query: ChatSyncGetQuery, (): ()) -> ApiResult<ChatOpening> {
    if query.since.is_some() {
        return Err(ApiError::invalid(
            "since",
            "an opening starts without a position",
        ));
    }
    let sync = sync_get(ctx.clone(), query, ()).await?;
    let draft = match draft_get(ctx, (), ()).await {
        Ok(draft) => draft,
        Err(error) if error.problem.status == 403 => None,
        Err(error) => return Err(error),
    };
    Ok(ChatOpening { sync, draft })
}

/// A cached path names candidates, never admits their chats. Each scope is checked here now.
async fn node_opening_get(
    ctx: Ctx,
    query: NodeChatOpeningGetQuery,
    (): (),
) -> ApiResult<NodeChatOpening> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let node = view::visible(&mut tx, &work.principal, &[id], work.now, Policy::default())
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    let (kind, path) = match &node {
        Node::Org(node) => (NodeKind::Org, &node.base.path),
        Node::Project(node) => (NodeKind::Project, &node.base.path),
        Node::Topic(node) => (NodeKind::Topic, &node.base.path),
        Node::Thread(node) => (NodeKind::Thread, &node.base.path),
    };
    let wanted: &[NodeKind] = match kind {
        NodeKind::Org => &[NodeKind::Org],
        NodeKind::Project => &[NodeKind::Project],
        NodeKind::Topic => &[NodeKind::Topic, NodeKind::Project],
        NodeKind::Thread => &[NodeKind::Thread, NodeKind::Topic, NodeKind::Project],
    };
    let candidates: Vec<Id> = wanted
        .iter()
        .filter_map(|kind| {
            path.iter()
                .rev()
                .find(|node| node.kind == *kind && !node.redacted)
                .map(|node| node.id)
        })
        .collect();
    let visible = view::visible(
        &mut tx,
        &work.principal,
        &candidates,
        work.now,
        Policy::default(),
    )
    .await?;
    let mut scopes = Vec::new();
    for node in visible {
        let (id, kind, name, chat_id) = match node {
            Node::Org(node) => (
                node.base.id,
                NodeKind::Org,
                node.base.name,
                node.base.chat_id,
            ),
            Node::Project(node) => (
                node.base.id,
                NodeKind::Project,
                node.base.name,
                node.base.chat_id,
            ),
            Node::Topic(node) => (
                node.base.id,
                NodeKind::Topic,
                node.base.name,
                node.base.chat_id,
            ),
            Node::Thread(node) => (
                node.base.id,
                NodeKind::Thread,
                node.base.name,
                node.base.chat_id,
            ),
        };
        match load(&mut tx, &work, chat_id, Mode::Read).await {
            Ok(loaded) => scopes.push(NodeChatScope {
                node: NodeRef {
                    thread_display: None,
                    id,
                    kind,
                    name: Some(name),
                    redacted: false,
                },
                chat: TreeChat {
                    id: chat_id,
                    seq: loaded.row.seq(),
                },
            }),
            Err(error) if matches!(error.problem.status, 403 | 404) => {}
            Err(error) => return Err(error),
        }
    }
    let chosen = query
        .chat_id
        .and_then(|id| scopes.iter().find(|scope| scope.chat.id == id))
        .or_else(|| {
            query
                .scope
                .and_then(|kind| scopes.iter().find(|scope| scope.node.kind == kind))
        })
        .or_else(|| scopes.first())
        .map(|scope| scope.chat.id);
    tx.commit().await.map_err(DbError::from)?;
    let opening = if let Some(id) = chosen {
        let mut child = ctx.for_id(id);
        child.route = ChatOpeningGet::ROUTE;
        Some(
            opening_get(
                child,
                ChatSyncGetQuery {
                    since: None,
                    limit: query.limit,
                },
                (),
            )
            .await?,
        )
    } else {
        None
    };
    Ok(NodeChatOpening { scopes, opening })
}

/// A chat and its latest messages, or what changed since a sequence.
async fn sync_get(ctx: Ctx, query: ChatSyncGetQuery, (): ()) -> ApiResult<ChatSync> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    note_opened(&mut tx, &work, &loaded).await?;
    if query.since.is_some_and(|since| since < 0) {
        return Err(ApiError::invalid(
            "since",
            "is a position in the chat, from 0",
        ));
    }
    let limit = work
        .refuse(&mut tx, Some(id), messages::limit_of(query.limit))
        .await?;
    let synced = messages::sync(&mut tx, &loaded.row, query.since, limit).await;
    let synced = work.refuse(&mut tx, Some(id), synced).await?;
    let chat = shown(&mut tx, &work.principal, &loaded).await?;
    let list = messages_view(&mut tx, &work.principal, id, &synced.messages).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(ChatSync {
        seq: chat.seq,
        chat,
        since: query.since,
        complete: synced.complete,
        messages: list,
        has_older: synced.has_older,
    })
}

/// A stretch of a chat's messages.
async fn message_list(
    ctx: Ctx,
    query: ChatMessageListQuery,
    (): (),
) -> ApiResult<ChatMessageListReply> {
    let id = ctx.id("id")?;
    let asked: Vec<Window> = [
        query.before.map(Window::Before),
        query.after.map(Window::After),
        query.around.map(Window::Around),
        query
            .pinned
            .filter(|pinned| *pinned)
            .map(|_| Window::Pinned),
        query.item_id.map(Window::Quoting),
    ]
    .into_iter()
    .flatten()
    .collect();
    let [window] = asked.as_slice() else {
        return Err(ApiError::invalid(
            "",
            "name exactly one of before, after, around, pinned and item_id",
        ));
    };
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    note_opened(&mut tx, &work, &loaded).await?;
    let limit = work
        .refuse(&mut tx, Some(id), messages::limit_of(query.limit))
        .await?;
    let slice = messages::window(&mut tx, &loaded.row, *window, limit).await;
    let slice = work.refuse(&mut tx, Some(id), slice).await?;
    let list = messages_view(&mut tx, &work.principal, id, &slice.messages).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(ChatMessageListReply {
        messages: list,
        has_older: slice.has_older,
        has_newer: slice.has_newer,
    })
}

// ------------------------------------------------------------------ writing messages

/// The redactor for what is written in a chat.
async fn redactor(conn: &mut Conn, work: &Work, loaded: &Loaded) -> ApiResult<redaction::Redactor> {
    let made = redaction::for_chat(conn, &loaded.row, &keys).await;
    work.refuse(conn, Some(loaded.id()), made).await
}

/// Sends a message.
async fn message_create(
    ctx: Ctx,
    (): (),
    body: ChatMessageCreateBody,
) -> ApiResult<Answer<ChatMessageCreateReply>> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Read).await?;
    work.refuse(&mut tx, Some(id), loaded.ensure_send()).await?;
    // A retry returns the stored acceptance before reviewing a changed model or binding.
    if let Some(previous) =
        rows::message_by_client(&mut tx, id, work.principal.user(), body.client_id.trim()).await?
    {
        let message = message_view(&mut tx, &work.principal, &previous).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(Answer::from(ChatMessageCreateReply {
            message,
            seq: loaded.row.seq(),
        })
        .status(StatusCode::OK));
    }
    work.refuse(&mut tx, Some(id), messages::check_parts(&body.parts, None))
        .await?;
    let targets = asking::targets(&body.parts)?;
    if !targets.is_empty() {
        asking::lock_threads(&mut tx, &asking::locks(&body.parts, &targets)).await?;
        loaded.lock(&mut tx).await.map_err(ApiError::internal)?;
        if let Some(previous) =
            rows::message_by_client(&mut tx, id, work.principal.user(), body.client_id.trim())
                .await?
        {
            let message = message_view(&mut tx, &work.principal, &previous).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Answer::from(ChatMessageCreateReply {
                message,
                seq: loaded.row.seq(),
            })
            .status(StatusCode::OK));
        }
        let review = asking::review(
            &mut tx,
            &work.principal,
            id,
            &body.parts,
            body.reply_to,
            &targets,
        )
        .await?;
        if body.agent_review != Some(review.fingerprint) {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "review the current queued inputs before asking these agents",
            ));
        }
    }
    let masking = redactor(&mut tx, &work, &loaded).await?;
    let written = messages::post(
        &mut tx,
        &work.acting(),
        &mut loaded,
        Post {
            parts: body.parts,
            reply_to: body.reply_to,
            client_id: body.client_id,
        },
        &masking,
    )
    .await;
    let mut written = work.refuse(&mut tx, Some(id), written).await?;
    let queued = if written.created && !targets.is_empty() {
        let done =
            asking::forward_message(&ctx, &mut tx, &work, &written.message, &targets).await?;
        let asked = serde_json::to_value(
            done.inputs
                .iter()
                .map(|input| json!({ "thread_id":input.thread,"input_id":input.input.id }))
                .collect::<Vec<_>>(),
        )
        .map_err(ApiError::internal)?;
        rows::people::asked(&mut tx, written.message.id, &asked).await?;
        written.message.asked_agents = asked;
        Some(done)
    } else {
        None
    };
    let message = message_view(&mut tx, &work.principal, &written.message).await?;
    tx.commit().await.map_err(DbError::from)?;
    if written.created {
        metrics::counter!("orch_chat_messages_total","kind"=>loaded.row.kind.clone()).increment(1);
    }
    if let Some(done) = &queued {
        ctx.app.engine.after_forward(done);
    }
    let answer = Answer::from(ChatMessageCreateReply {
        message,
        seq: written.seq,
    });
    Ok(if written.created {
        answer
    } else {
        answer.status(StatusCode::OK)
    })
}

/// Edits a message.
async fn message_update(ctx: Ctx, (): (), body: ChatMessageUpdateBody) -> ApiResult<ChatMessage> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let masking = redactor(&mut tx, &work, &loaded).await?;
    let edited = messages::edit(
        &mut tx,
        &work.acting(),
        &mut loaded,
        message,
        body.parts,
        &masking,
    )
    .await;
    let row = work.refuse(&mut tx, Some(id), edited).await?;
    let shown = message_view(&mut tx, &work.principal, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Deletes a message.
async fn message_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessage> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let deleted = messages::delete(&mut tx, &work.acting(), &mut loaded, message).await;
    let row = work.refuse(&mut tx, Some(id), deleted).await?;
    let shown = message_view(&mut tx, &work.principal, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// The emoji of a path, as a client wrote it.
fn emoji_of(ctx: &Ctx) -> ApiResult<String> {
    Ok(ctx.param("emoji")?.to_owned())
}

async fn react(ctx: &Ctx, add: bool) -> ApiResult<ChatMessage> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let emoji = emoji_of(ctx)?;
    let mut tx = transaction(ctx).await?;
    let work = Work::begin(ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Write).await?;
    let changed = messages::react(&mut tx, &work.acting(), &mut loaded, message, &emoji, add).await;
    let row = work.refuse(&mut tx, Some(id), changed).await?;
    let shown = message_view(&mut tx, &work.principal, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Adds the caller's reaction.
async fn reaction_add(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessage> {
    react(&ctx, true).await
}

/// Takes the caller's reaction away.
async fn reaction_remove(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessage> {
    react(&ctx, false).await
}

async fn pin(ctx: &Ctx, pinned: bool) -> ApiResult<ChatMessage> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let mut tx = transaction(ctx).await?;
    let work = Work::begin(ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Write).await?;
    let changed = messages::pin(&mut tx, &work.acting(), &mut loaded, message, pinned).await;
    let row = work.refuse(&mut tx, Some(id), changed).await?;
    let shown = message_view(&mut tx, &work.principal, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Pins a message.
async fn pin_set(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessage> {
    pin(&ctx, true).await
}

/// Takes the pin off a message.
async fn pin_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessage> {
    pin(&ctx, false).await
}

// ------------------------------------------------------------------ reading and following

/// Moves the caller's read position forward.
async fn chat_read(ctx: Ctx, (): (), body: ChatReadBody) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let read = following::mark_read_observed(
        &mut tx,
        &work.acting(),
        &loaded,
        body.seq,
        body.read_revision,
        body.unread_from.unwrap_or(false),
    )
    .await;
    work.refuse(&mut tx, Some(id), read).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Marks every chat of an org that the caller follows as read.
async fn org_chats_read(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let org = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let root = hierarchy::with_ancestors(&mut tx, &[org])
        .await?
        .into_iter()
        .find(|node| node.id == org && node.kind == "org")
        .map(|node| node.org_id)
        .ok_or_else(ApiError::not_found)?;
    let marked = following::mark_all_read(&mut tx, &work.acting(), root).await;
    work.refuse(&mut tx, None, marked).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Sets the bell and the hiding of a chat.
async fn state_set(ctx: Ctx, (): (), body: ChatStateSetBody) -> ApiResult<ChatState> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let changed = following::set_state(
        &mut tx,
        &work.acting(),
        &loaded,
        following::Change {
            level: body.level,
            hidden: body.hidden,
        },
    )
    .await;
    let state = work.refuse(&mut tx, Some(id), changed).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(state)
}

/// The caller's state in every chat they have one in.
async fn me_chat_states(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<ChatState>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    states_for(&ctx, &mut conn, &principal).await
}

/// Personal chat state through the same standing as its individual read, without chat contents.
pub(crate) async fn states_for(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<Vec<ChatState>> {
    let user = principal.user();
    let now = principal.now().get();
    let origin = origin(ctx);
    let acting = Acting {
        principal,
        now,
        origin: &origin,
    };
    let stored = rows::states_of(conn, user).await?;
    let member_of = rows::chats_of_member(conn, user).await?;
    let mut ids: Vec<Id> = stored.iter().map(|state| state.chat_id).collect();
    ids.extend(member_of.iter().copied());
    ids.extend(
        rows::following::derived(conn, Some(user), None, now)
            .await?
            .into_iter()
            .map(|reason| reason.chat_id),
    );
    ids.sort_unstable();
    ids.dedup();
    let mut states = Vec::new();
    for id in ids {
        let loaded = match chats::load(conn, &acting, id, Mode::Read).await {
            Ok(loaded) if !loaded.standing.inspects() => loaded,
            Err(ChatError::Db(error)) => return Err(error.into()),
            Err(ChatError::Internal(error)) => return Err(ApiError::internal(error)),
            _ => continue,
        };
        if let Some(state) =
            following::my_state(conn, user, &loaded.row, loaded.standing.in_chat, now)
                .await
                .map_err(ApiError::internal)?
        {
            states.push(state);
        }
    }
    Ok(states)
}

// ------------------------------------------------------------------ drafts

/// The caller's draft in a chat, if they have one.
async fn draft_get(ctx: Ctx, (): (), (): ()) -> ApiResult<Option<ChatDraft>> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    if loaded.standing.inspects() {
        let refused = Err(ChatError::Inspecting);
        return work.refuse(&mut tx, Some(id), refused).await;
    }
    let draft = following::draft(&mut tx, &work.acting(), id).await;
    let mut draft = work.refuse(&mut tx, Some(id), draft).await?;
    if let Some(draft) = &mut draft {
        mentions::draft_names(&mut tx, &work.principal, draft).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(draft)
}

/// Saves the caller's draft.
async fn draft_set(ctx: Ctx, (): (), body: ChatDraftSetBody) -> ApiResult<ChatDraft> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let saved =
        following::save_draft(&mut tx, &work.acting(), &loaded, &body.parts, body.reply_to).await;
    let mut draft = work.refuse(&mut tx, Some(id), saved).await?;
    mentions::draft_names(&mut tx, &work.principal, &mut draft).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(draft)
}

/// Throws the caller's draft away.
async fn draft_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let discarded = following::discard_draft(&mut tx, &work.acting(), &loaded).await;
    work.refuse(&mut tx, Some(id), discarded).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ------------------------------------------------------------------ forwarding

fn destination_of(to: &ForwardDestination) -> Destination {
    match to {
        ForwardDestination::Chat(chat) => Destination::Chat(chat.chat_id),
        ForwardDestination::User(user) => Destination::User(user.user_id),
        ForwardDestination::Thread(thread) => Destination::Thread(thread.thread_id),
    }
}

/// What a forward carries to its destinations.
struct Carried {
    /// The root org the source belongs to.
    root: Id,
    /// Copies of what is forwarded, which any chat or thread takes.
    copies: Vec<ChatPart>,
    /// For items of a thread: live quotes of them, and the chat of that thread, which is where quotes go.
    quotes: Option<(Vec<ChatPart>, Id)>,
}

/// The parts of a forward as the parts of an input.
fn input_parts(parts: &[ChatPart]) -> Vec<ContentPart> {
    parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Text { text } => Some(ContentPart::Text { text: text.clone() }),
            ChatPart::Forwarded(copy) => Some(ContentPart::Forwarded(copy.clone())),
            ChatPart::PastedText(paste) => Some(ContentPart::PastedText(paste.clone())),
            _ => None,
        })
        .collect()
}

/// Forwards messages of a chat, or items of a thread, to chats, people and agents' threads.
///
/// Into the chat of the very thread the items come from, they arrive as live quotes; anywhere else they arrive as
/// copies in the conversation's written form, one part for each message of a person and one for what the agent
/// said and did in a turn. An agent's thread takes the same parts as an input of the forwarder's. Everything is
/// written in one transaction.
#[allow(
    clippy::too_many_lines,
    reason = "one request, in the order it is answered: the source, the destinations of chats, then those of threads"
)]
async fn forward_create(
    ctx: Ctx,
    (): (),
    body: ForwardCreateBody,
) -> ApiResult<Answer<ForwardCreateReply>> {
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let act = work.acting();
    let to: Vec<Destination> = body.to.iter().map(destination_of).collect();
    if to.is_empty() || to.len() > usize::try_from(FORWARD_MAX_DESTINATIONS).unwrap_or(usize::MAX) {
        return Err(ApiError::invalid(
            "to",
            &format!("name 1 to {FORWARD_MAX_DESTINATIONS} destinations"),
        ));
    }
    let (chats_to, threads_to): (Vec<Destination>, Vec<Destination>) = to
        .iter()
        .copied()
        .partition(|each| !matches!(each, Destination::Thread(_)));
    let thread_ids: Vec<Id> = threads_to
        .iter()
        .filter_map(|each| match each {
            Destination::Thread(thread) => Some(*thread),
            _ => None,
        })
        .collect();
    let carried = match &body.from {
        ForwardSource::Chat(from) => {
            let prepared = forward::prepare(&mut tx, &act, from.chat_id, &from.message_ids).await;
            let prepared = work.refuse(&mut tx, Some(from.chat_id), prepared).await?;
            let people = forward::people_of(&prepared);
            let source_rows: Vec<_> = prepared.messages.iter().collect();
            let nodes: Vec<Id> = prepared
                .chat
                .row
                .node_id
                .into_iter()
                .chain(views::threads_of(&source_rows))
                .collect();
            let mut names = lookups(
                &mut tx,
                &work.principal,
                &people,
                &nodes,
                &views::groups_of(&source_rows),
            )
            .await?;
            names.nodes.extend(
                view::qualified_refs(
                    &mut tx,
                    &work.principal,
                    &views::role_places_of(&source_rows),
                    Policy::default(),
                )
                .await?,
            );
            let source = chat_view(
                &mut tx,
                &work.principal,
                &prepared.chat.row,
                &prepared.chat.members,
                &prepared.chat.standing,
            )
            .await?;
            Carried {
                root: prepared.chat.row.org_id,
                copies: forward::copies_of(&prepared, &source.base, &names),
                quotes: None,
            }
        }
        ForwardSource::Thread(from) => {
            let found = hierarchy::with_ancestors(&mut tx, &[from.thread_id]).await?;
            let node = found
                .iter()
                .find(|node| node.id == from.thread_id && node.deleted_at.is_none())
                .filter(|node| refusals::node_kind(&node.kind) == Some(NodeKind::Thread))
                .ok_or_else(ApiError::not_found)?;
            let own = rows::chat_of_node(&mut tx, from.thread_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            // Whoever forwards what a conversation says reads it.
            match nodes::check(
                &mut tx,
                &work.principal,
                from.thread_id,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await
            {
                Ok(Decision::Allowed { .. }) => {}
                Ok(Decision::Refused(refusal)) => {
                    return Err(refusals::denied(&mut tx, refusal).await?);
                }
                Err(error) => return Err(refusals::problem(&mut tx, error).await),
            }
            let quotes = forward::quote_items(&mut tx, from.thread_id, &from.item_ids).await;
            let quotes = work.refuse(&mut tx, Some(own.id), quotes).await?;
            let parts = written::forward_parts(&mut tx, from.thread_id, &from.item_ids)
                .await
                .map_err(ApiError::internal)?;
            if parts.len() > usize::try_from(FORWARD_MAX_PARTS).unwrap_or(usize::MAX) {
                return Err(ApiError::invalid(
                    "from.item_ids",
                    &format!("come to more than the {FORWARD_MAX_PARTS} parts of a forward"),
                ));
            }
            let refs = view::refs(
                &mut tx,
                &work.principal,
                &[from.thread_id],
                Policy::default(),
            )
            .await?;
            let thread_ref = refs
                .get(&from.thread_id)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            let copies: Vec<ChatPart> = parts
                .into_iter()
                .filter(|part| !part.text.trim().is_empty() || !part.images.is_empty())
                .map(|part| {
                    ChatPart::Forwarded(Box::new(Forwarded {
                        r#type: orch_api_contract::threads::ForwardedType::Forwarded,
                        from: ForwardedFrom::Thread {
                            thread: thread_ref.clone(),
                        },
                        source_id: part.source,
                        author: match (part.person, part.agent) {
                            (Some(person), _) => Some(ForwardedAuthor::User(person)),
                            (None, Some(agent)) => {
                                Some(ForwardedAuthor::Agent(AgentName { agent }))
                            }
                            (None, None) => None,
                        },
                        at: part.at,
                        text: part.text,
                        images: part.images,
                        attachments: Vec::new(),
                    }))
                })
                .collect();
            if copies.is_empty() {
                return Err(ApiError::invalid("from.item_ids", "say nothing to forward"));
            }
            Carried {
                root: node.org_id,
                copies,
                quotes: Some((quotes, own.id)),
            }
        }
    };
    // Shared lock order: authority, sorted source/destination threads, then destination chats.
    let mut locks = thread_ids.clone();
    if let ForwardSource::Thread(from) = &body.from {
        locks.push(from.thread_id);
    }
    asking::lock_threads(&mut tx, &locks).await?;
    // Chats and people: into the chat of the thread the items come from they are quotes, elsewhere copies.
    let mut sent: Vec<forward::Sent> = Vec::new();
    if !chats_to.is_empty() {
        let (home, elsewhere): (Vec<Destination>, Vec<Destination>) = match &carried.quotes {
            Some((_, own)) => chats_to
                .iter()
                .copied()
                .partition(|each| matches!(each, Destination::Chat(chat) if chat == own)),
            None => (Vec::new(), chats_to.clone()),
        };
        if let (false, Some((quotes, own))) = (home.is_empty(), &carried.quotes) {
            let done = forward::deliver(
                &mut tx,
                &act,
                &Delivery {
                    root: carried.root,
                    content: quotes.clone(),
                    only: Some(*own),
                    to: &home,
                    comment: body.comment.as_deref(),
                    client_id: &body.client_id,
                },
                &keys,
            )
            .await;
            sent.extend(work.refuse(&mut tx, None, done).await?);
        }
        if !elsewhere.is_empty() {
            let done = forward::deliver(
                &mut tx,
                &act,
                &Delivery {
                    root: carried.root,
                    content: carried.copies.clone(),
                    only: None,
                    to: &elsewhere,
                    comment: body.comment.as_deref(),
                    client_id: &body.client_id,
                },
                &keys,
            )
            .await;
            sent.extend(work.refuse(&mut tx, None, done).await?);
        }
    }
    sent.sort_by_key(|each| {
        chats_to
            .iter()
            .position(|asked| matches!(asked, Destination::Chat(chat) if *chat == each.chat))
            .unwrap_or(usize::MAX)
    });
    // Agents' threads: one input of the forwarder's for each, with the same parts.
    let mut queued = None;
    if !thread_ids.is_empty() {
        let mut parts: Vec<ContentPart> = Vec::new();
        if let Some(comment) = body
            .comment
            .as_deref()
            .filter(|comment| !comment.trim().is_empty())
        {
            parts.push(ContentPart::Text {
                text: comment.to_owned(),
            });
        }
        parts.extend(input_parts(&carried.copies));
        let caller = authorization::caller_of(ctx.user()?)?;
        match ctx
            .app
            .engine
            .forward_to_threads(&mut tx, &caller, &thread_ids, &parts, &body.client_id)
            .await
        {
            Ok(done) => queued = Some(done),
            Err((thread, error)) => {
                let mut conn = ctx.app.db.acquire().await?;
                let failed = crate::routes::threads::input_failed(
                    &mut conn,
                    thread.unwrap_or_else(Id::nil),
                    error,
                )
                .await;
                return Err(match thread {
                    Some(thread) => failed.with("object", Destination::Thread(thread).named()),
                    None => failed,
                });
            }
        }
    }
    let mut chats_written = Vec::new();
    let mut every_one_before = true;
    for each in &sent {
        every_one_before &= !each.written.created;
        chats_written.push(ForwardChatMessage {
            chat_id: each.chat,
            message: message_view(&mut tx, &work.principal, &each.written.message).await?,
        });
    }
    let mut threads_written = Vec::new();
    if let Some(done) = &queued {
        for each in &done.inputs {
            every_one_before &= !each.created;
            threads_written.push(ForwardThreadInput {
                thread_id: each.thread,
                input: each.input.clone(),
            });
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    if let Some(done) = &queued {
        ctx.app.engine.after_forward(done);
    }
    let answer = Answer::from(ForwardCreateReply {
        chats: chats_written,
        threads: threads_written,
    });
    Ok(if every_one_before {
        answer.status(StatusCode::OK)
    } else {
        answer
    })
}
