//! Original input callers and observed restrictions shared by image reads and file delivery.
use super::{CallContext, Engine, EngineError, source};
use orch_authz::{Caller, CapturedCaller};
use orch_db::Conn;
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

pub(super) type Callers = Vec<(Option<Caller>, Option<Time>)>;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Limits {
    callers: Vec<CapturedCaller>,
    until: Time,
}

pub(super) struct Restored {
    limits: Limits,
    pub(super) origins: source::CallOrigins,
}

impl Limits {
    pub(super) fn deadline(&self) -> Time {
        self.until
    }

    pub(super) async fn capture(
        engine: &Engine,
        conn: &mut Conn,
        call: &CallContext,
    ) -> Result<Self, EngineError> {
        let origins = source::original_callers(
            conn,
            call,
            engine.inner.clock.as_ref(),
            engine.inner.access.as_ref(),
        )
        .await?;
        let mut until = source::capability_deadline(conn, call).await?;
        for caller in origins.callers() {
            if let Some(end) = caller.grant().and_then(orch_authz::TokenGrant::expires_at) {
                until = until.min(end);
            }
        }
        Ok(Self {
            callers: origins
                .callers()
                .iter()
                .map(CapturedCaller::capture)
                .collect(),
            until,
        })
    }

    pub(super) async fn restore(
        &self,
        conn: &mut Conn,
        call: &CallContext,
        command: Id,
        deadline: Time,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Restored, EngineError> {
        let observed:Option<Json>=sqlx::query_scalar("SELECT plan #> ARRAY['image_delivery_limits',$2::text] FROM thread_tool_calls WHERE answer_id=$1 AND NOT retired")
            .bind(crate::ids::tool_result(call.thread,&call.call_ref)).bind(command.to_string()).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?.flatten();
        let mut limits = observed
            .map(serde_json::from_value::<Self>)
            .transpose()?
            .unwrap_or_else(|| self.clone());
        if limits.callers.len() != self.callers.len() {
            return Err(EngineError::ToolCallIdentity);
        }
        for (old, initial) in limits.callers.iter_mut().zip(&self.callers) {
            *old = old.intersect(initial).map_err(EngineError::unreadable)?;
        }
        limits.until = limits.until.min(self.until).min(deadline);
        let origins = Box::pin(source::original_callers_with_limits(
            conn,
            call,
            clock,
            access,
            &limits.callers,
        ))
        .await?;
        Ok(Restored { limits, origins })
    }
}

impl Restored {
    /// Call only after this native class has checked its physical subject and any result Read.
    pub(super) async fn finish(
        mut self,
        conn: &mut Conn,
        call: &CallContext,
        command: Id,
        clock: &dyn Clock,
    ) -> Result<Option<Callers>, EngineError> {
        let mut current = Vec::with_capacity(self.origins.callers().len());
        for (saved, caller) in self.limits.callers.iter_mut().zip(self.origins.callers()) {
            *saved = saved
                .narrowed_with(caller)
                .map_err(EngineError::unreadable)?;
            if let Some(end) = caller.grant().and_then(orch_authz::TokenGrant::expires_at) {
                self.limits.until = self.limits.until.min(end);
            }
            current.push((Some(caller.clone()), Some(self.limits.until)));
        }
        if current.is_empty()
            || clock.time() >= self.limits.until
            || current.iter().any(|(caller, _)| {
                caller
                    .as_ref()
                    .is_none_or(|caller| !caller.key_deadline_allows(clock))
            })
        {
            return Ok(None);
        }
        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,'{image_delivery_limits}',COALESCE(plan->'image_delivery_limits','{}'::jsonb) || jsonb_build_object($2::text,$3::jsonb)) WHERE answer_id=$1 AND NOT retired")
            .bind(crate::ids::tool_result(call.thread,&call.call_ref)).bind(command.to_string()).bind(serde_json::to_value(&self.limits)?).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
        for (_, until) in &mut current {
            *until = Some(self.limits.until);
        }
        Ok(Some(current))
    }
}
