//! A reviewed thread switch carries one actual pool member and retains its original grant path.
use super::*;
use crate::binding::model_pool::{self, SetupBinding};
use crate::resource_choice::{Hop, ProviderSelection};
use orch_provider_contract::pools::{Kind, Leaf, Target as ResourceTarget};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Review {
    pub leaf: Leaf,
    pub path: Vec<Hop>,
}

pub(crate) fn target(request: &ForkRequest) -> Option<&ResourceTarget> {
    request.to.as_ref()?.target.as_ref()
}

pub(super) async fn select(
    conn: &mut Conn,
    access: &dyn crate::access::ResourceAccess,
    principal: &Principal,
    thread: &ThreadRow,
    request: &ForkRequest,
    confirmation: Option<Id>,
) -> Result<Option<(model_pool::Selected, Review)>, PlanError> {
    let Some(to) = &request.to else {
        return Ok(None);
    };
    let Some(target) = &to.target else {
        return Ok(None);
    };
    let Some(agent) = to.agent_id.or(thread.agent_id) else {
        return Err(PlanError::invalid(
            "to.target",
            "The thread needs a bound agent to review a pool switch.",
        ));
    };
    if !matches!(
        target,
        ResourceTarget::Pool {
            kind: Kind::Model | Kind::Provider,
            ..
        }
    ) || request.result != ForkResult::Replace
        || to.provider_id.is_some()
        || !matches!(
            request.environment,
            None | Some(ForkEnvironment::Same { same: true })
        )
        || request
            .directory
            .as_ref()
            .is_some_and(|v| *v != thread.workspace_dir)
        || request
            .topic_id
            .is_some_and(|topic| topic != thread.topic_id)
    {
        return Err(PlanError::invalid(
            "to.target",
            "Choose one model or provider pool for this thread in its current workspace.",
        ));
    }
    if !principal.caller().permits(Permission::PoolUse)
        || node_lacks(conn, principal, thread.node_id, Permission::ThreadManage)
            .await?
            .is_some()
    {
        return Ok(None);
    }
    let binding = SetupBinding {
        agent,
        target,
        model: to.model.as_deref(),
        effort: to.effort.as_deref(),
        arrangement: to.arrangement.map(|v| convert(&v)).transpose()?,
    };
    let Some((selected, choice)) =
        model_pool::switch_choice(conn, access, principal, thread, binding, confirmation).await?
    else {
        return Ok(None);
    };
    let leaf = choice.leaf.ok_or(PlanError::Expired)?;
    Ok(Some((
        selected,
        Review {
            leaf,
            path: choice.path,
        },
    )))
}

impl PreparedFork {
    pub(crate) async fn reserve_reviewed_pool(
        &self,
        conn: &mut Conn,
        access: &dyn crate::access::ResourceAccess,
        stored: &StoredPlan,
        preflight: Id,
    ) -> Result<(), PlanError> {
        if target(&self.request).is_none() {
            return Ok(());
        }
        let Some(review) = self.target.review.as_ref() else {
            return Err(PlanError::Expired);
        };
        if stored.review.as_ref() != Some(review)
            || self.target.provider.id != stored.plan.to.provider.id
            || self.target.model != stored.plan.to.model
            || self.target.agent.id != stored.plan.to.agent.id
            || self.target.directory != stored.plan.to.directory
        {
            return Err(PlanError::Expired);
        }
        let Some((_, accepted)) = select(
            conn,
            access,
            &self.principal,
            self.source(),
            &self.request,
            Some(preflight),
        )
        .await?
        else {
            return Err(PlanError::Expired);
        };
        if &accepted != review {
            return Err(PlanError::Expired);
        }
        Ok(())
    }
}

/// A job only carries a choice admitted for its own confirmed preflight, source segment and actor.
pub(crate) async fn retained(
    conn: &mut Conn,
    caller: &Caller,
    recipe: &crate::jobs::fork::Recipe,
) -> Result<Option<ProviderSelection>, EngineError> {
    let Some(target) = target(&recipe.request) else {
        return Ok(None);
    };
    let unavailable =
        || EngineError::unreadable("The reviewed pool selection is no longer available.");
    let segment = recipe.source_segment.ok_or_else(unavailable)?;
    let id = recipe.preflight_id;
    let row = orch_db::queries::pool_choices::get(conn, id)
        .await?
        .ok_or_else(unavailable)?;
    if !caller.permits(Permission::PoolUse)
        || recipe.batch_id.is_some()
        || recipe.new_thread.is_some()
        || row.actor_id != recipe.by
        || caller.user() != recipe.by
        || row.target != serde_json::to_value(target)?
        || row.source != model_pool::switch_source(recipe.source_thread, segment, id)
        || recipe.result != ForkResult::Replace
    {
        return Err(unavailable());
    }
    ProviderSelection::load(
        conn,
        id,
        recipe.source_thread,
        recipe.forkplan.to.provider.id,
        Some(&recipe.forkplan.to.model),
        orch_access_contract::permissions::UsagePurpose::Turn,
        caller,
    )
    .await
    .map(Some)
}

/// Publish the confirmation's selection with the new segment in the native replacement transaction.
pub(crate) async fn bind_segment(
    conn: &mut Conn,
    recipe: &crate::jobs::fork::Recipe,
) -> Result<(), EngineError> {
    let Some(target) = target(&recipe.request) else {
        return Ok(());
    };
    let previous = recipe
        .source_segment
        .ok_or_else(|| EngineError::unreadable("The reviewed source segment is gone."))?;
    let inserted = sqlx::query(
        "INSERT INTO segment_pool_bindings(segment_id,choice_id)
         SELECT s.id,c.id FROM segments s JOIN pool_choices c ON c.id=$2
         WHERE s.id=$1 AND s.thread_id=$3 AND s.agent_id=$4 AND s.provider_id=$5
           AND c.state='selected' AND c.actor_id=$6 AND c.source=$7 AND c.target=$8",
    )
    .bind(recipe.new_segment)
    .bind(recipe.preflight_id)
    .bind(recipe.source_thread)
    .bind(recipe.forkplan.to.agent.id)
    .bind(recipe.forkplan.to.provider.id)
    .bind(recipe.by)
    .bind(model_pool::switch_source(
        recipe.source_thread,
        previous,
        recipe.preflight_id,
    ))
    .bind(serde_json::to_value(target)?)
    .execute(conn)
    .await
    .map_err(orch_db::DbError::from)?;
    if inserted.rows_affected() != 1 {
        return Err(EngineError::unreadable(
            "The confirmed pool choice could not be attached to its segment.",
        ));
    }
    Ok(())
}
