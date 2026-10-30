//! What the access engine says, as the problems a client acts on: a refusal with everything that is missing and who can give it, a suspension in an org, and a place that is archived.
//!
//! The engine answers in the ids and words of its own contract. A client
//! wants people it can show (a name and a picture, not an id), the kind of
//! thing as a word, and the node that is to blame, so the answers are turned
//! here and nowhere else: a handler that is refused by the engine passes the
//! refusal to [`denied`] and returns what comes back.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::sign_in::people::avatar_text;
use orch_access_contract::decisions::{self, Closed, Refusal};
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access::{AccessOptions, ClosedReason, MissingAccess, MissingAccessObject};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::common::{FieldFault, Problem};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::users::{Avatar, UserKind, UserRef};
use orch_authz::Error as AuthError;
use orch_authz::pages::ChangeError;
use orch_db::Conn;
use orch_db::queries::{hierarchy, identity};
use orch_types::Id;
use serde::Serialize;
use std::collections::HashMap;

/// A person as others see them, from what the database keeps of one.
#[must_use]
pub fn user_ref(brief: &identity::UserBrief) -> UserRef {
    UserRef {
        id: brief.id,
        kind: if brief.kind == "service" {
            UserKind::Service
        } else {
            UserKind::Human
        },
        username: brief.username.clone(),
        address: Some(format!("@{}", brief.username)),
        display_name: brief.display_name.clone(),
        avatar: Avatar {
            text: avatar_text(&brief.display_name),
            color: u32::try_from(brief.avatar_color).unwrap_or_default(),
        },
    }
}

/// The people `ids` as others see them, by identifier. Somebody who is not
/// there is not among them.
pub async fn user_refs(conn: &mut Conn, ids: &[Id]) -> ApiResult<HashMap<Id, UserRef>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(identity::users_brief(conn, ids)
        .await?
        .iter()
        .map(|brief| (brief.id, user_ref(brief)))
        .collect())
}

/// The kind of a node as a word of the API.
#[must_use]
pub fn node_kind(word: &str) -> Option<NodeKind> {
    match word {
        "org" => Some(NodeKind::Org),
        "project" => Some(NodeKind::Project),
        "topic" => Some(NodeKind::Topic),
        "thread" => Some(NodeKind::Thread),
        _ => None,
    }
}

/// A kind of thing as the word that names it in a problem.
fn word<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// A node named in a problem, with its name: the caller was just let in
/// there, or is suspended there, so it is theirs to know.
pub async fn named_node(conn: &mut Conn, id: Id) -> ApiResult<NodeRef> {
    let found = hierarchy::with_ancestors(conn, &[id]).await?;
    let node = found.iter().find(|node| node.id == id);
    Ok(NodeRef {
        thread_display: None,
        id,
        kind: node
            .and_then(|node| node_kind(&node.kind))
            .unwrap_or(NodeKind::Org),
        name: node.map(|node| node.name.clone()),
        redacted: node.is_none(),
    })
}

fn closed_reason(reason: Closed) -> ClosedReason {
    match reason {
        Closed::SubscriptionNotShared => ClosedReason::SubscriptionNotShared,
        Closed::Locked => ClosedReason::Locked,
    }
}

/// What a refusal says is missing, thing by thing, with the nearest people
/// who can give each by name.
pub async fn missing_access(
    conn: &mut Conn,
    missing: &[decisions::MissingAccess],
) -> ApiResult<Vec<MissingAccess>> {
    let people: Vec<Id> = missing
        .iter()
        .flat_map(|missing| {
            missing
                .ask
                .iter()
                .copied()
                .chain(missing.object.owner)
                .collect::<Vec<_>>()
        })
        .collect();
    let people = user_refs(conn, &people).await?;
    Ok(missing
        .iter()
        .map(|missing| MissingAccess {
            object: MissingAccessObject {
                kind: word(&missing.object.kind),
                id: missing.object.id,
                name: missing.object.name.clone(),
                owner: missing
                    .object
                    .owner
                    .and_then(|owner| people.get(&owner).cloned()),
            },
            permissions: missing
                .permissions
                .iter()
                .map(|permission| permission.as_str().to_owned())
                .collect(),
            options: missing.options.as_ref().map(|options| AccessOptions {
                models: options.models.clone(),
                efforts: options.efforts.clone(),
            }),
            ask: missing
                .ask
                .iter()
                .filter_map(|user| people.get(user).cloned())
                .collect(),
            ask_more: missing.ask_more,
            r#override: missing.r#override,
            closed: missing.closed.map(closed_reason),
        })
        .collect())
}

/// The refusal of the engine as a problem: `403 authz.denied` with what was
/// required, of which thing, whether a request for access can be made,
/// whether the caller may force the act, and everything that is missing,
/// thing by thing, with the nearest people who can give it.
pub async fn denied(conn: &mut Conn, refusal: Refusal) -> ApiResult<ApiError> {
    let missing = missing_access(conn, &refusal.missing).await?;
    let kind = word(&refusal.object.kind);
    Ok(ApiError::new(
        ErrorCode::AuthzDenied,
        format!("{} is required on this {kind}", refusal.required.as_str()),
    )
    .with("required", refusal.required.as_str())
    .with(
        "object",
        serde_json::json!({ "kind": kind, "id": refusal.object.id }),
    )
    .with("requestable", refusal.requestable)
    .with("override", refusal.r#override)
    .with("missing", missing))
}

/// What went wrong in the engine, as the answer a client gets. A suspension
/// in an org names the org, an archive names the node that is archived; a
/// node that is not there, or a permission that the kind of node it turned
/// out to be has none of (the address named the wrong kind of thing), is
/// `not_found`.
pub async fn problem(conn: &mut Conn, error: AuthError) -> ApiError {
    match error {
        AuthError::Db(error) => error.into(),
        AuthError::NotFound(_) | AuthError::NoSuchPermission { .. } => ApiError::not_found(),
        AuthError::NoActiveOwner { .. } => Problem::invalid_at(
            "owner",
            "this place has no active member who can be its first owner",
        )
        .into(),
        AuthError::Suspended { org: None } => {
            ApiError::new(ErrorCode::AuthSuspended, "this user is suspended")
        }
        AuthError::Suspended { org: Some(org) } => match named_node(conn, org).await {
            Ok(org) => ApiError::new(ErrorCode::OrgSuspended, "you are suspended in this org")
                .with("org", org),
            Err(error) => error,
        },
        AuthError::Archived { node } => match named_node(conn, node).await {
            Ok(node) => ApiError::new(
                ErrorCode::NodeArchived,
                "this is archived, and so is read-only",
            )
            .with("archived_node", node),
            Err(error) => error,
        },
        other @ (AuthError::NoSuchUser(_) | AuthError::Stored(_)) => ApiError::internal(other),
    }
}

/// Whether the engine's `Decision` came out allowed; if not, the problem.
pub async fn settle(conn: &mut Conn, decision: decisions::Decision) -> ApiResult<()> {
    match decision {
        decisions::Decision::Allowed { .. } => Ok(()),
        decisions::Decision::Refused(refusal) => Err(denied(conn, refusal).await?),
    }
}

/// Why a change of a page was not made, as the problem a client acts on:
/// what the caller lacks and who can give it, a page somebody else changed
/// meanwhile, a row that a lock above decides and where it was set,
/// somebody from outside that the caller may not name and who may, the
/// last owner who stays, and what in the change does not fit the page.
pub async fn change_failed(conn: &mut Conn, error: ChangeError) -> ApiError {
    match error {
        ChangeError::Engine(error) => problem(conn, error).await,
        ChangeError::Denied(refusal) => match denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        ChangeError::SystemAdminOnly => ApiError::new(
            ErrorCode::AuthzDenied,
            "only a system admin changes the defaults of the system",
        )
        .with("required", "access.manage")
        .with(
            "object",
            serde_json::json!({ "kind": "system", "id": Id::nil() }),
        )
        .with("requestable", false)
        .with("override", false)
        .with("missing", Vec::<MissingAccess>::new()),
        ChangeError::Invalid(faults) => Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: fault.path,
                    message: fault.message,
                })
                .collect(),
        )
        .into(),
        // The refusal does not say which harness or which place keeps it so;
        // the route that knows the provider adds them.
        ChangeError::SubscriptionNotShared => ApiError::new(
            ErrorCode::ProviderSubscriptionNotShared,
            "a subscription of this harness stays with its first owner, so nobody else is named in its Use row",
        )
        .with("harness_kind", serde_json::Value::Null)
        .with("scope", serde_json::Value::Null),
        ChangeError::FirstOwnerStays { owner } => ApiError::from(Problem::invalid_at(
            "owners.first",
            "what is registered from a person's machine keeps that machine's owner as its first owner",
        ))
        .with("owner", owner),
        ChangeError::Stale { current } => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the page was changed since it was read",
        )
        .with("current", current),
        ChangeError::Locked { permission, by } => {
            let place = match by {
                PlaceRef::System => None,
                PlaceRef::Node(node) => named_node(conn, node).await.ok(),
            };
            ApiError::new(
                ErrorCode::AccessLocked,
                format!("{} is decided by a lock above", permission.as_str()),
            )
            .with("permission", permission.as_str())
            .with("by", place)
        }
        ChangeError::Outside { ask, ask_more } => external_forbidden(conn, &ask, ask_more).await,
        ChangeError::ExternalLifetime { latest, place } => {
            let scope = match place {
                Some(node) => named_node(conn, node).await.ok(),
                None => None,
            };
            ApiError::new(
                ErrorCode::SharingExternalLifetimeExceeded,
                "somebody from outside may be named here only with an end no later than the place allows",
            )
            .with("latest_end", latest)
            .with("scope", scope)
        }
        ChangeError::LastOwner => ApiError::new(
            ErrorCode::OwnerLast,
            "the last person in an Owners row stays; name another first",
        ),
    }
}

/// `403 sharing.external_forbidden`: somebody from outside the root org is
/// named by a caller who may not, with the nearest people who may.
pub async fn external_forbidden(conn: &mut Conn, ask: &[Id], ask_more: i64) -> ApiError {
    let people = user_refs(conn, ask).await.unwrap_or_default();
    let ask: Vec<UserRef> = ask
        .iter()
        .filter_map(|user| people.get(user).cloned())
        .collect();
    ApiError::new(
        ErrorCode::SharingExternalForbidden,
        "somebody from outside the root org may be named only by those who hold the right to",
    )
    .with("ask", ask)
    .with("ask_more", ask_more)
}

/// A refusal of something that only a system admin does, with nobody to ask:
/// it is shut to everybody else.
#[must_use]
pub fn system_admin_only(detail: &str) -> ApiError {
    ApiError::new(ErrorCode::AuthzDenied, detail)
        .with("required", "system.admin")
        .with(
            "object",
            serde_json::json!({ "kind": "system", "id": Id::nil() }),
        )
        .with("requestable", false)
        .with("override", false)
        .with("missing", Vec::<MissingAccess>::new())
}
