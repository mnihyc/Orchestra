//! Existing native context and candidates, before binding choices are formatted.

use super::*;

use orch_access_contract::decisions::ObjectRef;
pub(super) struct Context {
    pub node: hierarchy::NodeDetail,
    pub settings: Resolved,
    pub environment_default: EnvironmentDefault,
    pub judged: Option<(environments::EnvironmentRow, EnvironmentOffers)>,
    pub sources: Vec<ObjectRef>,
    pub orch_tools: OrchToolsOf,
    pub policy_scope: Option<Id>,
}

pub(super) fn permission(kind: &str) -> ApiResult<Permission> {
    match kind {
        "topic" => Ok(Permission::TopicStartThreads),
        "project" => Ok(Permission::ProjectCreateTopics),
        _ => Err(invalid(
            "id",
            "a thread starts in a topic, project or folder",
        )),
    }
}

pub(super) async fn require(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<hierarchy::NodeDetail> {
    let node = hierarchy::details(conn, &[id], now)
        .await?
        .into_iter()
        .next()
        .filter(|node| node.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let permission = permission(&node.kind)?;
    let decision = nodes::check(conn, principal, id, permission, Act::Use, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    allowed(conn, decision).await?;
    Ok(node)
}

pub(super) async fn context(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: &NodeBindingOptionsGetQuery,
) -> ApiResult<Context> {
    if query.environment_new && query.environment_id.is_some() {
        return Err(invalid(
            "environment_id",
            "choose either an existing environment or a new one",
        ));
    }
    if !query.environment_new && (query.template_id.is_some() || query.host_id.is_some()) {
        return Err(invalid(
            "environment_new",
            "a template or host describes a new environment",
        ));
    }
    let id = ctx.id("id")?;
    let node = require(conn, principal, id, ctx.now).await?;
    let settings = resolved::load(conn, id).await.map_err(ApiError::internal)?;
    let environment_default =
        match setting::<String>(conn, Place::Node(id), keys::ENV_THREAD_DEFAULT)
            .await
            .map_err(ApiError::internal)?
            .as_deref()
        {
            Some("fresh") => EnvironmentDefault::Fresh,
            _ => EnvironmentDefault::Shared,
        };
    // This is the ordinary context decision, before its independently scoped choice
    // lists. A key never silently substitutes a different configured primary context.
    let existing = match query.environment_id {
        Some(environment) => Some(chosen_environment(conn, principal, environment, id).await?),
        None if !query.environment_new && environment_default == EnvironmentDefault::Shared => {
            let candidates =
                environment_candidates(conn, principal, id, Permission::EnvironmentUse, None)
                    .await?;
            match setting::<Id>(conn, Place::Node(id), keys::ENV_DEFAULT_ID)
                .await
                .map_err(ApiError::internal)?
            {
                Some(environment) if candidates.ids.contains(&environment) => {
                    let row = environment_queries::environments(conn, &[environment])
                        .await?
                        .into_iter()
                        .next();
                    if row.as_ref().is_some_and(|row| {
                        environment_reason(
                            row,
                            candidates.placeable.contains(&environment),
                            Permission::EnvironmentUse,
                        )
                        .is_none()
                    }) {
                        Some(chosen_environment(conn, principal, environment, id).await?)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        _ => None,
    };
    let (judged, sources) = match existing {
        Some(environment) => {
            let source = ObjectRef {
                kind: ObjectKind::Environment,
                id: environment.id,
            };
            let offers = binding::environment_offers(conn, &environment)
                .await
                .map_err(ApiError::internal)?;
            (Some((environment, offers)), vec![source])
        }
        None => match template_ground_source(conn, principal, &node, query.template_id).await? {
            Some((environment, offers, template)) => (
                Some((environment, offers)),
                vec![ObjectRef {
                    kind: ObjectKind::Template,
                    id: template,
                }],
            ),
            None => (None, Vec::new()),
        },
    };
    let mut orch_tools = OrchToolsOf::read(
        conn,
        id,
        judged
            .as_ref()
            .map(|(environment, _)| environment.id)
            .filter(|id| !id.is_nil()),
    )
    .await?;
    if let Some((environment, _)) = judged
        .as_ref()
        .filter(|(environment, _)| environment.id.is_nil())
    {
        orch_tools.environment_off = environment.config["exec.orch_tools"] == false;
    }
    let policy_scope = binding::allowed_set_at(conn, id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Context {
        node,
        settings,
        environment_default,
        judged,
        sources,
        orch_tools,
        policy_scope,
    })
}
