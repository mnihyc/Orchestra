//! Explicit person notices on readable documents, chats and places.

use std::collections::BTreeSet;

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::access::subject;
use crate::routes::nodes::{actor_name, invalid};
use crate::sign_in::origin;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access::AccessChoice;
use orch_api_contract::common::{ErrorCode, IDEMPOTENCY_HEADER};
use orch_api_contract::notifications::{
    PERSON_NOTIFY_MAX_PEOPLE, PersonNoticeAnchor, PersonNotifyBody, PersonNotifyContext,
    PersonNotifyContextGet, PersonNotifyCreate,
};
use orch_api_contract::routes::Endpoint;
use orch_authz::{Caller, Policy, Principal};
use orch_db::queries::{chats, documents, hierarchy, notifications::subscribers};
use orch_db::{Conn, DbError};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use orch_worker_core::person_notices::{self, Capture, Subject};
use serde_json::json;

/// A rolling hour permits bursts while the setting limits sustained notifications.
const RATE_WINDOW_HOURS: i64 = 1;

#[derive(Clone)]
pub(super) enum Target {
    Document(Box<documents::DocumentRow>),
    Node(hierarchy::NodeRow),
    Chat(chats::ChatRow),
    Resource {
        row: orch_authz::resources::Resource,
        plural: String,
    },
}

impl Target {
    pub(super) fn source(&self) -> ApiResult<Subject> {
        Ok(match self {
            Self::Document(row) => Subject::Object(ObjectRef {
                kind: ObjectKind::Document,
                id: row.id,
            }),
            Self::Node(row) => Subject::Object(ObjectRef {
                kind: row.kind.parse().map_err(ApiError::internal)?,
                id: row.id,
            }),
            Self::Chat(row) => Subject::Chat(row.id),
            Self::Resource { row, .. } => Subject::Object(row.object()),
        })
    }
    fn id(&self) -> Id {
        match self {
            Self::Document(row) => row.id,
            Self::Node(row) => row.id,
            Self::Chat(row) => row.id,
            Self::Resource { row, .. } => row.id,
        }
    }
    fn node(&self) -> Option<Id> {
        match self {
            Self::Document(row) => Some(row.node_id),
            Self::Node(row) => Some(row.id),
            Self::Chat(row) => Some(row.home()),
            Self::Resource { row, .. } => row.home,
        }
    }
    fn root(&self) -> Option<Id> {
        match self {
            Self::Document(row) => Some(row.org_id),
            Self::Node(row) => Some(row.org_id),
            Self::Chat(row) => Some(row.org_id),
            Self::Resource { row, .. } => row.root,
        }
    }
    fn name(&self) -> &str {
        match self {
            Self::Document(row) => &row.title,
            Self::Node(row) => &row.name,
            Self::Chat(row) => row.name.as_deref().unwrap_or("chat"),
            Self::Resource { row, .. } => &row.name,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Document(_) => "document",
            Self::Node(_) => "node",
            Self::Chat(_) => "chat",
            Self::Resource { row, .. } => row.kind.as_str(),
        }
    }
}

pub(super) fn mount(registry: &mut Registry) {
    registry
        .creation(
            PersonNotifyCreate,
            crate::middleware::idempotency::ObservedCreation::<(), PersonNotifyBody, ()> {
                kind: "person_notice",
                replay: |_ctx, (), _body, _created| Box::pin(async { Ok(()) }),
            },
        )
        .on_with_optional_operation(
            PersonNotifyCreate,
            crate::key_operations::native::stored_deferred::<super::subjects::Subjects, _, _>,
            create,
        )
        .on_snapshot_read_with_optional_operation(
            PersonNotifyContextGet,
            crate::key_operations::native::stored_query::<super::subjects::Subjects, _>,
            crate::key_operations::native::stored_read::<super::subjects::Subjects>,
        )
        .on_snapshot_read_with_optional_operation(
            orch_api_contract::notifications::DocumentNoticeJobList,
            crate::key_operations::native::selected::<super::document_jobs::NoticeJobs>,
            crate::key_operations::native::selected_read::<super::document_jobs::NoticeJobs>,
        );
}

pub(super) async fn target(conn: &mut Conn, object: &str, id: Id) -> ApiResult<Target> {
    match object {
        "documents" => documents::get(conn, id)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .map(|row| Target::Document(Box::new(row)))
            .ok_or_else(ApiError::not_found),
        "nodes" => hierarchy::with_ancestors(conn, &[id])
            .await?
            .into_iter()
            .find(|row| row.id == id && row.deleted_at.is_none())
            .map(Target::Node)
            .ok_or_else(ApiError::not_found),
        "chats" => chats::chat(conn, id)
            .await?
            .map(Target::Chat)
            .ok_or_else(ApiError::not_found),
        _ => {
            let kind = authorization::resource_kind_of(object).ok_or_else(ApiError::not_found)?;
            let reference = orch_access_contract::decisions::ObjectRef {
                kind: kind.into(),
                id,
            };
            let (_, row, _) = orch_authz::resources::site(conn, reference).await.map_err(
                |error| match error {
                    orch_authz::Error::NotFound(_) => ApiError::not_found(),
                    other => ApiError::internal(other),
                },
            )?;
            if row.deleted {
                return Err(ApiError::not_found());
            }
            Ok(Target::Resource {
                row,
                plural: object.to_owned(),
            })
        }
    }
}

pub(super) async fn reads(
    conn: &mut Conn,
    principal: &Principal,
    target: &Target,
) -> ApiResult<bool> {
    person_notices::reads(conn, principal, target.source()?)
        .await
        .map_err(ApiError::internal)
}

pub(super) async fn person_reads(
    conn: &mut Conn,
    target: &Target,
    user: Id,
    now: Time,
) -> ApiResult<bool> {
    match Principal::load(conn, Caller::session(user), now).await {
        Ok(person) => reads(conn, &person, target).await,
        Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => Ok(false),
        Err(error) => Err(ApiError::internal(error)),
    }
}

/// The target and anchor are checked before any notification or audit row is written.
pub(super) async fn destination(
    conn: &mut Conn,
    target: &Target,
    anchor: Option<PersonNoticeAnchor>,
) -> ApiResult<(String, Json)> {
    let id = target.id();
    match (target, anchor) {
        (Target::Document(_), None) => Ok((format!("/doc/{id}"), json!({"document_id":id}))),
        (Target::Document(_), Some(PersonNoticeAnchor::Block(block))) => {
            let found = documents::blocks_by_ids(conn, id, &[block]).await?;
            if !found
                .iter()
                .any(|row| row.id == block && row.removed_at.is_none())
            {
                return Err(invalid("anchor", "is not a live block of this document"));
            }
            Ok((
                format!("/doc/{id}?block={block}"),
                json!({"document_id":id,"block_id":block}),
            ))
        }
        (Target::Node(row), None) => {
            Ok((format!("/{}/{id}", row.kind), json!({"target_node_id":id})))
        }
        (Target::Node(row), Some(PersonNoticeAnchor::Turn(turn))) if row.kind == "thread" => {
            let found: Option<(i32, Id)> = sqlx::query_as(
                "SELECT ordinal, continuation_id FROM turns WHERE thread_id=$1 AND id=$2",
            )
            .bind(id)
            .bind(turn)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
            let (ordinal, continuation) =
                found.ok_or_else(|| invalid("anchor", "is not a turn of this thread"))?;
            Ok((
                format!("/thread/{id}?continuation={continuation}&turn={ordinal}"),
                json!({"target_node_id":id,"turn_id":turn}),
            ))
        }
        (Target::Chat(_), None) => Ok((format!("/chat/{id}"), json!({"chat_id":id}))),
        (Target::Resource { row, plural }, None) => {
            let section = if plural == "mcp-servers" || plural == "mcp_servers" {
                "mcp"
            } else {
                plural.as_str()
            };
            Ok((
                format!("/resources/{section}/{id}"),
                json!({"resource_kind":row.kind.as_str(),"resource_id":id}),
            ))
        }
        (Target::Chat(_), Some(PersonNoticeAnchor::Message(message))) => {
            let found = chats::message(conn, id, message)
                .await?
                .filter(|row| !row.is_deleted())
                .ok_or_else(|| invalid("anchor", "is not a live message of this chat"))?;
            Ok((
                format!("/chat/{id}?message={}", found.id),
                json!({"chat_id":id,"message_id":found.id}),
            ))
        }
        _ => Err(invalid("anchor", "does not name a part of this thing")),
    }
}

pub(super) async fn lock_sender(conn: &mut Conn, sender: Id) -> ApiResult<()> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('person.notify:' || $1::uuid::text, 0))",
    )
    .bind(sender)
    .execute(conn)
    .await
    .map_err(DbError::from)?;
    Ok(())
}

/// Removes transport-only notification fields before handing a change to the access engine.
pub(crate) fn page_change(
    body: &orch_api_contract::access::PageChange,
) -> ApiResult<(orch_access_contract::pages::PageChange, Vec<Id>)> {
    use orch_api_contract::access::HolderInput;
    let notifying = body.notify == Some(true);
    if !notifying && body.notify_anchor.is_some() {
        return Err(invalid(
            "notify_anchor",
            "needs notification to be selected",
        ));
    }
    let mut people = Vec::new();
    if notifying {
        if body.rows.is_some() || body.owners.is_some() || body.whole.is_some() {
            return Err(invalid("notify", "is available for explicit person grants"));
        }
        for name in body.name.as_deref().unwrap_or_default() {
            if !matches!(
                name.choice,
                AccessChoice::FullView | AccessChoice::FullInteract
            ) {
                return Err(invalid("notify", "is available for view or edit grants"));
            }
            for holder in &name.holders {
                match holder {
                    HolderInput::User { id } => {
                        if people.len() == PERSON_NOTIFY_MAX_PEOPLE {
                            return Err(invalid("notify", "names at most 200 people in one act"));
                        }
                        people.push(*id);
                    }
                    _ => return Err(invalid("notify", "is available for explicit person grants")),
                }
            }
        }
        if people.is_empty() {
            return Err(invalid("notify", "needs at least one person grant"));
        }
    }
    let mut plain = body.clone();
    plain.link_origin = None;
    plain.notify = None;
    plain.notify_anchor = None;
    let change = crate::routes::access::page::same(&plain)
        .ok_or_else(|| invalid("", "the change does not read as a change of a page"))?;
    Ok((change, people))
}

fn key(ctx: &Ctx) -> Option<&str> {
    ctx.headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn page_path(ctx: &Ctx) -> ApiResult<String> {
    Ok(format!(
        "/{}/{}/permissions",
        ctx.param("object")?,
        ctx.id("id")?
    ))
}

/// Serializes grants with direct notices and identifies a repeated grant.
pub(crate) async fn replay_page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    body: &orch_api_contract::access::PageChange,
) -> ApiResult<bool> {
    if body.notify != Some(true) {
        return Ok(false);
    }
    lock_sender(conn, principal.user()).await?;
    let Some(key) = key(ctx) else {
        return Ok(false);
    };
    let hours =
        i64::try_from(ctx.app.config.api.idempotency_ttl_hours).map_err(ApiError::internal)?;
    let Some(previous) = orch_db::idempotency::find(
        conn,
        principal.user(),
        key,
        principal.now().get() - chrono::Duration::hours(hours),
    )
    .await?
    else {
        return Ok(false);
    };
    let current = serde_json::to_value(body).map_err(ApiError::internal)?;
    if previous.method != "PATCH"
        || previous.path != page_path(ctx)?
        || previous.response.get("body") != Some(&current)
    {
        return Err(ApiError::new(
            ErrorCode::ConflictIdempotencyMismatch,
            "the key was used for another request",
        ));
    }
    Ok(true)
}

/// Saves the grant receipt in the same transaction as its notice.
pub(crate) async fn keep_page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    body: &orch_api_contract::access::PageChange,
) -> ApiResult<()> {
    if body.notify != Some(true) {
        return Ok(());
    }
    let Some(key) = key(ctx) else {
        return Ok(());
    };
    orch_db::idempotency::keep(
        conn,
        principal.user(),
        key,
        &orch_db::idempotency::Replay {
            method: "PATCH".to_owned(),
            path: page_path(ctx)?,
            status: 200,
            response: json!({"body":body}),
        },
        principal.now().get(),
    )
    .await?;
    Ok(())
}

async fn rate(
    conn: &mut Conn,
    sender: Id,
    target: &Target,
    count: usize,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<()> {
    let Some(max) = orch_settings::get::<i64>(
        conn,
        target.node().map_or(Place::System, Place::Node),
        keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR,
    )
    .await
    .map_err(ApiError::internal)?
    else {
        return Ok(());
    };
    if max == 0 {
        return Ok(());
    }
    let asked = i64::try_from(count).map_err(ApiError::internal)?;
    if asked > max {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "the audience exceeds the person notification rate; choose fewer people",
        )
        .with("limit", max)
        .with("asked", asked)
        .with("setting", keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR));
    }
    let since = now - chrono::Duration::hours(RATE_WINDOW_HOURS);
    let sent = orch_db::queries::notifications::operations::person_rate_used(
        conn,
        sender,
        target.root(),
        since,
    )
    .await?;
    if sent.saturating_add(asked) > max {
        let mut error = ApiError::new(
            ErrorCode::RateLimited,
            "the person notification rate is reached",
        )
        .with("limit", max)
        .with("asked", asked)
        .with("setting", keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR);
        if let Some(at) = orch_db::queries::notifications::operations::person_rate_release(
            conn,
            sender,
            target.root(),
            since,
            sent.saturating_add(asked).saturating_sub(max),
        )
        .await?
        {
            let millis = (at + chrono::Duration::hours(RATE_WINDOW_HOURS) - now).num_milliseconds();
            let seconds = u64::try_from(millis.saturating_add(999).div_euclid(1000).max(1))
                .map_err(ApiError::internal)?;
            error = error.retry_after(seconds);
        }
        return Err(error);
    }
    Ok(())
}

async fn note_chars(conn: &mut Conn, target: &Target) -> ApiResult<i64> {
    orch_settings::get::<i64>(
        conn,
        target.node().map_or(Place::System, Place::Node),
        keys::NOTIFICATIONS_PERSON_NOTE_CHARS,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("the notification note length did not resolve"))
}

fn producer_error(error: person_notices::Error) -> ApiError {
    match error {
        person_notices::Error::Refused
        | person_notices::Error::Raw(
            orch_authz::http_origin::Error::Refused | orch_authz::http_origin::Error::InvalidOrigin,
        ) => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original notice operation no longer permits this source",
        ),
        person_notices::Error::Db(error) => error.into(),
        person_notices::Error::Binding(error) => {
            crate::key_operations::admission::binding_error(error)
        }
        error => ApiError::internal(error),
    }
}

async fn require_audience(
    conn: &mut Conn,
    target: &Target,
    people: &[Id],
    now: Time,
) -> ApiResult<()> {
    orch_db::queries::transaction_guard::watch_standing(conn).await?;
    if subscribers::people_among(conn, people).await?.len() != people.len() {
        return Err(invalid(
            "user_ids",
            "must name people who can read this thing",
        ));
    }
    for person in people {
        if !person_reads(conn, target, *person, now).await? {
            return Err(invalid(
                "user_ids",
                "must name people who can read this thing",
            ));
        }
    }
    Ok(())
}

/// Delivers an explicit act in the caller's transaction, including its audit record.
#[expect(
    clippy::too_many_arguments,
    reason = "the target, audience and text belong to one caller-owned transaction"
)]
pub(crate) async fn tell(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: &str,
    id: Id,
    users: &[Id],
    anchor: Option<PersonNoticeAnchor>,
    note: Option<&str>,
) -> ApiResult<()> {
    tell_before(
        ctx,
        conn,
        principal,
        object,
        id,
        users,
        anchor,
        note,
        || Ok(()),
    )
    .await
}

/// The same native notice with a compound writer's once-only first-effect boundary.
#[expect(
    clippy::too_many_arguments,
    reason = "one native notice and its caller-owned first-effect boundary"
)]
pub(crate) async fn tell_before<F>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: &str,
    id: Id,
    users: &[Id],
    anchor: Option<PersonNoticeAnchor>,
    note: Option<&str>,
    before: F,
) -> ApiResult<()>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    tell_native(
        ctx, conn, principal, object, id, users, anchor, note, before, None,
    )
    .await
}
pub(crate) struct CopyAudience {
    pub users: Vec<Id>,
    pub people: u64,
}
/// Count the same native eligible readers/receivers, bounded in memory by the explicit-notice batch limit.
pub(crate) async fn copy_audience(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
) -> ApiResult<CopyAudience> {
    audience(ctx, conn, principal, chat, false).await
}
/// A native group ending tells current eligible members, including people who never authored.
pub(crate) async fn move_audience(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
) -> ApiResult<CopyAudience> {
    audience(ctx, conn, principal, chat, true).await
}
async fn audience(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
    members: bool,
) -> ApiResult<CopyAudience> {
    let target = target(conn, "chats", chat).await?;
    if !reads(conn, principal, &target).await? {
        return Err(ApiError::not_found());
    }
    let mut cursor = None;
    let mut users = Vec::new();
    let mut people = 0_u64;
    loop {
        let page: Vec<Id> = if members {
            sqlx::query_scalar("SELECT m.user_id FROM chat_members m JOIN users u ON u.id=m.user_id WHERE m.chat_id=$1 AND m.left_at IS NULL AND (m.expires_at IS NULL OR m.expires_at>$5) AND u.kind='human' AND u.status='active' AND m.user_id<>$2 AND ($3::uuid IS NULL OR m.user_id>$3) ORDER BY m.user_id LIMIT $4")
                .bind(chat).bind(principal.user()).bind(cursor).bind(i64::try_from(PERSON_NOTIFY_MAX_PEOPLE).map_err(ApiError::internal)?).bind(ctx.app.clock.now()).fetch_all(&mut *conn).await.map_err(DbError::from)?
        } else {
            sqlx::query_scalar("SELECT DISTINCT m.author_id FROM chat_messages m JOIN users u ON u.id=m.author_id WHERE m.chat_id=$1 AND m.kind='message' AND m.deleted_at IS NULL AND u.kind='human' AND u.status='active' AND m.author_id<>$2 AND ($3::uuid IS NULL OR m.author_id>$3) ORDER BY m.author_id LIMIT $4").bind(chat).bind(principal.user()).bind(cursor).bind(i64::try_from(PERSON_NOTIFY_MAX_PEOPLE).map_err(ApiError::internal)?).fetch_all(&mut *conn).await.map_err(DbError::from)?
        };

        let Some(last) = page.last() else {
            break;
        };
        cursor = Some(*last);
        let receivers =
            orch_db::queries::notifications::operations::receivers(conn, &page, "person.notify")
                .await?;
        for user in receivers {
            if person_reads(conn, &target, user, ctx.app.clock.time()).await? {
                people = people
                    .checked_add(1)
                    .ok_or_else(|| ApiError::internal("the contributor count overflowed"))?;
                if users.len() < PERSON_NOTIFY_MAX_PEOPLE {
                    users.push(user);
                }
            }
            crate::routes::duplicates::current(ctx, principal.caller())?;
        }
    }
    Ok(CopyAudience { users, people })
}
struct CopySource {
    created: Id,
    ending: Option<orch_api_contract::duplicates::DuplicateReceipt>,
}
/// The copy's real source and original HTTP identity remain on the same native explicit notice.
pub(crate) async fn tell_chat_copy(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    source: Id,
    created: Id,
    people: &[Id],
) -> ApiResult<()> {
    tell_native(
        ctx,
        conn,
        principal,
        "chats",
        source,
        people,
        None,
        None,
        || Ok(()),
        Some(CopySource {
            created,
            ending: None,
        }),
    )
    .await
}

/// The accepted destination receipt is captured on the same transaction before the
/// original group ends. No old-group foreign key remains on the recipient's Inbox row.
pub(crate) async fn tell_chat_move(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    source: Id,
    receipt: &orch_api_contract::duplicates::DuplicateReceipt,
    people: &[Id],
) -> ApiResult<()> {
    if receipt.kind != orch_api_contract::duplicates::DuplicateKind::Chat {
        return Err(ApiError::internal(
            "the native move notice needs its actual group receipt",
        ));
    }
    if people.is_empty() {
        return Ok(());
    }
    tell_native(
        ctx,
        conn,
        principal,
        "chats",
        source,
        people,
        None,
        None,
        || Ok(()),
        Some(CopySource {
            created: receipt.id,
            ending: Some(receipt.clone()),
        }),
    )
    .await
}
#[expect(
    clippy::too_many_arguments,
    reason = "one native notice, its owned first-effect and optional real copy source"
)]
async fn tell_native<F>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: &str,
    id: Id,
    users: &[Id],
    anchor: Option<PersonNoticeAnchor>,
    note: Option<&str>,
    before: F,
    copy: Option<CopySource>,
) -> ApiResult<()>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    let mut before = Some(before);
    if users.len() > PERSON_NOTIFY_MAX_PEOPLE {
        return Err(invalid("user_ids", "names at most 200 people in one act"));
    }
    let target = target(conn, object, id).await?;
    if !reads(conn, principal, &target).await? {
        return Err(ApiError::not_found());
    }
    let note_limit = note_chars(conn, &target).await?;
    if let Some(words) = note {
        if words.chars().any(|character| {
            matches!(
                character,
                '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
            )
        }) {
            return Err(invalid("note", "must be a single line"));
        }
        if i64::try_from(words.chars().count()).map_err(ApiError::internal)? > note_limit {
            return Err(invalid(
                "note",
                "is longer than the notification note setting allows",
            ));
        }
    }
    let people: Vec<Id> = users
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if people.is_empty() {
        return Err(invalid("user_ids", "must name at least one person"));
    }
    let sender = principal.user();
    lock_sender(conn, sender).await?;
    rate(conn, sender, &target, people.len(), principal.now().get()).await?;
    require_audience(conn, &target, &people, principal.now()).await?;
    // Lock each preference row in stable order so an opt-out cannot race the act.
    for person in &people {
        sqlx::query(
            "INSERT INTO notification_prefs(user_id) VALUES($1) ON CONFLICT(user_id) DO NOTHING",
        )
        .bind(person)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
        orch_db::queries::notifications::people::lock_prefs_shared(conn, *person).await?;
    }
    if orch_db::queries::notifications::operations::receivers(conn, &people, "person.notify")
        .await?
        .len()
        != people.len()
    {
        return Err(invalid(
            "user_ids",
            "must name people who can read this thing",
        ));
    }
    let act = new_id();
    let key = format!("person.notify:{act}");
    // The missing preference rows above are a rollbackable native lock prefix. Refresh
    // the exact stored subject after every such wait, before any Inbox/fanout effect.
    // Other compound producers keep their own staged source handoff.
    let operation = match ctx.route.name {
        name if name == PersonNotifyCreate::ROUTE.name => Some(person_notices::Operation::Person),
        name if name == orch_api_contract::notifications::ChatPersonGrant::ROUTE.name => {
            Some(person_notices::Operation::ChatGrant)
        }
        _ => None,
    };
    let late = if operation.is_some() || copy.is_some() {
        orch_db::queries::notifications::people::lock_announce_receivers(
            conn,
            &people,
            "person.notify",
            &key,
        )
        .await?;
        Some(transaction_guard::after_wait(ctx, conn).await?)
    } else {
        None
    };
    if late.is_some() {
        let fresh = self::target(conn, object, id).await?;
        if fresh.source()? != target.source()?
            || fresh.node() != target.node()
            || fresh.root() != target.root()
        {
            // No Inbox or external effect has occurred. Reprepare the one rollbackable
            // act with its actual new home and that home's audience/rate policy.
            return Err(DbError::Retry.into());
        }
    }
    if let Some((_, guard)) = &late {
        require_audience(conn, &target, &people, guard.now()).await?;
        if orch_db::queries::notifications::operations::receivers(conn, &people, "person.notify")
            .await?
            .len()
            != people.len()
        {
            return Err(invalid(
                "user_ids",
                "must name people who can read this thing",
            ));
        }
    }
    let (path, mut data) = destination(conn, &target, anchor).await?;
    data["path"] = json!(path);
    data["by"] = json!(principal.user());
    data["subject"] = json!(target.name());
    let ending = copy.as_ref().and_then(|copy| copy.ending.as_ref());
    if let Some(receipt) = ending {
        data.as_object_mut()
            .ok_or_else(|| ApiError::internal("the native notice data is missing"))?
            .remove("chat_id");
        data["path"] = json!(format!("/chat/{}", receipt.id));
        data["completed_chat_move"] = json!(
            orch_db::queries::notifications::operations::ending::ChatMove {
                source: id,
                created: receipt.id
            }
        );
    }
    let mut capture = if let Some((current, guard)) = &late {
        let capture = match &copy {
            None => Capture::prepare(
                conn,
                current.app.clock.as_ref(),
                guard.principal().caller(),
                target.source()?,
                operation
                    .ok_or_else(|| ApiError::internal("the native notice operation is missing"))?,
                act,
            )
            .await
            .map_err(producer_error)?,
            Some(copy) => {
                let origin = orch_authz::CapturedOperation::capture(guard.principal().caller())
                    .map_err(ApiError::internal)?;
                let raw = crate::key_operations::native::http_origin::capture_operation(
                    current, conn, &origin,
                )
                .await?;
                if let Some(receipt) = &copy.ending {
                    Capture::prepare_chat_move(
                        conn,
                        current.app.clock.as_ref(),
                        guard.principal().caller(),
                        raw.shared().clone(),
                        id,
                        receipt,
                        act,
                    )
                    .await
                    .map_err(producer_error)?
                } else {
                    Capture::prepare_chat_copy(
                        conn,
                        current.app.clock.as_ref(),
                        guard.principal().caller(),
                        raw.shared().clone(),
                        id,
                        copy.created,
                        act,
                    )
                    .await
                    .map_err(producer_error)?
                }
            }
        };
        data["operation_id"] = json!(act);
        Some(capture)
    } else {
        None
    };
    let title = format!("{} notified you about {}", actor_name(ctx)?, target.name());
    for person in &people {
        let notice = orch_db::queries::notifications::people::NewNotification {
            user_id: *person,
            kind: "person.notify",
            node_id: if ending.is_some() {
                None
            } else {
                target.node()
            },
            thread_id: match &target {
                Target::Node(row) if row.kind == "thread" => Some(row.id),
                _ => None,
            },
            chat_id: match &target {
                Target::Chat(row) if ending.is_none() => Some(row.id),
                _ => None,
            },
            approval_id: None,
            title: &title,
            body: note.unwrap_or_default(),
            data: &data,
            dedupe_key: &key,
            at: principal.now().get(),
        };
        let mut boundary_error = None;
        let result = orch_engine::notifications::tell_person_notice_before(
            conn,
            target.root(),
            &notice,
            Some(|| {
                if let Some((current, guard)) = &late
                    && let Err(error) = guard.check_deadline(current)
                {
                    boundary_error = Some(error);
                    return Err(orch_engine::EngineError::unreadable(
                        "the notice source ended before its first effect",
                    ));
                }
                if let Some(check) = before.take()
                    && let Err(error) = check()
                {
                    boundary_error = Some(error);
                    return Err(orch_engine::EngineError::unreadable(
                        "the compound notice ended before its first effect",
                    ));
                }
                Ok(())
            }),
        )
        .await;
        if let Some(error) = boundary_error {
            return Err(error);
        }
        if let Some(actual) = result.map_err(ApiError::internal)?
            && let Some(capture) = &mut capture
        {
            capture.record(&notice, &actual).map_err(producer_error)?;
        }
    }
    if let Some(capture) = capture {
        capture.save(conn).await.map_err(producer_error)?;
    }
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(sender),
        orch_db::audit::action!(AuditAction::PersonNotified).as_str(),
        target.kind(),
        Some(id),
        "Notified people",
    );
    entry.node_id = target.node();
    entry.org_id = target.root();
    entry.details = json!({"count":people.len(),"people":people,"anchor":anchor});
    let audit_id =
        orch_db::audit::append(conn, &entry, &origin(ctx), principal.now().get()).await?;
    orch_db::idempotency::creation::observe(conn, Some(sender), "person_notice", audit_id).await?;
    Ok(())
}

async fn create(mut ctx: Ctx, (): (), body: PersonNotifyBody) -> ApiResult<()> {
    let object = ctx.param("object")?.to_owned();
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    lock_sender(&mut tx, principal.user()).await?;
    let (current, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    ctx = current;
    let principal = guard.principal();
    tell(
        &ctx,
        &mut tx,
        principal,
        &object,
        id,
        &body.user_ids,
        body.anchor,
        body.note.as_deref(),
    )
    .await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

pub(super) async fn context_for(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    target: &Target,
) -> ApiResult<PersonNotifyContext> {
    let user = ctx.id("user_id")?;
    let can_view = person_reads(conn, target, user, principal.now()).await?;
    let human = orch_db::queries::identity::user_by_id(conn, user)
        .await?
        .is_some_and(|row| row.kind == "human" && row.status == "active");
    let mut gives = Vec::new();
    let mut visible = false;
    if !matches!(target, Target::Chat(_)) {
        let named = subject::named(ctx)?;
        let may_inspect =
            match subject::require_page(conn, principal, named.thing, Policy::default()).await {
                Ok(()) => true,
                Err(error)
                    if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) =>
                {
                    false
                }
                Err(error) => return Err(error),
            };
        if may_inspect {
            visible = true;
            let (_, site) = orch_authz::things::site(conn, named.thing)
                .await
                .map_err(ApiError::internal)?;
            let policy = subject::policy_of(conn, named.thing, &site).await?;
            let page = orch_authz::pages::read(conn, principal, named.thing.target(), policy)
                .await
                .map_err(ApiError::internal)?;
            let local = match target.root() {
                Some(root) => !orch_db::queries::access::local_holders_among(conn, root, &[user])
                    .await?
                    .is_empty(),
                None => true,
            };
            if !can_view && human && (page.external || local) {
                for choice in &page.gives.choices {
                    if let Some(choice) =
                        crate::routes::access::page::same::<_, AccessChoice>(choice)
                        && (choice == AccessChoice::FullView
                            || choice == AccessChoice::FullInteract
                                && !matches!(target, Target::Resource { .. }))
                    {
                        gives.push(choice);
                    }
                }
            }
        }
    } else if let Target::Chat(row) = target {
        (visible, gives) =
            super::chat_grants::choices(ctx, conn, principal, row, user, can_view, human).await?;
    }
    let note_chars = note_chars(conn, target).await?;
    Ok(PersonNotifyContext {
        can_view: visible.then_some(can_view),
        // A person's own card never offers to notify themselves.
        can_notify: human
            && can_view
            && principal.user() != user
            && !orch_db::queries::notifications::operations::receivers(
                conn,
                &[user],
                "person.notify",
            )
            .await?
            .is_empty(),
        gives,
        note_chars,
    })
}
