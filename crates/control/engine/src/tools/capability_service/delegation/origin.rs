//! A producer retains its real source; runtime identities and JSON never create that authority.
use super::*;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "producer", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Producer {
    ThreadTool {
        source: source::Source,
        call: CallContext,
        selected: selection::Selection,
        recovery: Option<recovery_binding::Binding>,
        limits: Vec<CapturedCaller>,
    },
}

pub(super) struct Callers {
    pub callers: Vec<Caller>,
    pub actor: Id,
    pub person: Option<Id>,
}

impl Producer {
    pub fn capture_tool(
        call: &CallContext,
        selected: &selection::Selection,
        limits: Vec<CapturedCaller>,
    ) -> Result<Self, EngineError> {
        if !accepted::is_prepared() {
            return Err(EngineError::StaleToolSource);
        }
        Ok(Self::ThreadTool {
            source: source::Source::captured()?,
            call: call.clone(),
            selected: selected.clone(),
            recovery: recovery_binding::binding(),
            limits,
        })
    }
    pub fn request(&self) -> Id {
        match self {
            Self::ThreadTool { source, .. } => source.answer(),
        }
    }
    pub fn node(&self) -> Id {
        match self {
            Self::ThreadTool { call, .. } => call.thread,
        }
    }
    pub fn thread(&self) -> Option<Id> {
        match self {
            Self::ThreadTool { call, .. } => Some(call.thread),
        }
    }
    pub fn backend(&self) -> &orch_provider_contract::capabilities::BackendRef {
        match self {
            Self::ThreadTool { selected, .. } => &selected.reference,
        }
    }
    pub fn limits(&self) -> &[CapturedCaller] {
        match self {
            Self::ThreadTool { limits, .. } => limits,
        }
    }
    pub fn locator(&self, slot: &str) -> Json {
        match self {
            Self::ThreadTool { source, .. } => json!({"source":source,"slot":slot}),
        }
    }
    pub fn matches_tool(&self, selected: &selection::Selection) -> Result<bool, EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                selected: choice,
                ..
            } => Ok(*owner == source::Source::captured()? && choice == selected),
        }
    }
    pub async fn selection_current(
        &self,
        conn: &mut Conn,
        capability: &str,
        attempt: Id,
    ) -> Result<bool, EngineError> {
        match self {
            Self::ThreadTool {
                call,
                selected,
                recovery,
                ..
            } => {
                if !selected.is_current(conn, call.thread).await? {
                    return Ok(false);
                }
                if let Some(binding) = recovery
                    && !binding
                        .current_call(conn, call, capability, Some(selected), attempt)
                        .await?
                {
                    return Ok(false);
                }
                Ok(true)
            }
        }
    }
    pub async fn current_act(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<bool, EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                call,
                limits,
                ..
            } => {
                let live: bool = sqlx::query_scalar(
                    "SELECT NOT(completed OR retired) FROM thread_tool_calls WHERE answer_id=$1",
                )
                .bind(owner.answer())
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
                .unwrap_or(false);
                if !live {
                    return Ok(false);
                }
                for limit in limits {
                    if limit.restore_current(conn, clock).await?.is_none() {
                        return Ok(false);
                    }
                }
                match owner.scope(source::check_call(conn, call)).await {
                    Ok(()) => Ok(true),
                    Err(EngineError::StaleToolSource | EngineError::ToolCallIdentity) => Ok(false),
                    Err(error) => Err(error),
                }
            }
        }
    }
    pub async fn callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Callers>, EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                call,
                limits,
                ..
            } => {
                let origins = match owner
                    .scope(source::original_callers_with_limits(
                        conn, call, clock, access, limits,
                    ))
                    .await
                {
                    Ok(origins) => origins,
                    Err(EngineError::StaleToolSource | EngineError::ToolCallIdentity) => {
                        return Ok(None);
                    }
                    Err(error) => return Err(error),
                };
                Ok(Some(Callers {
                    callers: origins.callers().to_vec(),
                    actor: origins.actor(),
                    person: origins.attribution().user_id,
                }))
            }
        }
    }
    pub async fn input_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Callers, EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                call,
                limits,
                ..
            } => {
                let origins = owner
                    .scope(source::input_callers_with_limits(conn, call, clock, limits))
                    .await?;
                Ok(Callers {
                    callers: origins.callers().to_vec(),
                    actor: origins.actor(),
                    person: origins.attribution().user_id,
                })
            }
        }
    }
    pub async fn before_read(&self, engine: &Engine, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                call,
                ..
            } => {
                owner.scope(source::check_call(conn, call)).await?;
                owner
                    .scope(source::original_callers(
                        conn,
                        call,
                        engine.inner.clock.as_ref(),
                        engine.inner.access.as_ref(),
                    ))
                    .await?;
                Ok(())
            }
        }
    }
    pub async fn publish(
        &self,
        engine: &Engine,
        parent: &ParentLink,
        record: &Record,
        receipt: &codecs::Receipt,
    ) -> Result<(), EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                call,
                ..
            } => {
                owner
                    .scope(thread_tool::publish(engine, call, parent, record, receipt))
                    .await
            }
        }
    }
    pub async fn retain_failure(
        &self,
        engine: &Engine,
        attempt: Id,
        failure: Json,
    ) -> Result<(), EngineError> {
        match self {
            Self::ThreadTool { source: owner, .. } => {
                owner
                    .scope(source::value(
                        engine,
                        &format!("delegated_failure:{attempt}"),
                        failure,
                    ))
                    .await?;
                Ok(())
            }
        }
    }
}
