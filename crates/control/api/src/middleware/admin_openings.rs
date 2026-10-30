//! One audit row for each private object opened through a browser session's administrative reach.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::routes::Method;
use orch_authz::engine::{Frame, Graph, view_permission};
use orch_authz::{Policy, Principal};
use orch_db::{Conn, audit};
use orch_types::{Id, Json};

/// A nonce identifies a logical screen opening across its initial HTTP reads and live feeds.
/// It never authorizes a read. Without it a successful content read is its own opening.
pub(crate) fn nonce(ctx: &Ctx) -> ApiResult<Id> {
    match ctx.headers.get("x-orch-opening") {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| ApiError::invalid("x-orch-opening", "must be an opening UUID")),
        None => Ok(ctx.opening),
    }
}

fn session(ctx: &Ctx) -> ApiResult<Id> {
    match &ctx.user()?.via {
        crate::middleware::authentication::Via::Session { session_id } => Ok(*session_id),
        _ => Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthSessionRequired,
            "administrative openings belong to a browser session",
        )),
    }
}

/// A successful object reply carries the marker produced by its actual authorization frame.
/// Lists and search hits are not openings. Failed reads and ordinary page access leave no row.
pub(crate) async fn finish_json(ctx: &Ctx, value: &Json) -> ApiResult<()> {
    if ctx.route.method != Method::GET || ctx.inspect_deleted {
        return Ok(());
    }
    let place = value
        .pointer("/access/admin_access/place")
        .or_else(|| value.pointer("/can/admin_access/place"))
        .or_else(|| value.get("inspected").filter(|place| !place.is_null()));
    let Some(place) = place else {
        return Ok(());
    };
    let path: Vec<_> = ctx.route.path.trim_matches('/').split('/').collect();
    if path.len() != 2 {
        return Ok(());
    }
    let kind = match path[0] {
        "orgs" => "org",
        "projects" => "project",
        "topics" => "topic",
        "threads" => "thread",
        "documents" => "document",
        "chats" => "chat",
        // A person's page is read at `/profiles/{username}`; its reply names them in `user`.
        "profiles" | "users" => "user",
        "environments" => "environment",
        "providers" => "provider",
        "agents" => "agent",
        "hosts" => "host",
        "machines" => "machine",
        "templates" => "template",
        "mounts" => "mount",
        "mcp-servers" => "mcp_server",
        "skills" => "skill",
        _ => return Ok(()),
    };
    let id = if kind == "user" {
        value.pointer("/user/id")
    } else {
        value.get("id")
    }
    .and_then(Json::as_str)
    .and_then(|id| id.parse().ok())
    .ok_or_else(|| ApiError::internal("a private object reply has no identity"))?;
    let mut tx = ctx.app.db.begin().await?;
    note(ctx, &mut tx, kind, id, place.clone(), nonce(ctx)?).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Appends only after a real reader has established native private access.
pub(crate) async fn note(
    ctx: &Ctx,
    conn: &mut Conn,
    kind: &str,
    id: Id,
    place: Json,
    opening: Id,
) -> ApiResult<()> {
    let user = ctx.user()?.id();
    let mut origin = crate::sign_in::origin(ctx);
    origin.as_admin = Some(user);
    let node = place
        .get("id")
        .and_then(Json::as_str)
        .and_then(|id| id.parse().ok());
    audit::scope_origin(
        origin.clone(),
        audit::opened(
            conn,
            audit::Opening {
                user,
                session: session(ctx)?,
                nonce: opening,
                kind,
                object: id,
                place,
                node,
            },
            &origin,
            ctx.app.clock.now(),
        ),
    )
    .await?;
    Ok(())
}

/// Freshly checks the native scope for a cached screen opening or an initial live subscription.
pub(crate) async fn private_place(
    conn: &mut Conn,
    principal: &Principal,
    kind: &str,
    id: Id,
) -> ApiResult<Option<PlaceRef>> {
    if !matches!(kind, "chat" | "user") && kind.parse::<ObjectKind>().is_err() {
        return Err(ApiError::invalid(
            "kind",
            "is not a kind of readable object",
        ));
    }
    if !principal.caller().as_admin() {
        return Ok(None);
    }
    if kind == "chat" {
        let row = orch_db::queries::chats::chat(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let members = orch_db::queries::chats::members(conn, id).await?;
        return orch_authz::chats::standing(
            conn,
            principal,
            &row,
            &members,
            orch_authz::chats::ChatPolicy::default(),
        )
        .await
        .map(|standing| standing.admin_access.map(|reach| reach.place))
        .map_err(ApiError::internal);
    }
    if kind == "user" {
        return crate::routes::profiles::admin_place(conn, principal, id).await;
    }
    let kind: ObjectKind = kind
        .parse()
        .map_err(|_| ApiError::invalid("kind", "is not a kind of readable object"))?;
    let (graph, site) = if kind.is_document() {
        let (graph, _, site) = orch_authz::documents::site(conn, id)
            .await
            .map_err(ApiError::internal)?;
        (graph, site)
    } else if kind.node().is_some() {
        let graph = Graph::load(conn, &[id]).await.map_err(ApiError::internal)?;
        let site = graph
            .site_of(id)
            .filter(|site| site.kind == kind)
            .ok_or_else(ApiError::not_found)?;
        (graph, site)
    } else {
        let object = if kind == ObjectKind::Machine {
            match orch_db::queries::resources::hosting::host_of_machine(conn, id).await? {
                Some(host) => ObjectRef {
                    kind: ObjectKind::Host,
                    id: host,
                },
                None => ObjectRef { kind, id },
            }
        } else {
            ObjectRef { kind, id }
        };
        let (graph, _, site) = orch_authz::resources::site(conn, object)
            .await
            .map_err(ApiError::internal)?;
        (graph, site)
    };
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[view_permission(site.kind)],
    )
    .await
    .map_err(ApiError::internal)?;
    Ok(frame.admin_access(&site).map(|reach| reach.place))
}

/// Cached views announce their opening too; the nonce joins the HTTP/feed audit, never a grant.
pub(crate) async fn cached(ctx: &Ctx, kind: &str, id: Id, opening: Id) -> ApiResult<()> {
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
    if let Some(place) = private_place(&mut tx, guard.principal(), kind, id).await? {
        note(ctx, &mut tx, kind, id, stored_place(place), opening).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Stable place identity for an audit record, independent of current display names.
pub(crate) fn stored_place(place: PlaceRef) -> Json {
    match place {
        PlaceRef::System => Json::String("system".to_owned()),
        PlaceRef::Node(id) => serde_json::json!({ "id": id }),
    }
}

/// Raw content has no JSON marker. Recheck its exact parent object's native View
/// under the ordinary identity/authority guard before returning the content.
pub(crate) async fn finish_raw(ctx: &Ctx) -> ApiResult<()> {
    if ctx.route.method != Method::GET
        || ctx.inspect_deleted
        || !ctx
            .user()
            .is_ok_and(|user| user.as_admin && user.is_session())
    {
        return Ok(());
    }
    let first = ctx
        .route
        .path
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    let first = if first == "{object}" {
        ctx.param("object")?
    } else {
        first
    };
    let kind = match first {
        "orgs" => "org",
        "projects" => "project",
        "topics" => "topic",
        "threads" => "thread",
        "documents" => "document",
        "chats" => "chat",
        "environments" => "environment",
        "providers" => "provider",
        "agents" => "agent",
        "hosts" => "host",
        "machines" => "machine",
        "templates" => "template",
        "mounts" => "mount",
        "mcp-servers" => "mcp_server",
        "skills" => "skill",
        _ => return Ok(()),
    };
    let Ok(id) = ctx.id("id") else {
        return Ok(());
    };
    cached(ctx, kind, id, ctx.opening).await
}
