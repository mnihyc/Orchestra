//! Asking for access to things, reading where the request stands, and deciding it: one approval for each thing asked for, each sent to the nearest people who can give it.
//!
//! The rules are the access engine's and the transaction is the engine's: what is written here is
//! the translation. A request is refused whole when one of its things cannot be asked for, and
//! says which and why; it is made by anybody who is signed in, for a thing whose identifier they
//! know. Where an approval stands, and whether the caller may decide it, is told by [`views_of`],
//! which the list and the reading of approvals use for the approvals of this kind.

use super::subject;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::routes::approvals;
use crate::routes::nodes::{invalid, view};
use crate::sign_in::origin;
use orch_access_contract::decisions::{Closed, ObjectRef};
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access as api;
use orch_api_contract::approvals::{
    AccessDecision, AccessRequest, Approval, ApprovalDecision, ApprovalState, CapabilityDecision,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeRef;
use orch_api_contract::users::UserRef;
use orch_authz::requests::{Ask, AskError, AskedOptions, REASON_MAX_CHARS, Whom};
use orch_authz::{Policy, Principal, requests};
use orch_db::Conn;
use orch_db::queries::approvals::{self as rows, ApprovalRow};
use orch_engine::approvals::access::{DecideError, Decided, RaiseError, read_requests};
use orch_types::{Id, Time};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// The path of a member of an item of a request.
fn at(index: usize, member: &str) -> String {
    format!("items[{index}].{member}")
}

/// An item of a request, read into what the access engine asks.
fn ask_of(index: usize, item: &api::AccessRequestItem, caller: Id) -> ApiResult<Ask> {
    let kind = subject::kind_of_word(&item.object.kind).ok_or_else(|| {
        invalid(
            &at(index, "object.kind"),
            "is no kind of thing that has a permission page",
        )
    })?;
    let mut permissions: Vec<Permission> = Vec::with_capacity(item.permissions.len());
    for (place, name) in item.permissions.iter().enumerate() {
        let permission = name.parse().map_err(|_| {
            invalid(
                &format!("items[{index}].permissions[{place}]"),
                &format!("{name} is no permission"),
            )
        })?;
        if !permissions.contains(&permission) {
            permissions.push(permission);
        }
    }
    let whom = match &item.holder {
        None => Whom::Requester,
        Some(api::HolderInput::User { id }) if *id == caller => Whom::Requester,
        Some(api::HolderInput::User { id }) => Whom::Person { id: *id },
        Some(api::HolderInput::Node { id }) => Whom::Work { id: *id },
        Some(_) => {
            return Err(invalid(
                &at(index, "holder"),
                "is a selected person, or the work in a place",
            ));
        }
    };
    Ok(Ask {
        label: item.label.clone(),
        object: ObjectRef {
            kind,
            id: item.object.id,
        },
        permissions,
        options: item.options.as_ref().map(|options| AskedOptions {
            models: options.models.clone(),
            efforts: options.efforts.clone(),
        }),
        whom,
    })
}

/// What is wrong with an item that cannot be asked for, as the answer a client gets.
fn item_refused(index: usize, error: AskError) -> ApiError {
    match error {
        AskError::Engine(orch_authz::Error::NoSuchUser(_)) => {
            invalid(&at(index, "holder"), "is no active person")
        }
        AskError::Engine(error) => ApiError::internal(error),
        AskError::NotFound => ApiError::new(
            ErrorCode::NotFound,
            format!("item {index} names nothing that is there"),
        ),
        AskError::Invalid { path, message } => invalid(&at(index, path), &message),
        AskError::Closed { closed, place } => {
            let word = match closed {
                Closed::SubscriptionNotShared => "subscription_not_shared",
                Closed::Locked => "locked",
            };
            invalid(
                &at(index, "permissions"),
                "is closed: nobody can give it, and it is not asked for",
            )
            .with("closed", word)
            .with(
                "by",
                place.map(|place| match place {
                    PlaceRef::System => json!("system"),
                    PlaceRef::Node(node) => json!(node),
                }),
            )
        }
        AskError::Held => invalid(
            &at(index, "permissions"),
            "are held already: there is nothing to ask for",
        ),
        AskError::Nobody => invalid(
            &at(index, "object"),
            "has nobody who could give it, apart from the person who asks",
        ),
    }
}

/// Asks for access to up to eight things in one batch. The request is made in the transaction that asked
/// the caller's credential and authority again once the body had come, with the caller and the time of
/// that check.
pub async fn create(
    mut ctx: Ctx,
    (): (),
    body: api::AccessRequestCreateBody,
) -> ApiResult<api::AccessRequestCreateReply> {
    let asker = ctx.user()?.id();
    let most = usize::try_from(api::ACCESS_REQUEST_MAX_ITEMS).unwrap_or(usize::MAX);
    if body.items.is_empty() || body.items.len() > most {
        return Err(invalid(
            "items",
            &format!("holds from one to {most} things"),
        ));
    }
    if body.reason.chars().count() > REASON_MAX_CHARS {
        return Err(invalid(
            "reason",
            &format!("must be at most {REASON_MAX_CHARS} characters"),
        ));
    }
    let asks: Vec<Ask> = body
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| ask_of(index, item, asker))
        .collect::<ApiResult<_>>()?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let raised = ctx
        .app
        .engine
        .raise_access_requests_guarded(
            tx,
            principal.clone(),
            &asks,
            body.reason.trim(),
            body.for_thread,
            &origin(&ctx),
        )
        .await
        .map_err(raise_failed)?;
    let mut conn = ctx.app.db.acquire().await?;
    let approvals = approvals::views(
        &mut conn,
        &principal,
        &raised.approvals,
        ctx.now,
        &ctx.app.engine,
    )
    .await?;
    Ok(api::AccessRequestCreateReply {
        batch_id: raised.batch_id,
        approvals,
    })
}

/// The same refusal shape for permission requests and membership requests.
pub(crate) fn raise_failed(error: RaiseError) -> ApiError {
    match error {
            RaiseError::Internal(error) => ApiError::engine(error),
            RaiseError::Item { index, error } => item_refused(index, error),
            RaiseError::Waiting { limit, waiting } => ApiError::new(
                ErrorCode::AccessTooManyRequests,
                format!(
                    "{waiting} requests are waiting for an answer, and {limit} is the most; wait for one, or take one back"
                ),
            )
            .with("limit", limit)
            .with("waiting", waiting),
            RaiseError::NoSuchThread => invalid("for_thread", "is no thread"),
            RaiseError::AlreadyWaiting { approval } => ApiError::new(ErrorCode::ConflictExists, "A request for this thing is already waiting").with("approval_id", approval),
            }
}

/// Where a batch of requests stands, for the person who made it.
pub async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<api::AccessRequestGetReply> {
    let batch = ctx.id("batch_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let found = rows::access::batch(&mut conn, batch).await?;
    if found.is_empty()
        || found
            .iter()
            .any(|row| row.requester_user_id != Some(principal.user()))
    {
        return Err(ApiError::not_found());
    }
    let complete = ctx
        .app
        .engine
        .access_batch_is_held(principal.user(), batch)
        .await
        .map_err(ApiError::internal)?;
    let approvals =
        approvals::views(&mut conn, &principal, &found, ctx.now, &ctx.app.engine).await?;
    Ok(api::AccessRequestGetReply {
        batch_id: batch,
        approvals,
        complete,
    })
}

/// How an approval of the kind `access` is shown to the one who reads it.
pub struct Shown {
    /// What it asks, with the names the reader may know.
    pub request: AccessRequest,
    /// The thing it is about, as the reader may know it.
    pub object: orch_api_contract::access::ObjectRef,
    /// Where it stands: a request that ran out reads as expired.
    pub state: ApprovalState,
    /// Whether the reader may decide it now.
    pub can_decide: bool,
    /// Whether it was sent to the reader: they are among the nearest who could give it.
    pub sent_to_them: bool,
}

/// How the approvals of the kind `access` among `found` are shown to `principal`.
pub async fn views_of(
    conn: &mut Conn,
    principal: &Principal,
    engine: &orch_engine::Engine,
    found: &[ApprovalRow],
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<HashMap<Id, Shown>> {
    let mine: Vec<&ApprovalRow> = found.iter().filter(|row| row.kind == "access").collect();
    if mine.is_empty() {
        return Ok(HashMap::new());
    }
    let read = read_requests(conn, &mine)
        .await
        .map_err(ApiError::internal)?;
    let mut users: Vec<Id> = Vec::new();
    let mut places: Vec<Id> = Vec::new();
    for row in &mine {
        users.extend(row.requester_user_id);
        places.extend(row.thread_id);
        if let Some((_, asked, _)) = read.get(&row.id) {
            match asked.whom {
                Whom::Person { id } => users.push(id),
                Whom::Work { id } => places.push(id),
                Whom::Requester => {}
            }
        }
    }
    let people = refusals::user_refs(conn, &users).await?;
    let refs = view::refs(conn, principal, &places, Policy::default()).await?;
    let mut shown = HashMap::new();
    for row in mine {
        if orch_engine::membership_requests::is_join(row) {
            let chat = row.object_id.ok_or_else(ApiError::not_found)?;
            let requester = row.requester_user_id.ok_or_else(ApiError::not_found)?;
            let pending = row.state == "pending" && row.expires_at.is_none_or(|end| end > now);
            let can_decide = pending
                && orch_engine::membership_requests::may_add(conn, principal, chat, requester)
                    .await
                    .map_err(ApiError::internal)?;
            let act = orch_chat::Acting {
                principal,
                now,
                origin: &orch_db::audit::Origin::default(),
            };
            let name = match orch_chat::chats::load(conn, &act, chat, orch_chat::chats::Mode::Read)
                .await
            {
                Ok(loaded) => {
                    crate::routes::chats::people_label(
                        conn,
                        principal,
                        &loaded.row,
                        &loaded.members,
                    )
                    .await?
                }
                Err(orch_chat::Error::Db(error)) => return Err(ApiError::internal(error)),
                Err(_) => String::new(),
            };
            let request = AccessRequest {
                membership: true,
                label: row
                    .request
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                permissions: Vec::new(),
                options: None,
                holder: api::Holder::User {
                    user: people
                        .get(&requester)
                        .cloned()
                        .ok_or_else(ApiError::not_found)?,
                },
                reason: row
                    .request
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                for_thread: None,
            };
            shown.insert(
                row.id,
                Shown {
                    request,
                    object: api::ObjectRef {
                        kind: "chat".to_owned(),
                        id: chat,
                        name,
                    },
                    state: if row.state == "pending" && !pending {
                        ApprovalState::Expired
                    } else {
                        state_of(&row.state)
                    },
                    can_decide,
                    sent_to_them: false,
                },
            );
            continue;
        }

        let Some((object, asked, requester)) = read.get(&row.id) else {
            continue;
        };
        let (object, requester) = (*object, *requester);
        let pending = row.state == "pending";
        let overdue = pending && row.expires_at.is_some_and(|end| end <= now);
        let standing = if pending && !overdue {
            engine
                .standing_on_access_request(principal.caller(), row)
                .await
                .map_err(ApiError::internal)?
        } else {
            requests::Standing::default()
        };
        let name = requests::named_for(conn, principal, object, Policy::default())
            .await
            .map_err(ApiError::internal)?;
        let by: UserRef = people
            .get(&requester)
            .cloned()
            .ok_or_else(|| ApiError::internal("the person who asked is not a user"))?;
        let holder = match asked.whom {
            Whom::Requester => api::Holder::User { user: by },
            Whom::Person { id } => api::Holder::User {
                user: people
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the selected person is not a user"))?,
            },
            Whom::Work { id } => api::Holder::Node {
                node: refs.get(&id).cloned().unwrap_or(NodeRef {
                    thread_display: None,
                    id,
                    kind: orch_api_contract::nodes::NodeKind::Org,
                    name: None,
                    redacted: true,
                }),
            },
        };
        shown.insert(
            row.id,
            Shown {
                request: AccessRequest {
                    membership: false,
                    label: asked.label.clone(),
                    permissions: asked
                        .permissions
                        .iter()
                        .map(|permission| permission.as_str().to_owned())
                        .collect(),
                    options: asked.options.as_ref().map(|options| api::AccessOptions {
                        models: options.models.clone(),
                        efforts: options.efforts.clone(),
                    }),
                    holder,
                    reason: asked.reason.clone(),
                    for_thread: row.thread_id.and_then(|thread| refs.get(&thread).cloned()),
                },
                object: api::ObjectRef {
                    kind: object.kind.as_str().to_owned(),
                    id: object.id,
                    name: name.unwrap_or_default(),
                },
                state: if overdue {
                    ApprovalState::Expired
                } else {
                    state_of(&row.state)
                },
                can_decide: standing.may_decide && standing.can_give,
                sent_to_them: standing.sent_to_them,
            },
        );
    }
    Ok(shown)
}

fn state_of(word: &str) -> ApprovalState {
    match word {
        "approved" => ApprovalState::Approved,
        "denied" => ApprovalState::Denied,
        "cancelled" => ApprovalState::Cancelled,
        "expired" => ApprovalState::Expired,
        _ => ApprovalState::Pending,
    }
}

/// Which of the pending approvals of the kind `access` among `found` were sent to `principal`.
pub fn sent_to(shown: &HashMap<Id, Shown>) -> HashSet<Id> {
    shown
        .iter()
        .filter(|(_, each)| each.sent_to_them)
        .map(|(id, _)| *id)
        .collect()
}

/// Decides a request for access: approving names the person who asked in the rows they asked
/// for, as the approver's own change of the page; denying gives nothing. `tx` and `approver` are
/// the transaction and the principal of the action guard of the request that decides.
pub async fn decide(
    ctx: &Ctx,
    mut tx: orch_db::Tx,
    approver: Principal,
    id: Id,
    decision: &ApprovalDecision,
) -> ApiResult<Approval> {
    // A body that holds `approve` and at most a note fits a decision on a capability as well, and is
    // read as that one when the body is read, whichever approval it is for.
    let alone;
    let decision = match decision {
        ApprovalDecision::Capability(CapabilityDecision { approve, note }) => {
            alone = AccessDecision {
                approve: *approve,
                permissions: None,
                options: None,
                expires_at: None,
                quick_duration: None,
                note: note.clone(),
            };
            &alone
        }
        ApprovalDecision::Access(access) => access,
        _ => {
            return Err(invalid(
                "",
                "a request for access is answered with approve, and if you like fewer permissions, narrower options, an end and a note",
            ));
        }
    };
    let AccessDecision {
        approve,
        permissions,
        options,
        expires_at,
        quick_duration,
        note,
    } = decision;
    let expires_at = if let Some(duration) = quick_duration {
        if !approve || expires_at.is_some() {
            return Err(invalid(
                "quick_duration",
                "is used for a grant instead of an explicit end date",
            ));
        }
        let row = rows::approval(&mut tx, id)
            .await?
            .filter(|row| row.kind == "access")
            .ok_or_else(ApiError::not_found)?;
        let place = row
            .node_id
            .map_or(orch_settings::Place::System, orch_settings::Place::Node);
        let choices: Vec<String> = orch_settings::get(
            &mut tx,
            place,
            orch_settings_contract::registry::keys::ACCESS_QUICK_GRANT_DURATIONS,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
        if !choices.contains(duration) {
            return Err(invalid(
                "quick_duration",
                "is not one of this place's current choices",
            ));
        }
        if duration == "always" {
            None
        } else {
            let days = duration
                .strip_suffix('d')
                .and_then(|days| days.parse::<i64>().ok())
                .filter(|days| *days > 0)
                .and_then(chrono::Duration::try_days)
                .ok_or_else(|| invalid("quick_duration", "does not fit a duration"))?;
            Some(Time::new(
                approver
                    .now()
                    .get()
                    .checked_add_signed(days)
                    .ok_or_else(|| invalid("quick_duration", "does not fit a date"))?,
            ))
        }
    } else {
        *expires_at
    };
    if note
        .as_ref()
        .is_some_and(|note| note.chars().count() > REASON_MAX_CHARS)
    {
        return Err(invalid(
            "note",
            &format!("must be at most {REASON_MAX_CHARS} characters"),
        ));
    }
    let decided = Decided {
        approve: *approve,
        permissions: permissions.clone(),
        options: options.clone(),
        expires_at,
        note: note.clone(),
    };
    match ctx
        .app
        .engine
        .decide_access_request_guarded(tx, approver, id, &decided, &origin(ctx))
        .await
    {
        Ok(_) => {
            let mut conn = ctx.app.db.acquire().await?;
            approvals::one_approval(ctx, &mut conn, id).await
        }
        Err(DecideError::NotFound) => Err(ApiError::not_found()),
        Err(DecideError::AlreadyDecided) => Err(approvals::already_decided(ctx, id).await),
        Err(DecideError::Denied(refusal)) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(refusals::denied(&mut conn, *refusal).await?)
        }
        Err(DecideError::Invalid { path, message }) => Err(invalid(path, &message)),
        Err(DecideError::Change(error)) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(refusals::change_failed(&mut conn, *error).await)
        }
        Err(DecideError::OwnRequest) => Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "a request is decided by somebody else than the person who made it",
        )
        .with("required", "access.manage")
        .with("object", json!({ "kind": "approval", "id": id }))
        .with("requestable", false)
        .with("override", false)
        .with("missing", Vec::<api::MissingAccess>::new())),
        Err(DecideError::Internal(error)) => Err(ApiError::internal(error)),
    }
}
