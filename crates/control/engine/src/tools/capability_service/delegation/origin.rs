//! A producer retains its real source; runtime identities and JSON never create that authority.
use super::*;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "producer", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Producer {
    Summary {
        source: crate::summaries::ProgramSource,
        limits: Vec<CapturedCaller>,
    },
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
    pub fn capture_summary(
        source: crate::summaries::ProgramSource,
        limits: Vec<CapturedCaller>,
    ) -> Self {
        Self::Summary { source, limits }
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                call,
                ..
            } => owner.scope(source::check_call(conn, call)).await,
            Self::Summary { source, .. } => source.lock_source(conn).await,
        }
    }
    pub fn summary(&self) -> Option<&crate::summaries::ProgramSource> {
        match self {
            Self::Summary { source, .. } => Some(source),
            Self::ThreadTool { .. } => None,
        }
    }
    pub fn program_origin(&self) -> capability_work::ProgramOrigin {
        match self {
            Self::ThreadTool { source, .. } => capability_work::ProgramOrigin::Tool {
                answer: source.answer(),
            },
            Self::Summary { source, .. } => capability_work::ProgramOrigin::Summary {
                call: source.call_id(),
            },
        }
    }
    pub fn matches_journal(&self, work: &capability_work::ProgramWork) -> bool {
        match self {
            Self::ThreadTool { source, .. } => {
                work.source_answer == Some(source.answer()) && work.source_summary_call.is_none()
            }
            Self::Summary { source, .. } => {
                work.source_summary_call == Some(source.call_id()) && work.source_answer.is_none()
            }
        }
    }
    pub fn request(&self) -> Id {
        match self {
            Self::ThreadTool { source, .. } => source.answer(),
            Self::Summary { source, .. } => source.request_id(),
        }
    }
    pub fn node(&self) -> Id {
        match self {
            Self::ThreadTool { call, .. } => call.thread,
            Self::Summary { source, .. } => source.node(),
        }
    }
    pub fn thread(&self) -> Option<Id> {
        match self {
            Self::ThreadTool { call, .. } => Some(call.thread),
            Self::Summary { source, .. } => source.thread(),
        }
    }
    pub fn backend(&self) -> &orch_provider_contract::capabilities::BackendRef {
        match self {
            Self::ThreadTool { selected, .. } => &selected.reference,
            Self::Summary { source, .. } => source.backend(),
        }
    }
    pub fn locator(&self, slot: &str) -> Json {
        match self {
            Self::ThreadTool { source, .. } => json!({"source":source,"slot":slot}),
            Self::Summary { source, .. } => {
                json!({"summary_call":source.call_id(),"request":source.request_id()})
            }
        }
    }
    pub fn matches_tool(&self, selected: &selection::Selection) -> Result<bool, EngineError> {
        match self {
            Self::ThreadTool {
                source: owner,
                selected: choice,
                ..
            } => Ok(*owner == source::Source::captured()? && choice == selected),
            Self::Summary { .. } => Ok(false),
        }
    }
    pub async fn selection_current(
        &self,
        conn: &mut Conn,
        capability: &str,
        attempt: Id,
    ) -> Result<bool, EngineError> {
        match self {
            Self::Summary { source, .. } => {
                Ok(capability == "summary" && source.call_id() == attempt)
            }
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
            Self::Summary { source, limits } => {
                for limit in limits {
                    if limit.restore_current(conn, clock).await?.is_none() {
                        return Ok(false);
                    }
                }
                Ok(source.current_callers(conn, clock).await?.is_some())
            }
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
            Self::Summary { source, limits } => {
                let Some(current) = source.current_callers(conn, clock).await? else {
                    return Ok(None);
                };
                let Some(callers) = summary_callers(conn, clock, current, limits).await? else {
                    return Ok(None);
                };
                Ok(Some(Callers {
                    callers,
                    actor: source.actor()?,
                    person: source.person(),
                }))
            }
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
            Self::Summary { source, limits } => {
                let current = source
                    .current_callers(conn, clock)
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                let callers = summary_callers(conn, clock, current, limits)
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                Ok(Callers {
                    callers,
                    actor: source.actor()?,
                    person: source.person(),
                })
            }
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
            Self::Summary { source, .. } => {
                source
                    .current_callers(conn, engine.inner.clock.as_ref())
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                Ok(())
            }
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
            Self::Summary { source, .. } => {
                source
                    .result(
                        engine,
                        &json!({"command":receipt.command,"result":receipt.result}),
                    )
                    .await
            }
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
            Self::Summary { source, .. } => {
                source.result(engine, &json!({"failure":failure})).await
            }
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

async fn summary_callers(
    conn: &mut Conn,
    clock: &dyn Clock,
    current: Vec<Caller>,
    limits: &[CapturedCaller],
) -> Result<Option<Vec<Caller>>, EngineError> {
    if current.len() != limits.len() {
        return Ok(None);
    }
    let mut callers = Vec::with_capacity(current.len());
    for (current, limit) in current.into_iter().zip(limits) {
        let Some(saved) = limit.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        let next = if current
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some())
        {
            saved.narrow_current_admission(&current)
        } else {
            current.intersect_restrictions(&saved)
        };
        let Ok(next) = next else {
            return Ok(None);
        };
        callers.push(next);
    }
    Ok(Some(callers))
}
