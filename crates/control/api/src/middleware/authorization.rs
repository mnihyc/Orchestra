//! Delegate decisions to orch-authz, and carry what is missing, and whether the caller may force it, into refusals.
//!
//! An endpoint declared with one permission is checked here before its
//! handler runs: the permission, on the object its path names. Every
//! decision is the access engine's; this module only finds the object and
//! turns what the engine says into the problem a client can act on
//! ([`refusals`]). A handler whose request is declared with a rule asks the
//! same questions itself, through [`require`], and never decides by itself.
//!
//! The object is found from the path the request is declared with: the
//! first segment says what kind of thing the `{id}` after it names. Nodes,
//! documents and the seven kinds of resource are served, each decided by
//! the engine of its own side; a path that names anything else is refused as
//! not connected, so that nothing is ever let through for want of a rule.

pub mod refusals;

use super::authentication::{Authed, Via};
use super::requests::{ApiError, ApiResult};
use super::transaction_guard;
use crate::registry::Ctx;
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{
    NodePermission, ObjectKind, Permission, PermissionName, ResourceKind,
};
use orch_api_contract::common::ErrorCode;
use orch_authz::{Act, Caller, Policy, Principal, TokenGrant, documents, nodes, resources};
use orch_db::Conn;
use orch_db::queries::hierarchy;
use orch_types::{Id, Time};

/// The permission of the node's own area that a request names for what it
/// reads.
pub const VIEW: PermissionName = PermissionName::OfNode(NodePermission::View);
/// The permission of the node's own area that a request names for what it
/// changes.
pub const MANAGE: PermissionName = PermissionName::OfNode(NodePermission::Manage);

/// A permission of the catalogue, as a request names it.
#[must_use]
pub const fn exact(permission: Permission) -> PermissionName {
    PermissionName::Exact(permission)
}

/// The caller as the access engine knows them: a browser session, or an
/// API token with the scopes and the subtree it was made with.
pub fn caller_of(authed: &Authed) -> ApiResult<Caller> {
    match &authed.via {
        Via::Session { .. } => Ok(Caller::session(authed.id()).with_admin_mode(authed.as_admin)),
        Via::Token {
            token_id,
            scopes,
            node_id,
        } => TokenGrant::from_stored(scopes, *node_id)
            // A synthetic grant (the nil id) is no stored token: nothing refuses it or bounds its life.
            .map(|grant| {
                if token_id.is_nil() {
                    grant
                } else {
                    grant.for_token(*token_id)
                }
            })
            .map(|grant| Caller::token(authed.id(), grant))
            .map_err(ApiError::internal),
    }
}

/// The session or API token that gave a short-lived handle to a long-lived stream.
/// No bearer secret is retained, and each use reloads the credential and its current authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetainedIdentity {
    user: Id,
    via: Via,
    issued_as_admin: bool,
}

impl RetainedIdentity {
    pub(crate) fn of(authed: &Authed) -> Self {
        Self {
            user: authed.id(),
            via: authed.via.clone(),
            issued_as_admin: authed.is_session() && authed.as_admin,
        }
    }

    pub(crate) fn from_stored(user: Id, kind: &str, id: Id, issued_as_admin: bool) -> Option<Self> {
        let via = match kind {
            "session" => Via::Session { session_id: id },
            "token" => Via::Token {
                token_id: id,
                scopes: Vec::new(),
                node_id: None,
            },
            _ => return None,
        };
        Some(Self {
            user,
            via,
            issued_as_admin,
        })
    }

    pub(crate) async fn principal(
        &self,
        conn: &mut Conn,
        now: Time,
    ) -> ApiResult<Option<Principal>> {
        use orch_db::queries::identity;

        let Some(user) = identity::user_by_id(conn, self.user)
            .await?
            .filter(|user| !user.suspended())
        else {
            return Ok(None);
        };
        let (as_admin, via) = match &self.via {
            Via::Session { session_id } => {
                let Some(as_admin) =
                    identity::session_admin_mode(conn, self.user, *session_id, now.get()).await?
                else {
                    return Ok(None);
                };
                (
                    as_admin && self.issued_as_admin,
                    Via::Session {
                        session_id: *session_id,
                    },
                )
            }
            Via::Token { token_id, .. } => {
                let row: Option<(Vec<String>, Option<Id>)> = sqlx::query_as(
                    "SELECT scopes, node_id FROM api_tokens WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$3)",
                )
                .bind(token_id)
                .bind(self.user)
                .bind(now.get())
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
                let Some((scopes, node_id)) = row else {
                    return Ok(None);
                };
                (
                    false,
                    Via::Token {
                        token_id: *token_id,
                        scopes,
                        node_id,
                    },
                )
            }
        };
        let authed = Authed {
            as_admin,
            user,
            via,
        };
        principal_of(conn, &authed, now).await.map(Some)
    }
}

/// The current caller; inside an action transaction, also rechecks the
/// credential and the endpoint's declared permission through the action guard.
pub async fn principal(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Principal> {
    if transaction_guard::changes(ctx) && orch_db::queries::transaction_guard::in_transaction(conn)
    {
        return Ok(transaction_guard::check_conn(ctx, conn)
            .await?
            .into_principal());
    }
    principal_of(conn, ctx.user()?, Time::new(ctx.app.clock.now())).await
}

pub(crate) async fn load_principal(
    conn: &mut Conn,
    caller: Caller,
    now: Time,
) -> ApiResult<Principal> {
    match Principal::load(conn, caller, now).await {
        Ok(principal) => Ok(principal),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

/// The principal of the signed-in caller `authed`, held out of the team orgs that wait for a second
/// step of theirs (until it is set up, or given in this session).
pub(crate) async fn principal_of(
    conn: &mut Conn,
    authed: &Authed,
    now: Time,
) -> ApiResult<Principal> {
    let principal = load_principal(conn, caller_of(authed)?, now).await?;
    crate::sign_in::second_step::hold(conn, authed, principal).await
}

/// The problem of an error of the access engine for `principal`: a hold for a second step is told
/// as the step it waits for, a suspension in the same org as the suspension, and anything else as
/// [`refusals::problem`] tells it.
pub async fn engine_problem(
    conn: &mut Conn,
    principal: &Principal,
    error: orch_authz::Error,
) -> ApiError {
    if let orch_authz::Error::Suspended { org: Some(org) } = &error
        && !principal.suspension().in_orgs().contains(org)
        && let Some(hold) = principal
            .second_step_hold()
            .filter(|hold| hold.orgs.contains(org))
    {
        return crate::sign_in::second_step::refusal(conn, hold).await;
    }
    refusals::problem(conn, error).await
}

/// Notes, for "Last seen" in the orgs it lies in, that the caller used the node or the document that
/// the request's path names, now that its permission was granted. Nothing else is noted: a request
/// about a resource, or about no one thing, says nothing of where its caller was. A failure to note
/// it is logged and costs the request nothing.
pub(crate) async fn note_use(ctx: &Ctx) {
    let Ok(authed) = ctx.user() else {
        return;
    };
    let noted = async {
        let mut conn = ctx.app.db.acquire().await?;
        let node = match subject(ctx) {
            Ok(Subject::Node(id)) => Some(id),
            Ok(Subject::Address(address)) => hierarchy::org_by_address(&mut conn, &address).await?,
            Ok(Subject::Document(id)) => {
                sqlx::query_scalar::<_, Id>("SELECT node_id FROM documents WHERE id=$1 UNION ALL SELECT node_id FROM document_containers WHERE id=$1 LIMIT 1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?
            }
            Ok(Subject::Resource(..)) | Err(_) => None,
        };
        if let Some(node) = node {
            orch_db::queries::second_step::note_use(
                &mut conn,
                authed.id(),
                node,
                ctx.now,
                crate::sign_in::second_step::LAST_SEEN_EVERY,
            )
            .await?;
        }
        Ok::<(), ApiError>(())
    };
    if let Err(error) = noted.await {
        tracing::warn!(request = ctx.route.name, %error, "a use could not be noted for Last seen");
    }
}

/// What a request's path says its object is.
enum Subject {
    /// A node, by its identifier.
    Node(Id),
    /// An org, by its short name or its address.
    Address(String),
    /// A resource of a kind, by its identifier.
    Resource(ResourceKind, Id),
    /// A document, by its identifier.
    Document(Id),
}

/// The kind of resource a plural in a path names (`providers`, `mcp-servers`).
#[must_use]
pub fn resource_kind_of(plural: &str) -> Option<ResourceKind> {
    match plural {
        "providers" => Some(ResourceKind::Provider),
        "agents" => Some(ResourceKind::Agent),
        "environments" => Some(ResourceKind::Environment),
        "hosts" => Some(ResourceKind::Host),
        "machines" => Some(ResourceKind::Machine),
        "templates" => Some(ResourceKind::Template),
        "mcp-servers" | "mcp_servers" => Some(ResourceKind::McpServer),
        "mounts" => Some(ResourceKind::Mount),
        "skills" => Some(ResourceKind::Skill),
        _ => None,
    }
}

/// Finds the object of a request from the path it is declared with.
fn subject(ctx: &Ctx) -> ApiResult<Subject> {
    let mut parts = ctx.route.path.trim_start_matches('/').split('/');
    match (parts.next(), parts.next()) {
        (Some("orgs"), Some("by-slug")) => Ok(Subject::Address(ctx.param("slug")?.to_owned())),
        (Some("nodes" | "orgs" | "projects" | "topics" | "threads"), Some("{id}")) => {
            Ok(Subject::Node(ctx.id("id")?))
        }
        (Some("me"), Some("favorites")) => Ok(Subject::Node(ctx.id("node_id")?)),
        // A person's exception from their notifications is about the node it names.
        (Some("me"), Some("notification-exceptions")) => Ok(Subject::Node(ctx.id("id")?)),
        (Some("documents" | "document-containers"), Some("{id}")) => {
            Ok(Subject::Document(ctx.id("id")?))
        }
        (Some("{object}"), Some("{id}")) => match ctx.param("object")? {
            "nodes" => Ok(Subject::Node(ctx.id("id")?)),
            "documents" | "document-containers" => Ok(Subject::Document(ctx.id("id")?)),
            other => match resource_kind_of(other) {
                Some(kind) => Ok(Subject::Resource(kind, ctx.id("id")?)),
                // A word in the path that names no kind of thing that has a page (a chat has
                // none) is a thing that is not there, whatever the caller wrote.
                None => Err(ApiError::new(
                    ErrorCode::NotFound,
                    format!("there is nothing called {other} that has a permission page"),
                )),
            },
        },
        (Some(plural), Some("{id}")) => match resource_kind_of(plural) {
            Some(kind) => Ok(Subject::Resource(kind, ctx.id("id")?)),
            None => Err(not_connected(ctx)),
        },
        _ => Err(not_connected(ctx)),
    }
}

/// Where a request acts, as its path names it, for the authorization epochs its commit checks:
/// the node, the document or the resource of its path; nothing for an org named by its
/// address, or a path that names no one thing.
pub(crate) fn place_of(ctx: &Ctx) -> orch_db::queries::transaction_guard::Place {
    use orch_db::queries::transaction_guard::{Place, resource_table};
    // As `subject` reads the path, without making (and logging) a problem for a path that names no place.
    let id = || {
        ctx.param_if_any("id")
            .and_then(|value| value.parse::<Id>().ok())
    };
    let node = |node| Place {
        node,
        ..Place::default()
    };
    let document = |document| Place {
        document,
        ..Place::default()
    };
    let resource = |kind: &str| Place {
        resource: resource_table(kind).zip(id()),
        ..Place::default()
    };
    let mut parts = ctx.route.path.trim_start_matches('/').split('/');
    match (parts.next(), parts.next()) {
        (Some("nodes" | "orgs" | "projects" | "topics" | "threads"), Some("{id}")) => node(id()),
        (Some("documents"), Some("{id}")) => document(id()),
        (Some("{object}"), Some("{id}")) => match ctx.param_if_any("object") {
            Some("nodes") => node(id()),
            Some("documents") => document(id()),
            Some(other) => resource(other),
            None => Place::default(),
        },
        (Some(plural), Some("{id}")) => resource(plural),
        _ => Place::default(),
    }
}

/// What a request asks of a node that is archived: deleting is let through
/// under archive, with the permission that deletes; the rest is as the
/// engine says per permission.
fn act_of(ctx: &Ctx) -> Act {
    if ctx.route.method.as_str() == "DELETE" && ctx.route.path == "/nodes/{id}" {
        Act::Delete
    } else {
        Act::Use
    }
}

fn not_connected(ctx: &Ctx) -> ApiError {
    ApiError::internal(format!(
        "internal authorization routing error: {} {} has no access target",
        ctx.route.method.as_str(),
        ctx.route.path
    ))
}

/// Requires `permission` on the object that the request's path names, and
/// refuses with what is missing when the caller does not hold it.
pub async fn require_on_path(ctx: &Ctx, permission: &str) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = principal(ctx, &mut conn).await?;
    require_on_path_in(ctx, &mut conn, &principal, permission).await
}

pub(crate) async fn require_on_path_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    permission: &str,
) -> ApiResult<()> {
    let _ = ctx.user()?;
    if ctx.inspect_deleted && ctx.route.name == "EnvironmentGet" {
        let id = ctx.id("id")?;
        if orch_db::queries::environments::retained::removal(conn, id)
            .await?
            .is_some_and(|r| r.deleted_at.is_some())
        {
            crate::routes::environments::retained::may_read(conn, principal, id).await?;
            return Ok(());
        }
    }
    if ctx.inspect_deleted
        && crate::middleware::inspection::prepare(ctx, conn, principal)
            .await?
            .is_some()
    {
        return Ok(());
    }
    let name: PermissionName = permission.parse().map_err(|error| {
        ApiError::internal(format!(
            "{} {} declares {permission}, which is no permission: {error}",
            ctx.route.method.as_str(),
            ctx.route.path
        ))
    })?;
    let subject = subject(ctx)?;
    let node = match subject {
        Subject::Node(id) => id,
        Subject::Address(address) => hierarchy::org_by_address(conn, &address)
            .await?
            .ok_or_else(ApiError::not_found)?,
        Subject::Resource(kind, id) => {
            let permission = name.on(kind.into()).ok_or_else(|| {
                ApiError::internal(format!(
                    "{} {} declares {permission}, which a {kind} does not have",
                    ctx.route.method.as_str(),
                    ctx.route.path
                ))
            })?;
            return require_resource(conn, principal, kind, id, permission, Policy::default())
                .await;
        }
        Subject::Document(id) => {
            let permission = name.on(ObjectKind::Document).ok_or_else(|| {
                ApiError::internal(format!(
                    "{} {} declares {permission}, which a document does not have",
                    ctx.route.method.as_str(),
                    ctx.route.path
                ))
            })?;
            return require_document(conn, principal, id, permission, Policy::default()).await;
        }
    };
    require(conn, principal, node, name, act_of(ctx), Policy::default()).await
}

/// Requires `permission` on the node `node` of the caller, `act` being
/// what is done: the engine decides, and a refusal comes back as the
/// problem that carries what is missing, who can give it and whether the
/// caller may force the act.
pub async fn require(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    permission: PermissionName,
    act: Act,
    policy: Policy,
) -> ApiResult<()> {
    match nodes::check_named(conn, principal, node, permission, act, policy).await {
        Ok(Decision::Allowed { .. }) => Ok(()),
        Ok(Decision::Refused(refusal)) => match kept_out_problem(conn, principal, node).await? {
            Some(problem) => Err(problem),
            None => Err(refusals::denied(conn, refusal).await?),
        },
        Err(error) => Err(engine_problem(conn, principal, error).await),
    }
}

/// Where a token was refused on the node `node` because a place on its way keeps the token out (an org
/// that refused it, or a place that admits a shorter life), the problem that says so; `None` for every
/// other refusal, which stays what the engine said.
pub async fn kept_out_problem(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
) -> ApiResult<Option<ApiError>> {
    let Some(grant) = principal.caller().grant() else {
        return Ok(None);
    };
    let path: Vec<Id> = hierarchy::with_ancestors(conn, &[node])
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    Ok(grant.kept_out_of(Some(&path)).map(kept_out))
}

/// The problem of a token kept out of a place: `token.refused_here` with the org, or
/// `token.lifetime_exceeded` with the longest life the place admits and the place (`null` for the system).
#[must_use]
pub fn kept_out(place: orch_authz::KeptOut) -> ApiError {
    match place {
        orch_authz::KeptOut::Refused { org } => ApiError::new(
            ErrorCode::TokenRefusedHere,
            "this token is refused in this org",
        )
        .with("org", org),
        orch_authz::KeptOut::Lifetime { place, max_days } => ApiError::new(
            ErrorCode::TokenLifetimeExceeded,
            format!("a token admitted here lives {max_days} days at most"),
        )
        .with("max_days", max_days)
        .with("scope", place),
    }
}

/// Requires `permission` on the resource `id` of the kind `kind` of the caller: the engine
/// decides, and a refusal comes back as the problem that carries what is missing and who can
/// give it.
pub async fn require_resource(
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    id: Id,
    permission: Permission,
    policy: Policy,
) -> ApiResult<()> {
    let object = ObjectRef {
        kind: kind.into(),
        id,
    };
    match resources::check(conn, principal, object, permission, policy).await {
        Ok(Decision::Allowed { .. }) => Ok(()),
        Ok(Decision::Refused(refusal)) => Err(refusals::denied(conn, refusal).await?),
        Err(error) => Err(engine_problem(conn, principal, error).await),
    }
}

/// Requires `permission` on the document `id` of the caller: the engine decides, and a refusal
/// comes back as the problem that carries what is missing and who can give it; a document under
/// an archived node that the caller would otherwise hold it on is `409 node.archived`.
pub async fn require_document(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
    policy: Policy,
) -> ApiResult<()> {
    match documents::check(conn, principal, id, permission, policy).await {
        Ok(Decision::Allowed { .. }) => Ok(()),
        Ok(Decision::Refused(refusal)) => Err(refusals::denied(conn, refusal).await?),
        Err(error) => Err(engine_problem(conn, principal, error).await),
    }
}
