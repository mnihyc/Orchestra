//! Admit the real actor and model before runtime work or a native model command.
use super::*;
use orch_engine::{
    commands::Outgoing, provider_relay::RuntimeAdmission, provider_runtime::RuntimeStart,
};
use orch_link_contract::commands::{CommandKind, LeaseMode, LeaseOpen, ProviderRelay, SerialKey};
use orch_types::{Hash, Time, new_id};
use std::collections::BTreeMap;

async fn room(
    ctx: &Ctx,
    conn: &mut Conn,
    selection: &Selection,
    payload: &Json,
    format: &str,
    user: Id,
) -> ApiResult<()> {
    let tier = orch_model_contract::parameters::requested_tier_for_caps(format, payload);
    if !ctx
        .app
        .engine
        .room_for_own_use(
            conn,
            &orch_engine::caps::OwnCapAsk {
                place: selection.provider.org_id,
                provider: selection.provider.id,
                model: &selection.model.model_name,
                tier: tier.as_deref(),
                purpose: orch_db::usage::ledger::purpose::RELAY,
                person: Some(user),
            },
        )
        .await
        .map_err(ApiError::engine)?
    {
        return Err(unavailable(
            "A usage cap leaves no room for this Relay call.",
        ));
    }
    Ok(())
}
pub(super) struct Request<'a> {
    pub provider: Id,
    pub model: &'a str,
    pub format: &'a str,
    pub path: &'a str,
    pub payload: &'a Json,
    pub input: Bytes,
}
pub(super) async fn run(
    ctx: &Ctx,
    exchange: &mut exchange::Exchange,
    request: Request<'_>,
) -> ApiResult<()> {
    let Request {
        provider,
        model,
        format,
        path,
        payload,
        input,
    } = request;
    let mut tx = ctx.app.db.begin().await?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let selected = orch_engine::provider_relay::select(
        &mut tx,
        guard.principal(),
        provider,
        model,
        format,
        payload,
    )
    .await
    .map_err(mapped)?;
    let prepared = orch_model_contract::features::prepare(
        format,
        &orch_model_contract::features::builtin(format),
        &selected.parameters,
        &input,
        None,
    )
    .map_err(|error| invalid(error.to_string()))?;
    orch_model_contract::parameters::check_relay_options(
        format,
        prepared.model.as_ref(),
        prepared.body.as_ref(),
    )
    .map_err(invalid)?;
    let input = Bytes::from(
        orch_model_contract::parameters::outgoing_at(
            format,
            path,
            prepared.model.as_ref(),
            prepared.body.as_ref(),
        )
        .map_err(invalid)?,
    );
    if u64::try_from(input.len()).unwrap_or(u64::MAX) > exchange.options.request_bytes {
        return Err(ApiError::new(
            ErrorCode::RequestTooLarge,
            "The normalized model request exceeds providers.relay_request_bytes.",
        ));
    }
    let normalized: Json = serde_json::from_slice(&input).map_err(ApiError::internal)?;
    room(
        &current,
        &mut tx,
        &selected,
        &normalized,
        format,
        guard.principal().user(),
    )
    .await?;
    let hash = Hash::of(&input);
    let id = new_id();
    exchange.command = Some(id);
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for name in [
        "accept",
        "content-type",
        "anthropic-version",
        "anthropic-beta",
        "openai-beta",
    ] {
        if let Some(value) = ctx.headers.get(name) {
            headers.insert(
                name.to_owned(),
                value
                    .to_str()
                    .map_err(|_| invalid("A model-interface header is not valid text."))?
                    .to_owned(),
            );
        }
    }
    headers
        .entry("content-type".into())
        .or_insert_with(|| "application/json".into());
    let parameters = BTreeMap::from([(model.to_owned(), selected.parameters.clone())]);
    let effort = orch_provider_contract::calls::requested_effort(&normalized)
        .map_err(invalid)?
        .map(str::to_owned)
        .or_else(|| selected.parameters.default_effort.clone());
    let origin = orch_authz::CapturedOperation::capture(guard.principal().caller())
        .map_err(ApiError::internal)?;
    let raw = http_origin::capture_operation(&current, &mut tx, &origin).await?;
    let mut metadata = serde_json::to_value(&headers).map_err(ApiError::internal)?;
    if let Some(tier) =
        orch_model_contract::parameters::requested_tier_for_caps(format, &normalized)
    {
        metadata["cap_tier"] = Json::String(tier);
    }
    let deadline = raw
        .until()
        .into_iter()
        .chain(
            guard
                .principal()
                .caller()
                .grant()
                .and_then(orch_authz::TokenGrant::expires_at),
        )
        .chain(std::iter::once(exchange.options.until))
        .min()
        .ok_or_else(|| ApiError::internal("the admitted model call has no deadline"))?;
    let mut call = rows::Call {
        id,
        provider_id: provider,
        machine_id: None,
        user_id: guard.principal().user(),
        entry_id: selected.entry,
        home_id: selected.provider.home_id,
        org_id: selected.provider.org_id,
        model_name: model.to_owned(),
        effort,
        protocol: format.to_owned(),
        path: path.to_owned(),
        relay_id: exchange.relay.id(),
        request_hash: hash.to_string(),
        request_bytes: i64::try_from(input.len()).map_err(ApiError::internal)?,
        max_request_bytes: i64::try_from(exchange.options.request_bytes)
            .map_err(ApiError::internal)?,
        idle_secs: i64::try_from(exchange.options.idle_secs).map_err(ApiError::internal)?,
        retry_ms: i64::try_from(exchange.options.retry_ms).map_err(ApiError::internal)?,
        model_parameters: serde_json::to_value(&parameters).map_err(ApiError::internal)?,
        headers: metadata,
        command_body: serde_json::json!({}),
        source: serde_json::to_value(&origin).map_err(ApiError::internal)?,
        created_at: guard.now().get(),
        deadline: deadline.get(),
        finished_at: None,
        native_done_at: None,
    };
    let held = current.app.blobs.claim_command_transfer(
        current.app.db.clone(),
        Arc::clone(&current.app.clock),
        current.app.link.dispatcher(),
        id,
        deadline,
    );
    guard.check_deadline(&current)?;
    held.begin_in(&mut tx).await?;
    exchange.hold(held);
    rows::create_call(&mut tx, &call).await?;
    let source =
        CallSource::initial(guard.principal().caller(), &call).map_err(ApiError::engine)?;
    source
        .check(&mut tx, ctx.app.clock.as_ref(), &raw)
        .await
        .map_err(continuing_error)?;
    let dedicated = selected.provider.placement == "managed"
        && orch_db::queries::provider_connections::environment(&mut tx, provider)
            .await?
            .is_none();
    tx.commit().await.map_err(DbError::from)?;
    // Runtime jobs keep their existing lifecycle; no environment owned by another person is started here.
    if dedicated {
        let admission = RuntimeAdmission::new(source.clone(), raw.clone());
        if let RuntimeStart::Job(job) = ctx
            .app
            .engine
            .start_provider_runtime_for_relay(provider, call.user_id, &admission)
            .await
            .map_err(|error| unavailable(error.to_string()))?
        {
            loop {
                let mut tx = ctx.app.db.begin().await?;
                source
                    .check(&mut tx, ctx.app.clock.as_ref(), &raw)
                    .await
                    .map_err(continuing_error)?;
                let job = orch_db::queries::jobs::job(&mut tx, job.id)
                    .await?
                    .ok_or_else(|| {
                        unavailable("The provider runtime's start job is unavailable.")
                    })?;
                tx.commit().await.map_err(DbError::from)?;
                if !job.is_open() {
                    if job.state != "succeeded" {
                        return Err(unavailable(
                            "The provider runtime could not start; its job retains the failure.",
                        ));
                    }
                    break;
                }
                if ctx.app.clock.time() >= Time::new(call.deadline) {
                    return Err(unavailable(
                        "The provider runtime exceeded jobs.io_timeout_secs.",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(exchange.options.retry_ms)).await;
            }
        }
    }
    let plan = ctx
        .app
        .engine
        .public_relay_plan(provider)
        .await
        .map_err(ApiError::engine)?;
    let mut tx = ctx.app.db.begin().await?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    source
        .check(&mut tx, ctx.app.clock.as_ref(), &raw)
        .await
        .map_err(continuing_error)?;
    let machine = orch_db::queries::resources::machine(&mut tx, plan.machine)
        .await?
        .filter(|machine| machine.online && machine.status == "active")
        .ok_or_else(|| unavailable("The machine keeping this provider's connection is offline."))?;
    let fresh = orch_engine::provider_relay::select_frozen(
        &mut tx,
        guard.principal(),
        provider,
        model,
        format,
        call.effort.as_deref(),
    )
    .await
    .map_err(mapped)?;
    if !orch_engine::provider_relay::same_request_policy(
        &call.model_parameters[model],
        &fresh.parameters,
    )
    .map_err(ApiError::engine)?
        || orch_engine::leases::machine_of(&fresh.provider) != Some(machine.id)
    {
        return Err(unavailable(
            "The provider's model or connection changed while the call waited.",
        ));
    }
    room(&current, &mut tx, &fresh, &normalized, format, call.user_id).await?;
    let body = ProviderRelay {
        start: plan.body,
        lease: LeaseOpen {
            lease_id: id,
            provider_id: provider,
            thread_id: id,
            mode: LeaseMode::Forwarded,
            protocol: Some(format.to_owned()),
            form: None,
            recipient: None,
            allowed_models: vec![model.to_owned()],
            model_parameters: parameters,
            request_options: BTreeMap::new(),
            access_program: None,
            native_model: None,
            allowance: None,
        },
        relay_id: call.relay_id,
        path: path.to_owned(),
        headers,
        request_bytes: call.request_bytes,
        request_hash: hash,
        max_request_bytes: exchange.options.request_bytes,
        idle_secs: exchange.options.idle_secs,
        retry_ms: exchange.options.retry_ms,
    };
    call.machine_id = Some(machine.id);
    call.command_body = serde_json::to_value(body).map_err(ApiError::internal)?;
    guard.check_deadline(&current)?;
    if !rows::bind_command(&mut tx, id, machine.id, &call.command_body).await? {
        return Err(unavailable(
            "The admitted model call ended before its command was prepared.",
        ));
    }
    let command = Outgoing {
        id,
        machine: machine.id,
        key: SerialKey::None,
        kind: CommandKind::ProviderRelay,
        body: call.command_body.clone(),
        deadline: Some(Time::new(call.deadline)),
        authority: None,
        content_transit: false,
    };
    let (source, raw) = native_command::issue(&current, tx, &call, command).await?;
    exchange.attach(source, raw);
    exchange.start(input);
    Ok(())
}
