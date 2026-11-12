//! Explicit person notices on readable documents, chats and places.

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::access::subject;

use crate::routes::nodes::invalid;

use orch_api_contract::access::AccessChoice;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::IDEMPOTENCY_HEADER;

use orch_api_contract::notifications::PERSON_NOTIFY_MAX_PEOPLE;

use orch_api_contract::notifications::PersonNoticeAnchor;

use orch_api_contract::notifications::PersonNotifyBody;

use orch_api_contract::notifications::PersonNotifyContext;

use orch_api_contract::notifications::PersonNotifyContextGet;

use orch_api_contract::notifications::PersonNotifyCreate;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::chats;

use orch_db::queries::documents;

use orch_db::queries::hierarchy;

use orch_db::queries::notifications::subscribers;

use orch_db::Conn;

use orch_db::DbError;

use orch_settings::Place;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use orch_worker_core::person_notices;

use serde_json::json;

pub use orch_api_base::routes::notifications::person::Target;

pub use orch_api_base::routes::notifications::person::tell;

pub use orch_api_base::routes::notifications::person::tell_before;

pub use orch_api_base::routes::notifications::person::CopySource;

pub use orch_api_base::routes::notifications::person::tell_native;
/// A rolling hour permits bursts while the setting limits sustained notifications.
const RATE_WINDOW_HOURS: i64 = 1;

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
orch_api_base::__api_bind_9bbe43b245!();

pub(super) async fn reads(
    conn: &mut Conn,
    principal: &Principal,
    target: &Target,
) -> ApiResult<bool> {
    person_notices::reads(conn, principal, target.source()?)
        .await
        .map_err(ApiError::internal)
}
orch_api_base::__api_bind_80eea56439!();

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
orch_api_base::__api_bind_7c8bf0625f!();

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
orch_api_base::__api_bind_012d881745!();

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
orch_api_base::__api_bind_013a538b95!();

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
orch_api_base::__api_bind_9c90bbf056!();

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
orch_api_base::__api_bind_9cdcfdd495!();

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
orch_api_base::__api_bind_d216f11fcf!();

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
orch_api_base::__api_bind_ef7d228569!();

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
orch_api_base::__api_bind_bc3f2eea2c!();

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
orch_api_base::__api_bind_981c9758e6!();

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
