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
mod defaults;
mod files;
mod forwarding;
mod group_mutation;
mod mention_operation;
mod mentions;
mod message_edit;
mod mutation;
mod names;
mod node_opening;
mod operation;
mod personal;
pub(crate) mod read_operation;
mod readers;

use crate::key_operations::resolver::Disclosure;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
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
    Chat, ChatChangeResponse, ChatDraft, ChatDraftDelete, ChatDraftGet, ChatDraftSet,
    ChatDraftSetBody, ChatDraftSetResponse, ChatGet, ChatListScope, ChatMemberAdd,
    ChatMemberRemove, ChatMessage, ChatMessageCreate, ChatMessageCreateBody,
    ChatMessageCreateReply, ChatMessageCreateResponse, ChatMessageDelete, ChatMessageList,
    ChatMessageListQuery, ChatMessageListReply, ChatMessageMutationResponse, ChatMessagePinDelete,
    ChatMessagePinSet, ChatMessageReactionAdd, ChatMessageReactionRemove, ChatMessageReceipt,
    ChatMessageUpdate, ChatMessageUpdateBody, ChatOpening, ChatOpeningGet, ChatPersonalReceipt,
    ChatRead, ChatReadBody, ChatState, ChatStateSet, ChatStateSetBody, ChatStateSetResponse,
    ChatSync, ChatSyncGet, ChatSyncGetQuery, ChatUpdate, ChatUpdateBody, ForwardChatMessage,
    ForwardChatReceipt, ForwardCreate, ForwardCreateBody, ForwardCreateReceipt, ForwardCreateReply,
    ForwardCreateResponse, ForwardDestination, ForwardInputReceipt, ForwardSource, NodeChatOpening,
    NodeChatOpeningGet, NodeChatOpeningGetQuery, NodeChatScope, OrgChatCreate, OrgChatCreateBody,
    OrgChatList, OrgChatListQuery, OrgChatsRead,
};
use orch_api_contract::chats::{
    ChatPart, FORWARD_MAX_DESTINATIONS, FORWARD_MAX_PARTS, ForwardThreadInput,
};
use orch_api_contract::common::{ErrorCode, Page, PageQuery, ReceiptProjection};
use orch_api_contract::nodes::{NodeKind, NodeRef, TreeChat};
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
        .on_with_operation(ChatGet, read_operation::get, chat_get)
        .on_with_operation(ChatUpdate, group_mutation::update, chat_update)
        .on_with_operation(ChatMemberAdd, group_mutation::add, member_add)
        .on_with_optional_operation(
            orch_api_contract::chats::ChatJoinRequest,
            crate::routes::access::request_operation::join,
            join_request,
        )
        .on_with_operation(ChatMemberRemove, group_mutation::remove, member_remove)
        .on_with_operation(ChatSyncGet, read_operation::sync, sync_get)
        .on_with_operation(ChatOpeningGet, read_operation::opening, opening_get)
        .on_with_operation(NodeChatOpeningGet, node_opening::prepare, node_opening_get)
        .on_with_operation(ChatMessageList, read_operation::list, message_list)
        .on_with_operation(ChatMessageCreate, operation::resolve, message_create)
        .on_with_operation(ChatMessageUpdate, message_edit::resolve, message_update)
        .on_with_operation(ChatMessageDelete, mutation::delete, message_delete)
        .on_with_operation(ChatMessageReactionAdd, mutation::direct, reaction_add)
        .on_with_operation(ChatMessageReactionRemove, mutation::direct, reaction_remove)
        .on_with_operation(ChatMessagePinSet, mutation::direct, pin_set)
        .on_with_operation(ChatMessagePinDelete, mutation::direct, pin_delete)
        .on_with_operation(ChatRead, crate::key_operations::direct::declared, chat_read)
        .on(OrgChatsRead, org_chats_read)
        .on_with_operation(
            ChatStateSet,
            crate::key_operations::direct::declared,
            state_set,
        )
        .on_with_operation(ChatDraftGet, personal::get, draft_get)
        .on_with_operation(ChatDraftSet, personal::set, draft_set)
        .on_with_operation(
            ChatDraftDelete,
            crate::key_operations::direct::declared,
            draft_delete,
        )
        .on_with_operation(ForwardCreate, forwarding::prepare, forward_create)
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
        ChatError::StaleDestination => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the direct destination changed during preparation; retry the same request",
        ),
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

/// Choose a typed refusal before any rich identity/permission-page projection is built.
async fn failed_projected(
    conn: &mut Conn,
    principal: &Principal,
    chat: Option<Id>,
    error: ChatError,
    receipt_only: bool,
) -> ApiError {
    if !receipt_only {
        return failed(conn, principal, chat, error).await;
    }
    match error {
        ChatError::Suspended { org } => ApiError::new(
            if org.is_some() {
                ErrorCode::OrgSuspended
            } else {
                ErrorCode::AuthSuspended
            },
            "the caller is suspended",
        ),
        ChatError::Archived { .. } => {
            ApiError::new(ErrorCode::NodeArchived, "this is archived and read-only")
        }
        ChatError::Lacks { permission, .. } => denied_for(
            chat,
            permission.as_str(),
            "the caller lacks the required permission",
        ),
        ChatError::Outsider { .. } => ApiError::new(
            ErrorCode::SharingExternalForbidden,
            "this caller may not bring somebody from outside into this chat",
        ),
        ChatError::Destination { error, .. } => {
            Box::pin(failed_projected(conn, principal, chat, *error, true)).await
        }
        safe @ (ChatError::Db(_)
        | ChatError::Authz(_)
        | ChatError::Settings(_)
        | ChatError::NotFound
        | ChatError::MentionRestricted
        | ChatError::StaleDestination
        | ChatError::StaleRead { .. }
        | ChatError::StaleMessage { .. }
        | ChatError::NotAllowed(_)
        | ChatError::Inspecting
        | ChatError::Invalid { .. }
        | ChatError::NotTheAuthor
        | ChatError::Unmasked(_)
        | ChatError::Unsupported(_)
        | ChatError::Internal(_)) => {
            authorization::problem_without_content(failed(conn, principal, chat, safe).await)
        }
    }
}

/// What a handler works with: the connection or transaction, who acts and when.
struct Work {
    // The shared request disclosure decision can narrow at a later guarded checkpoint.
    projection_ctx: Ctx,
    principal: Principal,
    origin: orch_db::audit::Origin,
    now: DateTime<Utc>,
}

impl Work {
    async fn begin(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Self> {
        let principal = authorization::principal(ctx, conn).await?;
        let now = principal.now().get();
        Ok(Self {
            projection_ctx: ctx.clone(),
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
            Err(error) => Err(failed_projected(
                conn,
                &self.principal,
                chat,
                error,
                self.projection_ctx.key_receipt_required(),
            )
            .await),
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
    Ok(
        names::lookups(conn, principal, people, nodes, groups, &[], None)
            .await?
            .lookups,
    )
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
    chat_base(conn, principal, row, members, standing, None).await
}

/// The existing chat header/state projection with an optional admitted live-name bound.
async fn chat_base(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChatRow,
    members: &[orch_db::queries::chats::MemberRow],
    standing: &orch_authz::chats::Standing,
    allowed: Option<&names::AllowSet>,
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
    let names = if let Some(allowed) = allowed {
        names::lookups(
            conn,
            principal,
            &people,
            &nodes,
            &[],
            &admin_places,
            Some(allowed),
        )
        .await?
        .lookups
    } else {
        let mut names = lookups(conn, principal, &people, &nodes, &[]).await?;
        names
            .nodes
            .extend(view::qualified_refs(conn, principal, &admin_places, Policy::default()).await?);
        names
    };
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
    Ok(names::messages(conn, principal, chat, list, None)
        .await?
        .messages)
}

#[expect(
    dead_code,
    reason = "retained single-message wrapper over the shared message projection"
)]
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
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<read_operation::PreparedRead>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(&ctx),
    };
    note_opened(&mut tx, &work, &prepared.loaded).await?;
    let chat = read_operation::project(&mut tx, &work.principal, &prepared)
        .await?
        .chat
        .ok_or_else(|| ApiError::internal("the chat read has no chat projection"))?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(chat)
}

/// Renames a group.
async fn chat_update(ctx: Ctx, (): (), _body: ChatUpdateBody) -> ApiResult<ChatChangeResponse> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, guard) = group_mutation::begin(&ctx).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        origin: origin(&ctx),
        now: guard.now().get(),
    };
    let group_mutation::PreparedGroup { mut loaded, effect } = guard
        .operation_data::<group_mutation::PreparedGroup>()?
        .as_ref()
        .clone();
    let group_mutation::Effect::Rename(prepared) = effect else {
        return Err(ApiError::internal("the group rename operands changed kind"));
    };
    guard.check_deadline(&ctx)?;
    let renamed = chats::apply_rename(&mut tx, &work.acting(), &mut loaded, prepared).await;
    operation::refuse(&work, &mut tx, id, renamed).await?;
    let full = group_mutation::response(&ctx, &mut tx, &work.principal).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(group_mutation::finish(&ctx, id, full))
}

/// Puts a person in a group.
async fn join_request(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::chats::ChatJoinRequestBody,
) -> ApiResult<orch_api_contract::access::AccessRequestWriteResponse> {
    if body.reason.chars().count() > orch_authz::requests::REASON_MAX_CHARS {
        return Err(ApiError::invalid("reason", "is too long"));
    }
    let chat = ctx.id("id")?;
    let (mut tx, initial) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let prefix = ctx
        .app
        .engine
        .prepare_join_request_prefix(&mut tx, initial.user().id(), chat)
        .await;
    // A failed transaction cannot run a new identity query. Preserve its real internal
    // cause, but apply the same pure HTTP deadline cut before publishing any held error.
    initial.check_deadline(&ctx)?;
    let prefix = match prefix {
        Err(error @ orch_engine::approvals::access::RaiseError::Internal(_)) => {
            return Err(crate::routes::access::requests::raise_failed(error));
        }
        other => other,
    };
    let (ctx, guard) = crate::middleware::transaction_guard::after_wait(&ctx, &mut tx).await?;
    prefix.map_err(crate::routes::access::requests::raise_failed)?;
    if body.expires_at.is_some_and(|end| end <= guard.now()) {
        return Err(ApiError::invalid("expires_at", "must be in the future"));
    }
    let principal = guard.principal().clone();
    let prepared = ctx
        .app
        .engine
        .prepare_join_request(
            &mut tx,
            &principal,
            chat,
            body.reason.trim(),
            body.label.as_deref(),
            body.expires_at,
        )
        .await
        .map_err(crate::routes::access::requests::raise_failed);
    guard.check_deadline(&ctx)?;
    let prepared = prepared?;
    let raised = ctx
        .app
        .engine
        .raise_prepared_join_request(tx, prepared, &origin(&ctx))
        .await
        .map_err(crate::routes::access::requests::raise_failed)?;
    crate::routes::access::request_operation::created_reply(&ctx, principal.caller(), raised).await
}

async fn member_add(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatChangeResponse> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, guard) = group_mutation::begin(&ctx).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        origin: origin(&ctx),
        now: guard.now().get(),
    };
    let group_mutation::PreparedGroup { mut loaded, effect } = guard
        .operation_data::<group_mutation::PreparedGroup>()?
        .as_ref()
        .clone();
    let group_mutation::Effect::Add(prepared) = effect else {
        return Err(ApiError::internal("the group add operands changed kind"));
    };
    guard.check_deadline(&ctx)?;
    let added = chats::apply_add_member(&mut tx, &work.acting(), &mut loaded, prepared).await;
    operation::refuse(&work, &mut tx, id, added).await?;
    let full = group_mutation::response(&ctx, &mut tx, &work.principal).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(group_mutation::finish(&ctx, id, full))
}

/// Takes a person out of a group, or lets them leave.
async fn member_remove(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, guard) = group_mutation::begin(&ctx).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        origin: origin(&ctx),
        now: guard.now().get(),
    };
    let group_mutation::PreparedGroup { mut loaded, effect } = guard
        .operation_data::<group_mutation::PreparedGroup>()?
        .as_ref()
        .clone();
    let group_mutation::Effect::Remove(prepared) = effect else {
        return Err(ApiError::internal(
            "the group removal operands changed kind",
        ));
    };
    guard.check_deadline(&ctx)?;
    let removed = chats::apply_remove_member(&mut tx, &work.acting(), &mut loaded, prepared).await;
    operation::refuse(&work, &mut tx, id, removed).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ------------------------------------------------------------------ reading messages

/// Keep the native conversation and draft checks, including inspection's private-draft refusal.
async fn opening_get(ctx: Ctx, _query: ChatSyncGetQuery, (): ()) -> ApiResult<ChatOpening> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<read_operation::PreparedRead>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(&ctx),
    };
    note_opened(&mut tx, &work, &prepared.loaded).await?;
    let projected = read_operation::project(&mut tx, &work.principal, &prepared).await?;
    let opening = read_operation::opening_view(&prepared, projected)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(opening)
}

/// A cached path names candidates, never admits their chats. Each scope is checked here now.
async fn node_opening_get(
    ctx: Ctx,
    _query: NodeChatOpeningGetQuery,
    (): (),
) -> ApiResult<NodeChatOpening> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<node_opening::PreparedNodeOpening>()?;
    let shown = node_opening::show(&ctx, &mut tx, principal, &prepared).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// A chat and its latest messages, or what changed since a sequence.
async fn sync_get(ctx: Ctx, query: ChatSyncGetQuery, (): ()) -> ApiResult<ChatSync> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<read_operation::PreparedRead>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(&ctx),
    };
    note_opened(&mut tx, &work, &prepared.loaded).await?;
    let projected = read_operation::project(&mut tx, &work.principal, &prepared).await?;
    let chat = projected
        .chat
        .ok_or_else(|| ApiError::internal("the sync read has no chat projection"))?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(ChatSync {
        seq: chat.seq,
        chat,
        since: query.since,
        complete: prepared.complete,
        messages: projected.messages,
        has_older: prepared.has_older,
    })
}

/// A stretch of a chat's messages.
async fn message_list(
    ctx: Ctx,
    _query: ChatMessageListQuery,
    (): (),
) -> ApiResult<ChatMessageListReply> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<read_operation::PreparedMessageList>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(&ctx),
    };
    note_opened(&mut tx, &work, &prepared.read.loaded).await?;
    let projected = read_operation::project(&mut tx, &work.principal, &prepared.read).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(ChatMessageListReply {
        messages: projected.messages,
        has_older: prepared.read.has_older,
        has_newer: prepared.has_newer,
    })
}

// ------------------------------------------------------------------ writing messages

/// The redactor for what is written in a chat.
async fn redactor(conn: &mut Conn, work: &Work, loaded: &Loaded) -> ApiResult<redaction::Redactor> {
    let made = redaction::for_chat(conn, &loaded.row, &keys).await;
    work.refuse(conn, Some(loaded.id()), made).await
}

/// Hold the receipt separately so a commit that narrows disclosure never releases the full message.
struct MessageProjection {
    receipt: ChatMessageReceipt,
    full: Option<ChatMessageCreateReply>,
}

impl MessageProjection {
    async fn load(
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        row: &MessageRow,
        seq: orch_types::Seq,
    ) -> ApiResult<Self> {
        let full = if ctx.disclosure() == Disclosure::Denied {
            None
        } else {
            operation::read_message(ctx, conn, principal, row, seq).await?
        };
        Ok(Self::new(row, seq, full))
    }

    async fn changed(
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        row: &MessageRow,
        seq: orch_types::Seq,
    ) -> ApiResult<Self> {
        if ctx.disclosure() == Disclosure::Denied {
            return Ok(Self::new(row, seq, None));
        }
        let plan = mutation::response_plan(ctx, conn, principal.caller(), row).await?;
        Self::changed_with_plan(ctx, conn, principal, row, seq, plan).await
    }

    async fn edited(
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        row: &MessageRow,
        seq: orch_types::Seq,
    ) -> ApiResult<Self> {
        if ctx.disclosure() == Disclosure::Denied {
            return Ok(Self::new(row, seq, None));
        }
        let plan = message_edit::response_plan(ctx, conn, row).await?;
        Self::changed_with_plan(ctx, conn, principal, row, seq, plan).await
    }

    async fn changed_with_plan(
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        row: &MessageRow,
        seq: orch_types::Seq,
        plan: crate::registry::OperationPlan,
    ) -> ApiResult<Self> {
        let full = if ctx.disclosure() == Disclosure::Denied {
            None
        } else {
            operation::read_message_with_plan(ctx, conn, principal, row, seq, plan).await?
        };
        Ok(Self::new(row, seq, full))
    }

    fn new(row: &MessageRow, seq: orch_types::Seq, full: Option<ChatMessageCreateReply>) -> Self {
        Self {
            receipt: ChatMessageReceipt {
                projection: ReceiptProjection::Receipt,
                chat_id: row.chat_id,
                message_id: row.id,
                seq,
            },
            full,
        }
    }

    fn finish(self, ctx: &Ctx) -> ChatMessageCreateResponse {
        match (self.full, ctx.key_receipt_required()) {
            (Some(full), false) => ChatMessageCreateResponse::full(full),
            _ => ChatMessageCreateResponse::Receipt(self.receipt),
        }
    }

    fn finish_changed(self, ctx: &Ctx) -> ChatMessageMutationResponse {
        match (self.full, ctx.key_receipt_required()) {
            (Some(full), false) => ChatMessageMutationResponse::full(full.message),
            _ => ChatMessageMutationResponse::Receipt(self.receipt),
        }
    }
}

/// Sends a message.
async fn message_create(
    ctx: Ctx,
    (): (),
    body: ChatMessageCreateBody,
) -> ApiResult<Answer<ChatMessageCreateResponse>> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let effects = guard.operation_data::<operation::MessageEffects>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        origin: origin(&ctx),
        now: guard.now().get(),
    };
    let mut loaded = load(&mut tx, &work, id, Mode::Read).await?;
    work.refuse(&mut tx, Some(id), loaded.ensure_send()).await?;
    // A retry returns the stored acceptance before reviewing a changed model or binding.
    if let Some(previous) =
        rows::message_by_client(&mut tx, id, work.principal.user(), body.client_id.trim()).await?
    {
        let reply =
            MessageProjection::load(&ctx, &mut tx, &work.principal, &previous, loaded.row.seq())
                .await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(Answer::from(reply.finish(&ctx)).status(StatusCode::OK));
    }
    if effects.replayed.is_some() {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the accepted message changed during operation preparation",
        ));
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
            let reply = MessageProjection::load(
                &ctx,
                &mut tx,
                &work.principal,
                &previous,
                loaded.row.seq(),
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Answer::from(reply.finish(&ctx)).status(StatusCode::OK));
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
    let written = messages::post_with_blob_origins(
        &mut tx,
        &work.acting(),
        &mut loaded,
        Post {
            parts: body.parts,
            reply_to: body.reply_to,
            client_id: body.client_id,
        },
        &masking,
        &effects.blobs,
    )
    .await;
    let mut written = work.refuse(&mut tx, Some(id), written).await?;
    let queued = if written.created && !targets.is_empty() {
        let done = asking::forward_message(
            &ctx,
            &mut tx,
            &work,
            &written.message,
            &targets,
            Some(&effects.names),
        )
        .await?;
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
    let reply = MessageProjection::load(
        &ctx,
        &mut tx,
        &work.principal,
        &written.message,
        written.seq,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    if written.created {
        metrics::counter!("orch_chat_messages_total","kind"=>loaded.row.kind.clone()).increment(1);
    }
    if let Some(done) = &queued {
        ctx.app.engine.after_forward(done);
    }
    let answer = Answer::from(reply.finish(&ctx));
    Ok(if written.created {
        answer
    } else {
        answer.status(StatusCode::OK)
    })
}

/// Edits a message.
async fn message_update(
    ctx: Ctx,
    (): (),
    body: ChatMessageUpdateBody,
) -> ApiResult<ChatMessageMutationResponse> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let effects = initial.operation_data::<message_edit::Effects>()?;
    let selected = effects.selected.clone();
    let mut ctx = ctx.for_operation_checkpoint(initial.user().clone(), initial.now().get());
    if initial.principal().caller().admitted_operation().is_some() {
        ctx = ctx.with_admitted_caller(initial.principal().caller().clone())?;
    }
    let work = Work {
        projection_ctx: ctx.clone(),
        now: initial.now().get(),
        principal: initial.principal().clone(),
        origin: origin(&ctx),
    };
    let mut loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let masking = redactor(&mut tx, &work, &loaded).await?;
    let prepared = messages::prepare_edit(
        &mut tx,
        &work.acting(),
        &loaded,
        message,
        body.parts,
        &masking,
    )
    .await;
    let prepared = work.refuse(&mut tx, Some(id), prepared).await;
    initial.check_deadline(&ctx)?;
    let prepared = prepared?;
    let (ctx, work, row, seq) = if let Some(unchanged) = prepared.unchanged() {
        // The exact native no-op consumes no byte/quote source and takes no lock.
        (ctx, work, unchanged.clone(), loaded.row.seq())
    } else {
        let parts = prepared.parts().to_vec();
        // A metadata-only no-op is not the writer's first native observation.
        // If that observation is changed, admit its actual source set once before
        // retaining an association; do not synthesize a conflict or rebase its CAS.
        let (work, selected) = if effects.no_op {
            let first = message_edit::first_sources(&ctx, &mut tx, &work, &loaded, &parts).await;
            initial.check_deadline(&ctx)?;
            let (principal, selected) = first?;
            (
                Work {
                    projection_ctx: ctx.clone(),
                    now: principal.now().get(),
                    principal,
                    origin: origin(&ctx),
                },
                selected,
            )
        } else {
            (work, selected)
        };
        message_edit::retain_sources(&ctx, &selected)?;
        let locked = messages::lock_edit(
            &mut tx,
            &work.acting(),
            &mut loaded,
            prepared,
            Some(&selected),
        )
        .await;
        // Keep the native first observation and CAS result. Re-preparing this
        // credential's current metadata must never rebase the writer's revision.
        let (current, checked) = transaction_guard::after_wait(&ctx, &mut tx).await?;
        let work = Work {
            projection_ctx: current.clone(),
            now: checked.now().get(),
            principal: checked.principal().clone(),
            origin: origin(&current),
        };
        let locked = work.refuse(&mut tx, Some(id), locked).await;
        checked.check_deadline(&current)?;
        let locked = locked?;
        let mut loaded = load(&mut tx, &work, id, Mode::Read).await?;
        let principal =
            message_edit::current_sources(&current, &mut tx, &work, &loaded, &parts, &selected)
                .await;
        checked.check_deadline(&current)?;
        let principal = principal?;
        let work = Work {
            projection_ctx: current.clone(),
            now: principal.now().get(),
            principal,
            origin: origin(&current),
        };
        let prepared =
            messages::check_edit(&mut tx, &work.acting(), &loaded, locked, Some(&selected)).await;
        // All awaited native byte/attachment/quote checks precede the final
        // actual HTTP/operation boundary; its producer also checks current send/author.
        let (current, final_guard) = transaction_guard::after_wait(&current, &mut tx).await?;
        let work = Work {
            projection_ctx: current.clone(),
            now: final_guard.now().get(),
            principal: final_guard.principal().clone(),
            origin: origin(&current),
        };
        let prepared = work.refuse(&mut tx, Some(id), prepared).await;
        // No awaited preparation occurs between this raw HTTP/key lifetime
        // check and write_checked_edit's first thread/chat sequence mutation.
        final_guard.check_deadline(&current)?;
        let prepared = prepared?;
        let written =
            messages::write_checked_edit(&mut tx, &work.acting(), &mut loaded, prepared).await;
        let row = work.refuse(&mut tx, Some(id), written).await?;
        (current, work, row, loaded.row.seq())
    };
    let shown = MessageProjection::edited(&ctx, &mut tx, &work.principal, &row, seq).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown.finish_changed(&ctx))
}

/// Deletes a message.
async fn message_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessageMutationResponse> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Read).await?;
    let deleted = messages::delete(&mut tx, &work.acting(), &mut loaded, message).await;
    let row = work.refuse(&mut tx, Some(id), deleted).await?;
    let shown =
        MessageProjection::changed(&ctx, &mut tx, &work.principal, &row, loaded.row.seq()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown.finish_changed(&ctx))
}

/// The emoji of a path, as a client wrote it.
fn emoji_of(ctx: &Ctx) -> ApiResult<String> {
    Ok(ctx.param("emoji")?.to_owned())
}

async fn react(ctx: &Ctx, add: bool) -> ApiResult<ChatMessageMutationResponse> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let emoji = emoji_of(ctx)?;
    let mut tx = transaction(ctx).await?;
    let work = Work::begin(ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Write).await?;
    let changed = messages::react(&mut tx, &work.acting(), &mut loaded, message, &emoji, add).await;
    let row = work.refuse(&mut tx, Some(id), changed).await?;
    let shown =
        MessageProjection::changed(ctx, &mut tx, &work.principal, &row, loaded.row.seq()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown.finish_changed(ctx))
}

/// Adds the caller's reaction.
async fn reaction_add(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessageMutationResponse> {
    react(&ctx, true).await
}

/// Takes the caller's reaction away.
async fn reaction_remove(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessageMutationResponse> {
    react(&ctx, false).await
}

async fn pin(ctx: &Ctx, pinned: bool) -> ApiResult<ChatMessageMutationResponse> {
    let (id, message) = (ctx.id("id")?, ctx.id("message_id")?);
    let mut tx = transaction(ctx).await?;
    let work = Work::begin(ctx, &mut tx).await?;
    let mut loaded = load(&mut tx, &work, id, Mode::Write).await?;
    let changed = messages::pin(&mut tx, &work.acting(), &mut loaded, message, pinned).await;
    let row = work.refuse(&mut tx, Some(id), changed).await?;
    let shown =
        MessageProjection::changed(ctx, &mut tx, &work.principal, &row, loaded.row.seq()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown.finish_changed(ctx))
}

/// Pins a message.
async fn pin_set(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessageMutationResponse> {
    pin(&ctx, true).await
}

/// Takes the pin off a message.
async fn pin_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatMessageMutationResponse> {
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
async fn state_set(ctx: Ctx, (): (), body: ChatStateSetBody) -> ApiResult<ChatStateSetResponse> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        now: guard.now().get(),
        origin: origin(&ctx),
    };
    let loaded = chats::load(&mut tx, &work.acting(), id, Mode::Read).await;
    let loaded = operation::refuse(&work, &mut tx, id, loaded).await?;
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
    let state = operation::refuse(&work, &mut tx, id, changed).await?;
    let full = personal::state_response(&ctx, &mut tx, &work.principal, state).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(match (full, ctx.key_receipt_required()) {
        (Some(full), false) => ChatStateSetResponse::full(full),
        _ => ChatStateSetResponse::Receipt(ChatPersonalReceipt {
            projection: ReceiptProjection::Receipt,
            chat_id: id,
        }),
    })
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
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<personal::PreparedDraft>()?;
    let mut draft = prepared.draft.clone();
    if let Some(draft) = &mut draft {
        mentions::draft_names_bounded(&mut tx, &principal, draft, Some(&prepared.names)).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(draft)
}

/// Saves the caller's draft.
async fn draft_set(ctx: Ctx, (): (), body: ChatDraftSetBody) -> ApiResult<ChatDraftSetResponse> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let effects = guard.operation_data::<personal::DraftEffects>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        now: guard.now().get(),
        origin: origin(&ctx),
    };
    let loaded = chats::load(&mut tx, &work.acting(), id, Mode::Read).await;
    let loaded = operation::refuse(&work, &mut tx, id, loaded).await?;
    let saved = following::save_draft_with_blob_origins(
        &mut tx,
        &work.acting(),
        &loaded,
        &body.parts,
        body.reply_to,
        &effects.blobs,
    )
    .await;
    let draft = operation::refuse(&work, &mut tx, id, saved).await?;
    let full = personal::draft_response(&ctx, &mut tx, &work.principal, draft).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(match (full, ctx.key_receipt_required()) {
        (Some(full), false) => ChatDraftSetResponse::full(full),
        _ => ChatDraftSetResponse::Receipt(ChatPersonalReceipt {
            projection: ReceiptProjection::Receipt,
            chat_id: id,
        }),
    })
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
#[derive(Clone)]
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
) -> ApiResult<Answer<ForwardCreateResponse>> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let prepared = guard.operation_data::<forwarding::PreparedForward>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        origin: origin(&ctx),
        now: guard.now().get(),
    };
    let act = work.acting();
    let chats_to: Vec<_> = body
        .to
        .iter()
        .map(destination_of)
        .filter(|to| !matches!(to, Destination::Thread(_)))
        .collect();
    let thread_ids = prepared.threads.clone();
    let carried = &prepared.carried;
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
            let done = forward::deliver_checked(
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
                &prepared.chats,
                &prepared.blobs,
            )
            .await;
            sent.extend(work.refuse(&mut tx, None, done).await?);
        }
        if !elsewhere.is_empty() {
            let done = forward::deliver_checked(
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
                &prepared.chats,
                &prepared.blobs,
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
        let caller = work.principal.caller();
        match ctx
            .app
            .engine
            .forward_create_to_threads(
                &mut tx,
                caller,
                &prepared.source,
                &thread_ids,
                &parts,
                &body.client_id,
                true,
                &prepared.blobs,
            )
            .await
        {
            Ok(done) => queued = Some(done),
            Err((thread, error)) => {
                let failed = crate::routes::threads::input_failed_projected(
                    &mut tx,
                    thread.unwrap_or_else(Id::nil),
                    error,
                    ctx.key_receipt_required(),
                )
                .await;
                return Err(match thread {
                    Some(thread) => failed.with("object", Destination::Thread(thread).named()),
                    None => failed,
                });
            }
        }
    }
    let every_chat_before = sent.iter().all(|each| !each.written.created);
    let every_thread_before = queued
        .as_ref()
        .is_none_or(|done| done.inputs.iter().all(|input| !input.created));
    let receipt = ForwardCreateReceipt {
        projection: ReceiptProjection::Receipt,
        chats: sent
            .iter()
            .map(|each| ForwardChatReceipt {
                chat_id: each.chat,
                message_id: each.written.message.id,
                seq: each.written.seq,
            })
            .collect(),
        threads: queued
            .as_ref()
            .into_iter()
            .flat_map(|done| &done.inputs)
            .map(|input| ForwardInputReceipt {
                thread_id: input.thread_id,
                input_id: input.input_id,
                seq: input.seq,
            })
            .collect(),
    };
    let full =
        forwarding::read_response(&ctx, &mut tx, &work.principal, &sent, &receipt.threads).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Some(done) = &queued {
        ctx.app.engine.after_forward_projected(done);
    }
    let response = match (full, ctx.key_receipt_required()) {
        (Some(full), false) => ForwardCreateResponse::full(full),
        _ => ForwardCreateResponse::Receipt(receipt),
    };
    let answer = Answer::from(response);
    Ok(if every_chat_before && every_thread_before {
        answer.status(StatusCode::OK)
    } else {
        answer
    })
}
