//! Explicit person notices on readable documents, chats and places.

use std::collections::BTreeSet;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::middleware::transaction_guard;

pub use crate::registry::Ctx;

use crate::routes::nodes::actor_name;

pub use crate::routes::nodes::invalid;

use crate::sign_in::origin;

use orch_access_contract::audit::AuditAction;

pub use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

pub use orch_api_contract::notifications::PERSON_NOTIFY_MAX_PEOPLE;

pub use orch_api_contract::notifications::PersonNoticeAnchor;

pub use orch_api_contract::notifications::PersonNotifyCreate;

use orch_api_contract::routes::Endpoint;

pub use orch_authz::Principal;

pub use orch_db::queries::chats;

pub use orch_db::queries::documents;

pub use orch_db::queries::hierarchy;

pub use orch_db::Conn;

pub use orch_db::DbError;

pub use orch_types::Id;

use orch_types::new_id;

pub use orch_worker_core::person_notices;

use orch_worker_core::person_notices::Capture;

use orch_worker_core::person_notices::Subject;

pub use serde_json::json;
#[derive(Clone)]
pub enum Target {
    Document(Box<documents::DocumentRow>),
    Node(hierarchy::NodeRow),
    Chat(chats::ChatRow),
    Resource {
        row: orch_authz::resources::Resource,
        plural: String,
    },
}

impl Target {
    pub fn source(&self) -> ApiResult<Subject> {
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
    pub fn id(&self) -> Id {
        match self {
            Self::Document(row) => row.id,
            Self::Node(row) => row.id,
            Self::Chat(row) => row.id,
            Self::Resource { row, .. } => row.id,
        }
    }
    pub fn node(&self) -> Option<Id> {
        match self {
            Self::Document(row) => Some(row.node_id),
            Self::Node(row) => Some(row.id),
            Self::Chat(row) => Some(row.home()),
            Self::Resource { row, .. } => row.home,
        }
    }
    pub fn root(&self) -> Option<Id> {
        match self {
            Self::Document(row) => Some(row.org_id),
            Self::Node(row) => Some(row.org_id),
            Self::Chat(row) => Some(row.org_id),
            Self::Resource { row, .. } => row.root,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Self::Document(row) => &row.title,
            Self::Node(row) => &row.name,
            Self::Chat(row) => row.name.as_deref().unwrap_or("chat"),
            Self::Resource { row, .. } => &row.name,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Document(_) => "document",
            Self::Node(_) => "node",
            Self::Chat(_) => "chat",
            Self::Resource { row, .. } => row.kind.as_str(),
        }
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_9bbe43b245, __api_install_9bbe43b245, __api_callback_9bbe43b245;
    slot __ApiCallback9bbe43b245, __API_CALLBACK_9BBE43B245, __api_set_9bbe43b245;
    base orch_api_base::routes::notifications::person;

    async fn target<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api str, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::notifications::person::Target>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_80eea56439, __api_install_80eea56439, __api_callback_80eea56439;
    slot __ApiCallback80eea56439, __API_CALLBACK_80EEA56439, __api_set_80eea56439;
    base orch_api_base::routes::notifications::person;

    async fn reads<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::notifications::person::Target) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_7c8bf0625f, __api_install_7c8bf0625f, __api_callback_7c8bf0625f;
    slot __ApiCallback7c8bf0625f, __API_CALLBACK_7C8BF0625F, __api_set_7c8bf0625f;
    base orch_api_base::routes::notifications::person;

    async fn destination<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_api_base::routes::notifications::person::Target, arg2: Option<orch_api_contract::notifications::PersonNoticeAnchor>) -> orch_api_base::middleware::requests::ApiResult<(String, orch_types::Json)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_012d881745, __api_install_012d881745, __api_callback_012d881745;
    slot __ApiCallback012d881745, __API_CALLBACK_012D881745, __api_set_012d881745;
    base orch_api_base::routes::notifications::person;

    async fn lock_sender<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_013a538b95, __api_install_013a538b95, __api_callback_013a538b95;
    slot __ApiCallback013a538b95, __API_CALLBACK_013A538B95, __api_set_013a538b95;
    base orch_api_base::routes::notifications::person;

    fn page_change<'__api>(arg0: &'__api orch_api_contract::access::PageChange) -> orch_api_base::middleware::requests::ApiResult<(orch_access_contract::pages::PageChange, Vec<orch_types::Id>)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9c90bbf056, __api_install_9c90bbf056, __api_callback_9c90bbf056;
    slot __ApiCallback9c90bbf056, __API_CALLBACK_9C90BBF056, __api_set_9c90bbf056;
    base orch_api_base::routes::notifications::person;

    async fn replay_page<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_contract::access::PageChange) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9cdcfdd495, __api_install_9cdcfdd495, __api_callback_9cdcfdd495;
    slot __ApiCallback9cdcfdd495, __API_CALLBACK_9CDCFDD495, __api_set_9cdcfdd495;
    base orch_api_base::routes::notifications::person;

    async fn keep_page<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_contract::access::PageChange) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d216f11fcf, __api_install_d216f11fcf, __api_callback_d216f11fcf;
    slot __ApiCallbackd216f11fcf, __API_CALLBACK_D216F11FCF, __api_set_d216f11fcf;
    base orch_api_base::routes::notifications::person;

    async fn rate<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id, arg2: &'__api orch_api_base::routes::notifications::person::Target, arg3: usize, arg4: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ef7d228569, __api_install_ef7d228569, __api_callback_ef7d228569;
    slot __ApiCallbackef7d228569, __API_CALLBACK_EF7D228569, __api_set_ef7d228569;
    base orch_api_base::routes::notifications::person;

    async fn note_chars<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_api_base::routes::notifications::person::Target) -> orch_api_base::middleware::requests::ApiResult<i64>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_bc3f2eea2c, __api_install_bc3f2eea2c, __api_callback_bc3f2eea2c;
    slot __ApiCallbackbc3f2eea2c, __API_CALLBACK_BC3F2EEA2C, __api_set_bc3f2eea2c;
    base orch_api_base::routes::notifications::person;

    fn producer_error(arg0: orch_worker_core::person_notices::Error) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_981c9758e6, __api_install_981c9758e6, __api_callback_981c9758e6;
    slot __ApiCallback981c9758e6, __API_CALLBACK_981C9758E6, __api_set_981c9758e6;
    base orch_api_base::routes::notifications::person;

    async fn require_audience<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_api_base::routes::notifications::person::Target, arg2: &'__api [orch_types::Id], arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<()>;
}

/// Delivers an explicit act in the caller's transaction, including its audit record.
#[expect(
    clippy::too_many_arguments,
    reason = "the target, audience and text belong to one caller-owned transaction"
)]
pub async fn tell(
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
pub async fn tell_before<F>(
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

pub struct CopySource {
    pub created: Id,
    pub ending: Option<orch_api_contract::duplicates::DuplicateReceipt>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "one native notice, its owned first-effect and optional real copy source"
)]
pub async fn tell_native<F>(
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
