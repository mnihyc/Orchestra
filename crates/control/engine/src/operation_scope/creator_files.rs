//! Current Environment effects of the two thread-creating operations' source preparation.
//! This binds facts only; the caller admits the complete original operation afterwards.

use super::{Prepared, compatible_selection, facets, targets};
use crate::EngineError;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::{Caller, CapturedOperation};
use orch_db::{Conn, queries::hierarchy};
use orch_types::{Id, Time};

const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

fn slot(plan: &Prepared, role: &'static str, action: KeyAction) -> Result<usize, EngineError> {
    let mut found = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.target == TargetResolver::ResolvedCollection(role));
    let Some((index, facet)) = found.next() else {
        return Err(EngineError::unreadable(format!(
            "the creator file collection {role} is not declared"
        )));
    };
    if found.next().is_some()
        || facet.action != action
        || facet.usage != FacetUsage::Required
        || facet.resources != [ENVIRONMENT]
    {
        return Err(EngineError::unreadable(format!(
            "the creator file collection {role} has invalid metadata"
        )));
    }
    Ok(index)
}

/// Bind the pending Prepare/Worktree Environment and a worktree's same-Environment
/// source read. Known absence is an explicit empty collection, never a missing slot.
pub async fn bind(
    conn: &mut Conn,
    plan: &mut Prepared,
    environment: Id,
    pending: bool,
    worktree: bool,
) -> Result<(), EngineError> {
    if !matches!(
        plan.variant().operation.as_str(),
        "ThreadCreate"
            | "TopicCreate.with_thread"
            | "TopicInputScheduleCreate"
            | "InputScheduleUpdate.definition"
    ) {
        return Err(EngineError::unreadable(
            "creator file effects need their original thread-creating operation",
        ));
    }
    let effects = [
        (
            slot(plan, "creator_file_environments", KeyAction::Manage)?,
            pending,
        ),
        (
            slot(plan, "creator_file_sources", KeyAction::Read)?,
            worktree,
        ),
    ];
    let target = if pending || worktree {
        vec![
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: environment,
                },
            )
            .await
            .map_err(|error| match error {
                targets::TargetError::Missing => {
                    EngineError::Authz(orch_authz::Error::NotFound(environment))
                }
                targets::TargetError::Db(error) => EngineError::Db(error),
                targets::TargetError::Authz(error) => EngineError::Authz(error),
            })?,
        ]
    } else {
        Vec::new()
    };
    for (index, present) in effects {
        if present {
            plan.bind(index, ENVIRONMENT, &target)
        } else {
            plan.bind_empty(index)
        }
        .map_err(EngineError::unreadable)?;
    }
    Ok(())
}

/// Resolve the original creator's current parent effects from its actual persisted thread.
/// File preparation, provisioning and initial copies share these same identities; none may
/// substitute an old accepted path or another operation's admission.
pub fn required_facet(
    plan: &Prepared,
    resolver: TargetResolver,
    action: KeyAction,
    kind: KeyTargetKind,
    target: &orch_access_contract::key_scope::KeyResolvedTarget,
) -> Result<Vec<orch_access_contract::key_scope::KeyFacet>, EngineError> {
    let mut matches = plan.variant().facets.iter().filter(|facet| {
        facet.target == resolver
            && facet.usage == FacetUsage::Required
            && facet.action == action
            && facet.resources == [kind]
    });
    let metadata = matches
        .next()
        .ok_or_else(|| EngineError::unreadable("the continuing creator effect is not declared"))?;
    if matches.next().is_some() {
        return Err(EngineError::unreadable(
            "the continuing creator effect is ambiguous",
        ));
    }
    facets(
        &plan.variant().operation,
        metadata,
        kind,
        std::slice::from_ref(target),
    )
    .map_err(EngineError::unreadable)
}

pub async fn parent_facets(
    conn: &mut Conn,
    selection: &str,
    thread: Id,
    environment: Id,
) -> Result<Option<Vec<orch_access_contract::key_scope::KeyFacet>>, EngineError> {
    if !matches!(selection, "ThreadCreate" | "TopicCreate.with_thread") {
        return Ok(None);
    }
    let plan = Prepared::for_selection(selection).map_err(EngineError::unreadable)?;
    let Some(row) = orch_db::queries::threads::thread(conn, thread)
        .await?
        .filter(|row| row.environment_id == Some(environment))
    else {
        return Ok(None);
    };
    let topic = targets::node(conn, row.topic_id)
        .await
        .map_err(EngineError::unreadable)?;
    if topic.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Ok(None);
    }
    let mut required = Vec::new();
    if selection == "ThreadCreate" {
        required.extend(required_facet(
            &plan,
            TargetResolver::PermissionObject,
            KeyAction::Write,
            KeyTargetKind::Object(ObjectKind::Thread),
            &topic,
        )?);
    } else {
        let nodes = hierarchy::with_ancestors(conn, &[row.topic_id]).await?;
        let Some(parent) = nodes
            .iter()
            .find(|node| node.id == row.topic_id)
            .and_then(|node| node.parent_id)
        else {
            return Ok(None);
        };
        let parent = targets::node(conn, parent)
            .await
            .map_err(EngineError::unreadable)?;
        if parent.object.kind != KeyTargetKind::Object(ObjectKind::Project) {
            return Ok(None);
        }
        required.extend(required_facet(
            &plan,
            TargetResolver::PermissionObject,
            KeyAction::Write,
            KeyTargetKind::Object(ObjectKind::Topic),
            &parent,
        )?);
        required.extend(required_facet(
            &plan,
            TargetResolver::Resolved("first_thread_destination"),
            KeyAction::Write,
            KeyTargetKind::Object(ObjectKind::Thread),
            &topic,
        )?);
    }
    Ok(Some(required))
}

/// Reauthorize only the creator's continuing file effects. Creation, delegated gifts and the
/// first input have already been accepted; they are not performed again by this source job.
/// The original selection and current ancestry remain part of every continuing effect.
pub async fn restore(
    conn: &mut Conn,
    origin: &CapturedOperation,
    thread: Id,
    environment: Id,
    worktree: bool,
    now: Time,
) -> Result<Option<Caller>, EngineError> {
    let Some(caller) = origin.restore(conn, now).await? else {
        return Ok(None);
    };
    let plan = Prepared::for_selection(origin.operation()).map_err(EngineError::unreadable)?;
    if !compatible_selection(&caller, plan.variant()) {
        return Ok(None);
    }
    let Some(mut required) = parent_facets(conn, origin.operation(), thread, environment).await?
    else {
        return Ok(None);
    };
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    required.extend(required_facet(
        &plan,
        TargetResolver::ResolvedCollection("creator_file_environments"),
        KeyAction::Manage,
        ENVIRONMENT,
        &target,
    )?);
    if worktree {
        required.extend(required_facet(
            &plan,
            TargetResolver::ResolvedCollection("creator_file_sources"),
            KeyAction::Read,
            ENVIRONMENT,
            &target,
        )?);
    }
    Ok(caller
        .admit_facets(&required)
        .ok()
        .map(|admitted| admitted.into_caller()))
}
