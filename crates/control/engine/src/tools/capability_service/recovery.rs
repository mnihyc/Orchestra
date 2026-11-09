//! Rule decisions stay attached to the actual admitted managed-tool request.
use super::*;
use orch_provider_contract::calls::{Purpose, Step, Until};

async fn intent(
    engine: &Engine,
    conn: &mut Conn,
    sources: &BuiltinSources,
    call: &CallContext,
    capability: &str,
) -> Result<selection::Selection, EngineError> {
    let origins = source::original_callers(
        conn,
        call,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?;
    let selected = backend(conn, call.thread, sources, capability).await?;
    if !selected.is_current(conn, call.thread).await?
        || selected
            .mode_setting
            .strip_prefix("agent.params.")
            .is_some_and(|member| {
                origins
                    .attribution()
                    .params
                    .get(member)
                    .and_then(Json::as_str)
                    != Some("service")
            })
    {
        #[cfg(test)]
        if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
            eprintln!("service_recovery original intent is no longer selected");
        }
        return Err(EngineError::StaleToolSource);
    }
    Ok(selected)
}

fn failure(command: Option<&link::CommandRow>) -> Json {
    let Some(command) = command else {
        return serde_json::json!({"code":"unavailable","message":"The selected service is unavailable","retryable":true});
    };
    if let Some(failure) = command
        .error
        .as_ref()
        .and_then(|error| error.pointer("/details/failure"))
        .filter(|failure| failure.is_object())
    {
        let mut failure = failure.clone();
        failure["message"] =
            serde_json::json!("The selected service could not complete the request");
        if failure.get("retryable").is_none() {
            failure["retryable"] = serde_json::json!(true);
        }
        return failure;
    }
    let code = command
        .error
        .as_ref()
        .and_then(|error| error.get("code"))
        .and_then(Json::as_str)
        .unwrap_or("harness_error");
    let code = if code == "unsupported" {
        "unavailable"
    } else {
        "harness_error"
    };
    serde_json::json!({"code":code,"message":"The selected service could not complete the request","retryable":true})
}

fn candidate_selection(
    sources: &BuiltinSources,
    capability: &str,
    root: &selection::Selection,
    candidate: &orch_provider_contract::calls::Candidate,
) -> Option<selection::Selection> {
    if candidate.agent_id.is_some()
        || candidate.dimensions.is_some()
        || !(candidate.params.is_null()
            || candidate
                .params
                .as_object()
                .is_some_and(|params| params.is_empty()))
    {
        return None;
    }
    let declarations = sources.capabilities().declarations();
    let reference = match &candidate.route {
        Some(route) => route.clone(),
        None => {
            let route = declarations
                .capabilities
                .get(capability)?
                .modes
                .get("service")?
                .routes
                .iter()
                .find(|route| {
                    declarations
                        .routes
                        .get(*route)
                        .is_some_and(|route| route.executor.as_deref() == Some("model-backed"))
                })?;
            orch_provider_contract::capabilities::BackendRef {
                route: route.clone(),
                offering: Some(orch_provider_contract::capabilities::OfferingRef {
                    provider: candidate.provider_id,
                    offering: candidate.model.clone(),
                }),
                ..Default::default()
            }
        }
    };
    let selected = selection::from_value(
        declarations,
        capability,
        serde_json::to_value(reference).ok()?,
    )?;
    if !matches!(selected.executor.as_str(), "api-backed" | "model-backed")
        || selected.offering()?.provider != candidate.provider_id
    {
        return None;
    }
    Some(selection::Selection {
        reference: selected.reference,
        executor: selected.executor,
        ..root.clone()
    })
}

fn pause_reason(
    row: &crate::calls::Recovery,
    deadline: Time,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'static str> {
    let next = row
        .action
        .as_ref()
        .and_then(|action| action.get("step"))
        .cloned()
        .and_then(|step| serde_json::from_value::<Step>(step).ok());
    match next {
        Some(Step::Wait {
            until: Until::Person,
        }) => Some("wait_for_person"),
        Some(Step::Wait {
            until: Until::Reset,
        }) if row.due_at.is_none() => Some("reset_unreported"),
        Some(Step::Wait {
            until: Until::Reset,
        }) if row.due_at.is_some_and(|at| at >= deadline.get()) => Some("reset_after_deadline"),
        _ if matches!(row.state.as_str(), "stopped" | "waiting") => Some("recovery_needs_person"),
        _ if row.due_at.is_some_and(|at| at >= deadline.get()) || now >= deadline.get() => {
            Some("native_deadline")
        }
        _ => None,
    }
}

pub(super) async fn execute(
    engine: &Engine,
    sources: &BuiltinSources,
    call: &CallContext,
    capability: &str,
) -> Result<Json, EngineError> {
    let purpose: Purpose = serde_json::from_value(serde_json::json!(capability))?;
    let (configured, deadline) = {
        let mut conn = engine.db().acquire().await?;
        let selected = intent(engine, &mut conn, sources, call, capability).await?;
        if let Some(paused) = crate::calls::tool_pause::active(
            &mut conn,
            call.thread,
            purpose,
            &selected,
            engine.inner.clock.now(),
        )
        .await?
        {
            return Ok(paused.result());
        }
        let original = source::original_callers(
            &mut conn,
            call,
            engine.inner.clock.as_ref(),
            engine.inner.access.as_ref(),
        )
        .await?;
        let mut deadline = source::capability_deadline(&mut conn, call).await?;
        for caller in original.callers() {
            if let Some(until) = caller.grant().and_then(orch_authz::TokenGrant::expires_at) {
                deadline = deadline.min(until);
            }
        }
        if engine.inner.clock.time() >= deadline {
            return Err(EngineError::StaleToolSource);
        }
        (selected, deadline)
    };
    let root = source::resource_id(
        "provider-service",
        crate::ids::tool_result(call.thread, &call.call_ref),
    );
    let configured_offering = configured.offering().ok_or(EngineError::StaleToolSource)?;
    let configured_model = if configured.executor == "model-backed" {
        configured_offering.offering.as_str()
    } else {
        ""
    };
    let request = crate::calls::ServiceCall {
        node: call.thread,
        purpose,
        root,
        workflow: None,
        task: None,
        provider: configured_offering.provider,
        model: configured_model,
        dimensions: None,
    };
    loop {
        let planned = {
            let mut tx = engine.db().begin().await?;
            let _ = threads::lock(&mut tx, call.thread).await?;
            if intent(engine, &mut tx, sources, call, capability).await? != configured {
                return Err(EngineError::StaleToolSource);
            }
            let plan =
                crate::calls::plan_service(&mut tx, &request, engine.inner.clock.now()).await?;
            match plan {
                crate::calls::ServicePlan::Paused => {
                    let row = crate::calls::read_root(&mut tx, call.thread, purpose, root)
                        .await?
                        .ok_or(EngineError::StaleToolSource)?;
                    if let Some(reason) = pause_reason(&row, deadline, engine.inner.clock.now()) {
                        let paused = crate::calls::tool_pause::retain(
                            &mut tx,
                            &row,
                            &configured,
                            reason,
                            engine.inner.clock.now(),
                        )
                        .await?;
                        tx.commit().await.map_err(orch_db::DbError::from)?;
                        return Ok(paused.result());
                    }
                    let due = row.due_at.ok_or(EngineError::StaleToolSource)?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    let delay = due
                        .signed_duration_since(engine.inner.clock.now())
                        .to_std()
                        .unwrap_or_default();
                    tokio::time::sleep(delay).await;
                    continue;
                }
                crate::calls::ServicePlan::Ready {
                    call: command,
                    candidate,
                } => {
                    let selected = match candidate.as_ref() {
                        Some(candidate) => {
                            match candidate_selection(sources, capability, &configured, candidate) {
                                Some(selected) => selected,
                                None => {
                                    let row = crate::calls::read_root(
                                        &mut tx,
                                        call.thread,
                                        purpose,
                                        root,
                                    )
                                    .await?
                                    .ok_or(EngineError::StaleToolSource)?;
                                    crate::calls::skip_step(
                                        &mut tx,
                                        &row,
                                        "candidate_parameters",
                                        engine.inner.clock.now(),
                                    )
                                    .await?;
                                    tx.commit().await.map_err(orch_db::DbError::from)?;
                                    continue;
                                }
                            }
                        }
                        None => configured.clone(),
                    };
                    let binding = if let Some(mut row) =
                        crate::calls::read_root(&mut tx, call.thread, purpose, root).await?
                    {
                        row.context["configured_capability_route"] =
                            serde_json::json!(configured.reference);
                        if let Some(candidate) = &candidate {
                            row.context["candidate"] = serde_json::json!(candidate);
                        }
                        sqlx::query("UPDATE model_call_recovery SET context=$4 WHERE node_id=$1 AND purpose=$2 AND call_key=$3 AND state='running'")
                            .bind(call.thread).bind(purpose.as_str()).bind(root).bind(&row.context).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
                        Some(recovery_binding::Binding::capture(
                            root,
                            &configured,
                            &selected,
                            &row,
                            candidate.as_deref().cloned(),
                        )?)
                    } else {
                        None
                    };
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    recovery_binding::Attempt {
                        root,
                        command,
                        selected,
                        binding,
                    }
                }
            }
        };
        if engine.inner.clock.time() >= deadline {
            return Err(EngineError::StaleToolSource);
        }
        let actual = planned.selected.clone();
        let command_id = planned.command;
        let chosen = planned.binding.is_some() && actual.reference != configured.reference;
        let result = planned
            .scope(engine.execute_capability_once(sources, call, capability))
            .await;
        match result {
            Ok(result) => {
                let mut tx = engine.db().begin().await?;
                let _ = threads::lock(&mut tx, call.thread).await?;
                source::check_call(&mut tx, call).await?;
                let row = crate::calls::read_root(&mut tx, call.thread, purpose, root).await?;
                crate::calls::finish_service(&mut tx, call.thread, purpose, root).await?;
                if let Some(row) = row {
                    let turn = threads::turn(&mut tx, call.turn)
                        .await?
                        .ok_or(EngineError::StaleToolSource)?;
                    let mut unit = crate::unit::Unit::new(&mut tx, engine.inner.clock.now());
                    let action = if actual.reference != configured.reference {
                        "failover"
                    } else {
                        "retry"
                    };
                    let reference = format!("capability-recovery:{root}:{command_id}:completed");
                    crate::ingest::add_marker(&mut unit,call.thread,turn.segment_id,Some(call.turn),&serde_json::json!({
                        "marker":"call_recovery","data":{"action":action,"rule":row.rule["id"],"kind":row.failure["code"],
                            "attempt":row.action.as_ref().and_then(|action|action["attempt"].as_u64()).unwrap_or(1),
                            "automatic":true,"input_id":turn.input_id}
                    }),&reference).await?;
                    let finished = unit.finish().await?;
                    engine.commit(tx, finished).await?;
                } else {
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                }
                return Ok(result);
            }
            Err(original) => {
                let mut tx = engine.db().begin().await?;
                let _ = threads::lock(&mut tx, call.thread).await?;
                if intent(engine, &mut tx, sources, call, capability).await? != configured {
                    return Err(original);
                }
                let command = link::command(&mut tx, command_id).await?;
                if command.is_none() && engine.inner.clock.time() >= deadline {
                    return Err(original);
                }
                if command.as_ref().is_some_and(|command| {
                    matches!(command.state.as_str(), "queued" | "sent" | "accepted")
                }) {
                    return Err(original);
                }
                if chosen && command.is_none() {
                    let row = crate::calls::read_root(&mut tx, call.thread, purpose, root)
                        .await?
                        .ok_or(EngineError::StaleToolSource)?;
                    crate::calls::skip_step(
                        &mut tx,
                        &row,
                        "candidate_not_allowed",
                        engine.inner.clock.now(),
                    )
                    .await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    continue;
                }
                let failed = failure(command.as_ref());
                let reference = actual.offering().ok_or(EngineError::StaleToolSource)?;
                let model = if actual.executor == "model-backed" {
                    reference.offering.as_str()
                } else {
                    ""
                };
                let row=crate::calls::reserve_service(&mut tx,&request,command_id,&failed,
                    serde_json::json!({"provider_id":reference.provider,"model":model,"capability_route":actual.reference,
                        "configured_capability_route":configured.reference}),engine.inner.clock.now()).await?;
                let Some(row) = row else { return Err(original) };
                if let Some(reason) = pause_reason(&row, deadline, engine.inner.clock.now()) {
                    let paused = crate::calls::tool_pause::retain(
                        &mut tx,
                        &row,
                        &configured,
                        reason,
                        engine.inner.clock.now(),
                    )
                    .await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    return Ok(paused.result());
                }
                tx.commit().await.map_err(orch_db::DbError::from)?;
            }
        }
    }
}
