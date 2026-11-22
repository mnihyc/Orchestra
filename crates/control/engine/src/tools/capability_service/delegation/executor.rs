//! The common execution port owns no ambient permission; a real producer binds every request.
use super::*;
use async_trait::async_trait;
use orch_provider_contract::{
    SourceError, SourceErrorCode,
    capabilities::{
        CallSource, CapabilityCall, CapabilityError, CapabilityExecutor, CapabilityResult,
        ResolvedRoute,
    },
};

/// A source-bound adapter; public call identities and JSON cannot construct its authority.
pub(crate) struct Executor {
    engine: Engine,
    call: CallContext,
    owner: source::Source,
    selected: selection::Selection,
    attempt: Id,
    recovery: Option<recovery_binding::Binding>,
    request: CapabilityCall,
}

impl Executor {
    pub(crate) async fn thread_tool(
        engine: &Engine,
        call: &CallContext,
        capability: &str,
        sources: &BuiltinSources,
        selected: selection::Selection,
    ) -> Result<Self, EngineError> {
        let owner = source::Source::captured()?;
        if !accepted::is_prepared() {
            return Err(EngineError::StaleToolSource);
        }
        let mut conn = engine.db().acquire().await?;
        let origins = source::original_callers(
            &mut conn,
            call,
            engine.inner.clock.as_ref(),
            engine.inner.access.as_ref(),
        )
        .await?;
        let declarations = sources.capabilities().declarations();
        let definition = declarations
            .capabilities
            .get(capability)
            .ok_or(EngineError::StaleToolSource)?;
        let route = declarations
            .routes
            .get(&selected.reference.route)
            .filter(|route| {
                route.executor.as_deref() == Some("program-session")
                    && route.orch_tool.as_deref() == Some(call.tool.name())
            })
            .ok_or(EngineError::StaleToolSource)?;
        let mode = definition
            .modes
            .get("service")
            .filter(|mode| mode.routes.contains(&selected.reference.route))
            .ok_or(EngineError::StaleToolSource)?;
        let request = CapabilityCall {
            source: CallSource {
                request_id: owner.answer(),
                thread_id: Some(call.thread),
                turn_id: Some(call.turn),
                node_id: None,
                for_user_id: origins.attribution().user_id,
            },
            capability: capability.into(),
            route: ResolvedRoute {
                id: selected.reference.route.clone(),
                kind: route.kind.clone(),
                executor: route.executor.clone(),
                execution: route.execution.clone(),
                offering: selected.reference.offering.clone(),
                backend: Some(selected.reference.clone()),
                orch_tool: route.orch_tool.clone(),
                disable_native: route.disable_native,
                native_value: mode.native_value.clone(),
            },
            item: definition.item.clone(),
            ledger_unit: definition.ledger_unit.clone(),
            input: accepted::input(call, capability)?,
        };
        let root = source::resource_id("provider-service", owner.answer());
        Ok(Self {
            engine: engine.clone(),
            call: call.clone(),
            owner,
            selected,
            attempt: recovery_binding::command(root)?,
            recovery: recovery_binding::binding(),
            request,
        })
    }

    pub(crate) fn request(&self) -> CapabilityCall {
        self.request.clone()
    }
}

fn native_error(error: EngineError) -> CapabilityError {
    let code = match &error {
        EngineError::StaleToolSource
        | EngineError::ToolCallIdentity
        | EngineError::BeforeEffect(_) => SourceErrorCode::Refused,
        _ => SourceErrorCode::Unavailable,
    };
    let mut portable = SourceError::new(code, "The native program could not complete this work.");
    if let EngineError::Capability(failure) = &error {
        portable.message = failure.message.clone();
        portable.retryable = failure.retryable;
    }
    CapabilityError::native(portable, error)
}

#[async_trait]
impl CapabilityExecutor for Executor {
    fn support(&self) -> Vec<ExecutionSupport> {
        super::execution_support()
    }

    async fn execute(&self, request: CapabilityCall) -> Result<CapabilityResult, CapabilityError> {
        let root = source::resource_id("provider-service", self.owner.answer());
        if request != self.request
            || source::Source::captured().map_err(native_error)? != self.owner
            || !accepted::is_prepared()
            || accepted::input(&self.call, &request.capability).map_err(native_error)?
                != request.input
            || recovery_binding::command(root).map_err(native_error)? != self.attempt
            || recovery_binding::binding() != self.recovery
        {
            return Err(native_error(EngineError::ToolCallIdentity));
        }
        let required = request
            .route
            .execution
            .as_ref()
            .ok_or_else(|| native_error(EngineError::StaleToolSource))?;
        super::run::execute(
            &self.engine,
            &self.call,
            &request.capability,
            self.selected.clone(),
            required,
        )
        .await
        .map_err(native_error)
    }
}

/// The actual native producer receives its original typed refusal, including existing recovery facts.
pub(crate) fn engine_error(error: CapabilityError) -> EngineError {
    let (portable, native) = error.into_parts();
    match native {
        Some(native) => match native.downcast::<EngineError>() {
            Ok(error) => *error,
            Err(error) => EngineError::unreadable(error),
        },
        None => EngineError::unreadable(portable),
    }
}
