//! Select the native reduced resource view before any global resource renderer.
use super::*;
use crate::routes::resources::context::Request;

pub(super) async fn selection(
    origin: &ReadContext,
    route: &Route,
    parameters: &ReadParameters,
    requested_node: Option<Id>,
) -> ApiResult<Option<Request>> {
    let (kind, single) = match route.name {
        "ProviderGet" => (ObjectKind::Provider, true),
        "AgentGet" => (ObjectKind::Agent, true),
        "ProviderList" => (ObjectKind::Provider, false),
        "AgentList" => (ObjectKind::Agent, false),
        _ => return Ok(None),
    };
    if requested_node.is_none() {
        if single {
            let id = parameters
                .path
                .get("id")
                .and_then(|id| id.parse().ok())
                .ok_or_else(|| denied("Use a complete resource identity"))?;
            let mut conn = origin.app().db.acquire().await?;
            let target = crate::key_operations::targets::resource_identity(
                &mut conn,
                orch_access_contract::decisions::ObjectRef { kind, id },
            )
            .await?;
            // Ordinary full reads in this root keep their existing capabilities.
            // The real read footprint checks this placement again before rendering.
            if target
                .node_path()
                .is_some_and(|path| path.contains(&origin.root()))
            {
                return Ok(None);
            }
        } else if parameters
            .query
            .get("org_id")
            .is_some_and(|id| !id.is_null())
        {
            // A caller explicitly asked for the native home filter. Its actual
            // source footprint must fit the fixed root; never reinterpret a home.
            return Ok(None);
        }
    }
    for (key, value) in &parameters.query {
        if !value.is_null() && !matches!(key.as_str(), "cursor" | "limit") {
            return Err(denied(
                "A work-context resource list accepts cursor and limit. Use a native home-filtered read for other filters.",
            ));
        }
    }
    let after = parameters
        .query
        .get("cursor")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .and_then(|value| value.parse::<Id>().ok())
                .ok_or_else(|| denied("Use the resource list's native cursor"))
        })
        .transpose()?;
    let limit = parameters
        .query
        .get("limit")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .or_else(|| value.as_str().and_then(|value| value.parse::<u32>().ok()))
                .ok_or_else(|| denied("Use a nonnegative native page limit"))
        })
        .transpose()?;
    let node = match requested_node {
        Some(node) => node,
        None => default_node(origin).await?,
    };
    Ok(Some(Request { node, after, limit }))
}

async fn default_node(origin: &ReadContext) -> ApiResult<Id> {
    let ReadContext::Ask(ask) = origin else {
        return Ok(origin.conversation());
    };
    let mut conn = origin.app().db.acquire().await?;
    let Some(context) = rows::get(&mut conn, origin.conversation(), false)
        .await?
        .and_then(|row| row.context)
    else {
        return Ok(origin.root());
    };
    let (kind, id, _) = reference_id(&ask.source, &mut conn, &context).await?;
    if matches!(kind.as_str(), "org" | "project" | "topic" | "thread") {
        return Ok(id);
    }
    // A document/resource URL is not proof of its protected parent. The model
    // may select an explicit readable node with list; otherwise use the fixed root.
    Ok(origin.root())
}
