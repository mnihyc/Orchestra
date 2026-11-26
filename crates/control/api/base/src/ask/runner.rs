//! The admitted turn's native model choices, read witnesses and thread items.

use super::*;

use async_trait::async_trait;

use orch_engine::ask::Limits;

use orch_engine::ask::Step;

use orch_engine::ask::ToolAnswer;

use orch_engine::ask::TurnHost;

use orch_engine::ask::model::Chosen;

use orch_engine::error::EngineError;

use orch_link_contract::commands::ProviderGenerate;

use orch_model_contract::generation::GenerationInput;

use orch_model_contract::generation::GenerationMessage;

use orch_model_contract::generation::GenerationOutput;

use orch_model_contract::generation::GenerationPart;

use orch_model_contract::generation::GenerationRole;

use std::collections::HashMap;
fn failed(error: impl std::fmt::Display) -> EngineError {
    EngineError::unreadable(error)
}

fn refused(error: ApiError) -> EngineError {
    match error.problem.extra.get("reason").and_then(Json::as_str) {
        Some(reason) => EngineError::AskRefused {
            reason: reason.to_owned(),
            message: error.problem.detail.clone(),
        },
        None => failed(&error.problem.detail),
    }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    body: api::AskMessageBody,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        let target = crate::key_operations::targets::node(conn, id).await?;
        let hashes = orch_chat::following::content_blobs(&body.content);
        for hash in &hashes {
            if rows::file(conn, id, hash).await?.is_none() {
                return Err(ApiError::not_found());
            }
        }
        let plan = crate::key_operations::native::bind(
            ctx,
            &orch_engine::ask::command::MESSAGE,
            &orch_engine::ask::command::facts(target, !hashes.is_empty()),
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let row = rows::get(conn, id, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if row.temporary && row.user_id != caller.user() {
            return Err(ApiError::not_found());
        }
        super::program::opening(ctx, conn, &principal, id, false).await?;
        Ok(plan)
    })
}

fn model_error(error: orch_engine::ask::model::Error) -> ApiError {
    match error {
        orch_engine::ask::model::Error::Db(error) => error.into(),
        orch_engine::ask::model::Error::Denied(refusal) => {
            denied_because(&refusal.reason, refusal.message)
        }
        orch_engine::ask::model::Error::Native(error) => ApiError::internal(error),
    }
}

async fn choose(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    wanted: Option<api::AskModel>,
    current_turn: Option<Id>,
) -> ApiResult<Chosen> {
    let model = wanted.ok_or_else(|| ApiError::internal("The accepted Ask model is absent"))?;
    let selected = Box::pin(orch_engine::ask::model::select(
        conn,
        principal,
        root,
        model,
        ctx.app.clock.time(),
        current_turn,
    ))
    .await
    .map_err(model_error)?;
    selected
        .available(
            &ctx.app.engine,
            conn,
            root,
            principal.caller().user(),
            current_turn,
        )
        .await
        .map_err(model_error)
}

async fn choose_input(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    turn: Id,
    row: &rows::Conversation,
    question: &GenerationMessage,
    page: Option<&str>,
    wanted: Option<api::AskPreference>,
) -> ApiResult<(Chosen, bool)> {
    use orch_provider_contract::pools::{Kind, Target};
    let root = row.root_org_id;
    let thread = row.id;
    let choices = super::models::resolve(conn, principal, root).await?;
    if let Some(reason) = choices.blocked_reason {
        return Err(super::models::blocked(reason));
    }
    let wanted = wanted
        .or_else(|| choices.default_model.map(Into::into))
        .ok_or_else(|| super::models::blocked(api::AskModelsBlockedReason::NoUsableModels))?;
    let mut listed = false;
    let mut visible = false;
    let mut allowed = false;
    for entry in &choices.models {
        if entry.selection.matches(&wanted) {
            listed = true;
            visible |= entry.model.is_some();
            allowed |= entry.allowed;
        }
    }
    if !listed {
        return Err(ApiError::invalid(
            "model",
            "This model is not on this organization's Ask list. Choose one of the listed models.",
        ));
    }
    match wanted {
        api::AskPreference::Backend(ref model) => {
            if !visible {
                return Err(ApiError::invalid(
                    "model",
                    "The configured Ask model is unavailable to you",
                ));
            }
            if !allowed {
                return Err(ApiError::invalid(
                    "model",
                    "Your access to this provider does not cover this model's default options. Choose another model.",
                ));
            }
            Ok((
                choose(ctx, conn, principal, root, Some(model.clone()), None).await?,
                false,
            ))
        }
        api::AskPreference::Pool {
            target:
                Target::Pool {
                    kind: kind @ (Kind::Model | Kind::Provider),
                    id,
                },
            ref model,
            ref params,
        } => {
            let chosen = Box::pin(orch_engine::ask::pool::reserve(
                &ctx.app.engine,
                conn,
                principal,
                root,
                thread,
                turn,
                kind,
                id,
                model.as_deref(),
                params.as_ref(),
                &material::Question {
                    ctx,
                    row,
                    turn,
                    question,
                    page,
                },
            ))
            .await
            .map_err(model_error)?;
            Ok((chosen, true))
        }
        api::AskPreference::Pool { .. } => {
            Err(ApiError::invalid("model", "Choose a model pool for Ask."))
        }
    }
}

/// Only a resumed saved choice gives way to today's allowed default. An explicit
/// new choice is still checked as submitted; a cap or payer refusal never triggers
/// an unrelated model fallback.
async fn resume_model(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    explicit: Option<Option<api::AskPreference>>,
    saved: Option<api::AskPreference>,
) -> ApiResult<(Option<api::AskPreference>, bool)> {
    if let Some(explicit) = explicit {
        return Ok((explicit, false));
    }
    if saved.is_none() {
        return Ok((None, false));
    }
    let choices = super::models::resolve(conn, principal, root).await?;
    if let Some(reason) = choices.blocked_reason {
        return Err(super::models::blocked(reason));
    }
    let allowed = |model: &api::AskPreference| {
        choices
            .models
            .iter()
            .any(|choice| choice.allowed && choice.selection.matches(model))
    };
    if saved.as_ref().is_some_and(allowed) {
        return Ok((saved, false));
    }
    if saved.as_ref().is_some_and(|choice| match choice {
        api::AskPreference::Backend(model) => model.backend.route == api::PROGRAM_ROUTE,
        api::AskPreference::Pool { .. } => true,
    }) || choices.default_model.as_ref().is_some_and(|choice| {
        choice
            .backend()
            .is_none_or(|backend| backend.route == api::PROGRAM_ROUTE)
    }) {
        let reason = saved
            .as_ref()
            .and_then(|model| {
                choices
                    .models
                    .iter()
                    .find(|entry| entry.selection.matches(model))
            })
            .and_then(|entry| entry.unavailable_reason.as_deref())
            .unwrap_or("selected_backend_unavailable");
        return Err(denied_because(
            reason,
            "The selected Ask backend is unavailable. Choose another model to continue.",
        ));
    }
    choices
        .default_model
        .ok_or_else(|| super::models::blocked(api::AskModelsBlockedReason::NoUsableModels))?;
    Ok((None, true))
}

fn words(content: &[orch_api_contract::threads::ContentPart]) -> String {
    content
        .iter()
        .map(|part| match part {
            orch_api_contract::threads::ContentPart::Text { text } => text.clone(),
            _ => serde_json::to_string(part).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) async fn message(ctx: Ctx, body: api::AskMessageBody) -> ApiResult<api::AskReceipt> {
    Ok(accept(ctx, body).await?.receipt)
}

pub struct Accepted {
    pub receipt: api::AskReceipt,
    pub input: Id,
    pub created: bool,
    pub caller: orch_authz::Caller,
}

pub(super) async fn accept(ctx: Ctx, body: api::AskMessageBody) -> ApiResult<Accepted> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let pending = accept_in(&ctx, &mut tx, &guard, ctx.id("id")?, body).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(pending.start(&ctx.app))
}

/// A controller cannot start while its thread/input transaction remains private.
pub struct Pending {
    pub accepted: Accepted,
    host: Option<Arc<Host>>,
}

impl Pending {
    /// Call only after the transaction that accepted the turn committed.
    pub(super) fn start(self, app: &crate::state::App) -> Accepted {
        if let Some(host) = self.host {
            launch(app, host);
        }
        self.accepted
    }
}

/// The ordinary writer within an existing, freshly guarded thread-birth transaction.
pub(super) async fn accept_in(
    ctx: &Ctx,
    conn: &mut Conn,
    guard: &transaction_guard::Guard,
    thread: Id,
    body: api::AskMessageBody,
) -> ApiResult<Pending> {
    if body.client_id.is_empty()
        || body.client_id.len() > 128
        || body.content.is_empty()
        || words(&body.content).chars().count()
            > orch_api_contract::threads::INPUT_TEXT_MAX_CHARS as usize
    {
        return Err(ApiError::invalid(
            "content",
            "Supply a client ID and a bounded nonempty message",
        ));
    }
    let principal = guard
        .principal()
        .ordinary(&mut *conn)
        .await
        .map_err(ApiError::internal)?;
    let row = super::program::opening(ctx, &mut *conn, &principal, thread, true).await?;
    let person = principal.caller().user();
    let caller = principal.caller().clone();
    if let Some(previous) =
        threads::input_by_client(&mut *conn, row.id, Some(person), &body.client_id).await?
    {
        return Ok(Pending {
            accepted: Accepted {
                receipt: api::AskReceipt {
                    conversation_id: row.id,
                    turn_id: previous.turn_id,
                    seq: row.seq,
                },
                input: previous.id,
                created: false,
                caller,
            },
            host: None,
        });
    }
    if row.state != "ready" {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            "Stop the current turn or start a new conversation",
        ));
    }
    // A page that belongs to no organization (Management, Settings, the inbox) is context only and
    // agrees with the conversation's organization; only a page inside another one disagrees.
    if let Some(context) = body.context.as_deref()
        && root_of_context(
            ctx,
            &mut *conn,
            &principal,
            Some(context),
            Some(row.root_org_id),
        )
        .await?
            != row.root_org_id
    {
        return Err(denied_because(
            "other_organization",
            "This page belongs to another organization. Ask in that organization.",
        ));
    }
    let saved = row
        .model
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let (wanted, changed_model) = resume_model(
        &mut *conn,
        &principal,
        row.root_org_id,
        body.model.clone(),
        saved,
    )
    .await?;
    let preference = wanted
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let mut supplied = Vec::new();
    for part in &body.content {
        let blob = match part {
            orch_api_contract::threads::ContentPart::Image { blob, .. } => Some(*blob),
            orch_api_contract::threads::ContentPart::File { blob, .. } => *blob,
            _ => None,
        };
        if let Some(blob) = blob {
            let file = rows::file(&mut *conn, row.id, &blob)
                .await?
                .ok_or_else(|| {
                    ApiError::invalid(
                        "content",
                        "Link this private upload to the conversation before sending it",
                    )
                })?;
            let url = super::files::url(row.id, &blob, row.temporary);
            if serde_json::from_value::<orch_api_contract::threads::ImageMediaType>(json!(
                file.media_type
            ))
            .is_ok()
            {
                supplied.push(GenerationPart::ImageRef {
                    blob,
                    size: file.size,
                    media_type: file.media_type,
                    name: file.name,
                    url,
                });
            } else {
                supplied.push(GenerationPart::Text {
                    text: format!("[{}]({url})", file.name),
                });
            }
        } else {
            supplied.push(GenerationPart::Text {
                text: words(std::slice::from_ref(part)),
            });
        }
    }
    let generation = GenerationMessage {
        role: GenerationRole::User,
        content: supplied,
        continuation: vec![],
    };
    let turn = new_id();
    let (chosen, pooled) = choose_input(
        ctx,
        &mut *conn,
        &principal,
        turn,
        &row,
        &generation,
        body.context.as_deref(),
        wanted,
    )
    .await?;
    guard.before_effect(ctx).map_err(ApiError::engine)?;
    let input = new_id();
    let segment = new_id();
    let owner = new_id();
    let seq = orch_db::sequences::next(&mut *conn, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    rows::segment(
        &mut *conn,
        segment,
        row.id,
        &rows::SegmentBinding {
            provider_id: chosen.offering.provider,
            transport: match &chosen.protocol {
                Some(protocol) => rows::DialogueTransport::Api {
                    protocol: protocol.clone(),
                    backend: chosen.model.backend.clone(),
                },
                None => rows::DialogueTransport::Program(chosen.model.backend.clone()),
            },
            model: chosen.offering.offering.clone(),
        },
        person,
        ctx.now,
        seq,
    )
    .await?;
    let authority =
        orch_engine::ask::program::input_origin(principal.caller()).map_err(ApiError::engine)?;
    let content = serde_json::to_value(&body.content).map_err(ApiError::internal)?;
    threads::insert_input(
        &mut *conn,
        &threads::NewInput {
            id: input,
            thread_id: row.id,
            author_id: Some(person),
            submission_authority: Some(&authority),
            run_as: None,
            run_as_authority: None,
            source: "user",
            content: &content,
            delivery: "queue",
            state: "delivered",
            turn_id: None,
            client_id: Some(&body.client_id),
            limit_bypass: None,
            place: threads::Place::End,
            at: ctx.now,
            plan: false,
        },
        seq,
    )
    .await?;
    threads::insert_turn(
        &mut *conn,
        &threads::NewTurn {
            id: turn,
            thread_id: row.id,
            segment_id: segment,
            input_id: input,
            model: &chosen.offering.offering,
            effort: None,
            params: chosen.model.params.as_ref().unwrap_or(&Json::Null),
            lease_id: None,
            limit_bypass: None,
            at: ctx.now,
        },
        seq,
    )
    .await?;
    if pooled {
        orch_engine::ask::pool::bind(&mut *conn, turn, turn)
            .await
            .map_err(ApiError::engine)?;
    }
    threads::start_running(&mut *conn, turn, None, ctx.now, seq).await?;
    rows::input_turn(&mut *conn, input, turn).await?;
    orch_engine::notifications::follow_thread(&mut *conn, person, row.id, ctx.now)
        .await
        .map_err(ApiError::engine)?;
    let payload = json!({"parts":body.content,"input_id":input,"delivery":"queue","source":"user","ask_message":generation});
    let item = new_id();
    threads::insert_item(
        &mut *conn,
        &threads::NewItem {
            id: item,
            thread_id: row.id,
            segment_id: segment,
            turn_id: Some(turn),
            stream: "main",
            parent_item_id: None,
            kind: "user_message",
            status: "completed",
            author_id: Some(person),
            payload: &payload,
            native_ref: None,
            search_text: None,
            at: ctx.now,
        },
        seq,
    )
    .await?;
    rows::running(
        &mut *conn,
        row.id,
        turn,
        preference.as_ref(),
        chosen.payer,
        body.context.as_deref(),
        ctx.now,
    )
    .await?;
    rows::runner(&mut *conn, row.id, turn, owner, ctx.app.clock.now()).await?;
    let message = api::AskMessage {
        id: item,
        turn_id: turn,
        role: api::AskRole::User,
        content: serde_json::from_value(content).map_err(ApiError::internal)?,
        citations: vec![],
        summary: None,
        complete: true,
        created_at: Time::new(ctx.now),
    };
    let seq = rows::emit(
        &mut *conn,
        row.id,
        &serde_json::to_value(api::AskEventData::Message {
            message: Box::new(message),
        })
        .map_err(ApiError::internal)?,
    )
    .await?;
    if changed_model {
        rows::emit(&mut *conn, row.id, &json!({"kind":"notice","code":"model_changed",
            "message":"The previous model is no longer allowed. Ask is using the current default."})).await?;
    }
    emit_header(ctx, &mut *conn, row.id).await?;
    let program_identity = if chosen.protocol.is_none() {
        Some(
            crate::key_operations::native::http_origin::capture_preparation(
                ctx,
                conn,
                principal.caller(),
            )
            .await?,
        )
    } else {
        None
    };
    let origin = Arc::new(tools::ReadOrigin {
        epochs: None,
        source: ctx.clone(),
        principal,
        root: row.root_org_id,
        conversation: row.id,
    });
    let host = Arc::new(Host {
        reading: material::Reading::new(origin, row.temporary, turn, chosen.model, chosen.protocol),
        program_identity,
        segment,
        owner,
        pending: Default::default(),
        compactions: Default::default(),
        calls: Default::default(),
        opaque_inputs: Default::default(),
    });
    Ok(Pending {
        accepted: Accepted {
            receipt: api::AskReceipt {
                conversation_id: row.id,
                turn_id: Some(turn),
                seq,
            },
            input,
            created: true,
            caller,
        },
        host: Some(host),
    })
}

fn launch(app: &crate::state::App, host: Arc<Host>) {
    let app = app.clone();
    super::recovery::install(&app);
    tokio::spawn(async move {
        let done = tokio_util::sync::CancellationToken::new();
        let lost = tokio_util::sync::CancellationToken::new();
        super::recovery::watch(
            app.clone(),
            host.thread,
            host.turn,
            host.owner,
            done.clone(),
            lost.clone(),
        );
        let result = tokio::select! {
            result = run(&app, &host) => result,
            () = lost.cancelled() => Err(failed("The Ask controller became unavailable. Send a follow-up to continue.")),
        };
        done.cancel();
        if let Err(error) = finish(&app, &host, result).await {
            tracing::warn!(%error,"Ask turn could not be finalized");
        }
    });
}

mod material;

struct Host {
    reading: material::Reading,
    program_identity: Option<orch_authz::http_origin::HttpPreparation>,
    segment: Id,
    owner: Id,
    pending: tokio::sync::Mutex<HashMap<String, (Option<tools::Trace>, Json)>>,
    calls: tokio::sync::Mutex<HashMap<Id, Id>>,
    compactions: tokio::sync::Mutex<HashMap<Id, context::Pending>>,
    opaque_inputs: tokio::sync::Mutex<HashMap<Id, bool>>,
}

impl std::ops::Deref for Host {
    type Target = material::Reading;
    fn deref(&self) -> &Self::Target {
        &self.reading
    }
}

impl Host {
    async fn lock_active(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let row = rows::get(conn, self.thread, true)
            .await?
            .ok_or_else(|| failed("Ask was deleted"))?;
        if row.turn != Some(self.turn)
            || row.state != "running"
            || row.runner != Some(self.owner)
            || !super::recovery::alive(row.heartbeat, self.origin.source.app.clock.now())
        {
            return Err(failed("Ask was stopped"));
        }
        Ok(())
    }
}

async fn run(
    app: &crate::state::App,
    host: &Arc<Host>,
) -> Result<orch_model_contract::generation::GenerationStop, EngineError> {
    let get = |key: &'static str, default: i64| async move {
        setting::<i64>(
            &mut *app.db.acquire().await?,
            Place::Node(host.origin.root),
            key,
        )
        .await
        .map(|value| value.unwrap_or(default))
        .map_err(failed)
    };
    let limits = Limits {
        steps: u32::try_from(get("ask.max_steps", 16).await?).map_err(failed)?,
        tools: u32::try_from(get("ask.max_tools", 32).await?).map_err(failed)?,
    };
    let input = material::request(host.origin.root, get("ask.max_output_tokens", 4096).await?);
    app.engine.run_ask(host.as_ref(), input, limits).await
}

async fn finish(
    app: &crate::state::App,
    host: &Host,
    result: Result<orch_model_contract::generation::GenerationStop, EngineError>,
) -> Result<(), EngineError> {
    let mut tx = app.db.begin().await?;
    let Some(row) = rows::get(&mut tx, host.thread, true).await? else {
        return Ok(());
    };
    if row.turn != Some(host.turn) || row.runner != Some(host.owner) {
        return Ok(());
    }
    let calls = if result.is_err() {
        rows::calls(&mut tx, host.turn).await?
    } else {
        Vec::new()
    };
    let seq = orch_db::sequences::next(&mut tx, host.thread)
        .await?
        .ok_or_else(|| failed("Ask was deleted"))?;
    let error = result.as_ref().err().map(|error| {
        let (code, model) = match error {
            EngineError::AskContextTooLong(model) => ("ask_context_too_long", Some(model.as_str())),
            EngineError::AskTurnTooLong(model) => ("ask_turn_too_long", Some(model.as_str())),
            _ => ("ask_unavailable", None),
        };
        // A turn's error always says whether trying again may help: a turn whose stored error lacked the
        // member could not be read again, and its conversation failed to open from then on.
        let mut failure =
            json!({"code":code,"message":error.to_string(),"retryable":code == "ask_unavailable"});
        if let Some(model) = model {
            failure["model"] = json!(model);
        }
        if let EngineError::AskRefused { reason, .. } = error {
            failure["code"] = json!(format!("ask.{reason}"));
            failure["retryable"] = json!(false);
        }
        failure
    });
    threads::finish_turn(
        &mut tx,
        host.turn,
        if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
        error.as_ref(),
        app.clock.now(),
        seq,
    )
    .await?;
    threads::settle_items_of_turn(
        &mut tx,
        host.turn,
        if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
        app.clock.now(),
        seq,
    )
    .await?;
    rows::finish(&mut tx, host.thread, host.turn).await?;
    emit_settled(&host.origin.source, &mut tx, host.thread, host.turn)
        .await
        .map_err(failed)?;
    if let Some(mut error) = error {
        error["kind"] = json!("error");
        rows::emit(&mut tx, host.thread, &error).await?;
    }
    if result.is_ok() {
        rows::emit(
            &mut tx,
            host.thread,
            &json!({"kind":"notice","code":"ready","message":"Ask is ready"}),
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    app.engine.cancel_ask_calls(&calls).await?;
    Ok(())
}

#[async_trait]
impl TurnHost for Host {
    async fn issue(
        &self,
        engine: &orch_engine::Engine,
        command: orch_engine::commands::Outgoing,
    ) -> Result<(), EngineError> {
        let ctx = &self.origin.source;
        let (mut tx, guard) = transaction_guard::begin(ctx).await.map_err(failed)?;
        let principal = guard.principal().ordinary(&mut tx).await?;
        let row = super::program::opening(ctx, &mut tx, &principal, self.thread, true)
            .await
            .map_err(refused)?;
        if row.root_org_id != self.origin.root {
            return Err(failed("The conversation's governing root changed"));
        }
        if row.turn != Some(self.turn)
            || row.state != "running"
            || row.runner != Some(self.owner)
            || !super::recovery::alive(row.heartbeat, self.origin.source.app.clock.now())
        {
            return Err(failed("Ask was stopped"));
        }
        let request: ProviderGenerate = serde_json::from_value(command.body.clone())?;
        let current = choose(
            ctx,
            &mut tx,
            &principal,
            self.origin.root,
            Some(api::AskModel {
                backend: self.model.backend.clone(),
                params: request.params.clone(),
            }),
            Some(self.turn),
        )
        .await
        .map_err(refused)?;
        if current.machine != Some(command.machine)
            || current.protocol.as_deref() != Some(request.protocol.as_str())
            || current.offering.provider != request.provider_id
            || current.offering.offering != request.model_name
            || current.model.params != request.params
            || current.parameters != request.model_parameters
        {
            return Err(failed(
                "The model configuration changed before this request started. Retry the question.",
            ));
        }
        let budget = context::budget(
            &mut tx,
            self.origin.root,
            &current.parameters,
            &current.offering.offering,
            self.learned_window
                .load(std::sync::atomic::Ordering::Relaxed),
            self.reduced_output
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        .await?;
        if request.input.max_output_tokens > budget.output || !budget.fits(&request.input)? {
            return Err(EngineError::AskContextTooLong(request.model_name.clone()));
        }
        if row.payer_id != Some(current.payer) {
            sqlx::query("UPDATE threads SET ask_payer_id=$2 WHERE node_id=$1")
                .bind(self.thread)
                .bind(current.payer)
                .execute(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
            emit_header(ctx, &mut tx, self.thread)
                .await
                .map_err(failed)?;
        }
        let mut command = command;
        let bound = ctx.route.name == "AskMessageCreate";
        if bound {
            let source = orch_engine::ask::command::CommandSource::capture(
                &mut tx,
                principal.caller(),
                orch_engine::ask::command::Controller {
                    thread: self.thread,
                    turn: self.turn,
                    runner: self.owner,
                },
                &command,
            )
            .await?;
            let raw = crate::key_operations::native::http_origin::capture_operation(
                ctx,
                &mut tx,
                source.origin(),
            )
            .await
            .map_err(failed)?;
            let authority = crate::key_operations::native::http_origin::bind_source(
                &raw,
                &command,
                source.command(&command)?,
            )
            .map_err(failed)?;
            command.authority = Some(authority);
        }
        let before: orch_engine::commands::BeforeCommand<'_> = Box::new(|| {
            guard
                .check_deadline(ctx)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            if !super::recovery::alive(row.heartbeat, ctx.app.clock.now()) {
                return Err(
                    Box::new(std::io::Error::other("The Ask controller lease expired"))
                        as Box<dyn std::error::Error + Send + Sync>,
                );
            }
            Ok(())
        });
        if bound {
            // The native command guard now takes these same controller/source locks in its own
            // transaction. Release this preliminary transaction before that canonical insert.
            tx.commit().await.map_err(orch_db::DbError::from)?;
            engine.queue_ask_command(command, before).await?;
        } else {
            // Other entry operations keep their existing queue transaction.
            engine.queue_ask_command(command, before).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(())
    }
    async fn check_active(&self) -> Result<(), EngineError> {
        let mut conn = self.origin.source.app.db.acquire().await?;
        let principal = self
            .origin
            .source
            .operation_principal(&mut conn)
            .await
            .map_err(refused)?
            .ordinary(&mut conn)
            .await?;
        let row = super::program::opening(
            &self.origin.source,
            &mut conn,
            &principal,
            self.thread,
            false,
        )
        .await
        .map_err(refused)?;
        if row.root_org_id != self.origin.root {
            return Err(failed("The conversation's governing root changed"));
        }
        if row.turn != Some(self.turn)
            || row.state != "running"
            || row.runner != Some(self.owner)
            || !super::recovery::alive(row.heartbeat, self.origin.source.app.clock.now())
        {
            return Err(failed("Ask was stopped"));
        }
        Ok(())
    }

    async fn prepare(&self, input: &GenerationInput, repair: bool) -> Result<Step, EngineError> {
        self.check_active().await?;
        let ctx = &self.origin.source;
        let mut conn = ctx.app.db.acquire().await?;
        let principal = ctx
            .operation_principal(&mut conn)
            .await
            .map_err(refused)?
            .ordinary(&mut conn)
            .await?;
        let chosen = choose(
            ctx,
            &mut conn,
            &principal,
            self.origin.root,
            Some(self.model.clone()),
            Some(self.turn),
        )
        .await
        .map_err(refused)?;
        if chosen.protocol != self.protocol {
            return Err(failed(
                "The dialogue interface changed. Retry the question under its current model.",
            ));
        }
        drop(conn);
        if chosen.protocol.is_some() {
            ctx.app
                .engine
                .start_ask_provider(self.thread, chosen.offering.provider)
                .await?;
        }
        self.check_active().await?;
        let mut conn = ctx.app.db.acquire().await?;
        let principal = ctx
            .operation_principal(&mut conn)
            .await
            .map_err(refused)?
            .ordinary(&mut conn)
            .await?;
        let current = choose(
            ctx,
            &mut conn,
            &principal,
            self.origin.root,
            Some(chosen.model.clone()),
            Some(self.turn),
        )
        .await
        .map_err(refused)?;
        if current.machine != chosen.machine || current.protocol != chosen.protocol {
            return Err(failed(
                "The provider location changed before this request started. Retry the question.",
            ));
        }
        let chosen = current;
        let (origin, credential_stamp) = if let Some(identity) = &self.program_identity {
            let mut tx = sqlx::Connection::begin(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            let raw = identity
                .current(&mut tx, ctx.app.clock.as_ref())
                .await
                .map_err(failed)?;
            let stamp = orch_engine::ask::program::credential_stamp(raw.caller())?;
            let epochs = orch_engine::ask::program::ReadEpochs::observe(
                &mut tx,
                &[self.origin.root, self.thread, chosen.offering.provider],
            )
            .await?;
            raw.check(ctx.app.clock.as_ref()).map_err(failed)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            (
                Arc::new(tools::ReadOrigin {
                    epochs: Some(Arc::new(std::sync::Mutex::new(epochs))),
                    ..(*self.origin).clone()
                }),
                Some(stamp),
            )
        } else {
            (self.origin.clone(), None)
        };
        drop(conn);
        let format = chosen
            .structured_output
            .map(|enforced| orch_engine::ask::dialogue::Format { enforced, repair });
        let mut db = material::ReadDb::Published(&ctx.app.db);
        let material::Prepared {
            input,
            compaction,
            opaque_reusable,
            ..
        } = self
            .reading
            .prepare(
                &mut db,
                &origin,
                input,
                &chosen.parameters,
                &chosen.offering.offering,
                format,
                None,
            )
            .await?;
        let summary = compaction.as_ref().map(|pending| api::AskSummary {
            through: pending.through,
            model: api::AskModel {
                backend: chosen.model.backend.clone(),
                params: None,
            },
        });
        use orch_engine::ask::dialogue::{ProgramRequest, Request};
        let provider = chosen.offering.provider;
        let request = match chosen.protocol {
            Some(protocol) => Request::Api(ProviderGenerate {
                native_capability: None,
                capture_native_result: false,
                provider_id: provider,
                protocol,
                model_name: chosen.offering.offering,
                params: chosen.model.params,
                model_parameters: chosen.parameters,
                input,
            }),
            None => Request::Program(ProgramRequest {
                backend: chosen.model.backend,
                model_name: chosen.offering.offering,
                params: chosen.model.params,
                model_parameters: chosen.parameters,
                input,
                format: format
                    .ok_or_else(|| failed("The program output declaration is absent."))?,
                read_epochs: origin
                    .epochs
                    .as_ref()
                    .ok_or_else(|| failed("The Ask source check is missing."))?
                    .lock()
                    .map_err(failed)?
                    .clone(),
                credential_stamp: credential_stamp
                    .ok_or_else(|| failed("The Ask credential check is missing."))?,
            }),
        };

        let mut tx = ctx.app.db.begin().await?;
        if let Request::Program(native) = &request
            && !native.read_epochs.current(&mut tx).await?
        {
            return Err(failed(
                "Read access changed while preparing the answer. Retry the question.",
            ));
        }
        let row = rows::get(&mut tx, self.thread, true)
            .await?
            .ok_or_else(|| failed("Ask was deleted"))?;
        if row.turn != Some(self.turn)
            || row.runner != Some(self.owner)
            || !super::recovery::alive(row.heartbeat, ctx.app.clock.now())
        {
            return Err(failed("Ask was stopped"));
        }
        let id = new_id();
        let item = new_id();
        let seq = orch_db::sequences::next(&mut tx, self.thread)
            .await?
            .ok_or_else(|| failed("Ask was deleted"))?;
        let mut payload = json!({"text":"","text_len":0,"phase":"commentary"});
        if let Some(summary) = &summary {
            payload["ask_summary"] = serde_json::to_value(summary)?;
        }
        threads::insert_item(
            &mut tx,
            &threads::NewItem {
                id: item,
                thread_id: self.thread,
                segment_id: self.segment,
                turn_id: Some(self.turn),
                stream: "main",
                parent_item_id: None,
                kind: "agent_message",
                status: "in_progress",
                author_id: None,
                payload: &payload,
                native_ref: None,
                search_text: None,
                at: ctx.app.clock.now(),
            },
            seq,
        )
        .await?;
        rows::retain_call(
            &mut tx,
            id,
            self.thread,
            self.turn,
            item,
            self.origin.root,
            self.origin.principal.caller().user(),
            provider,
            chosen.machine,
            &request.retained()?,
            ctx.app.clock.now(),
        )
        .await?;
        let started = api::AskMessage {
            id: item,
            turn_id: self.turn,
            role: api::AskRole::Assistant,
            content: vec![],
            citations: vec![],
            summary: summary.clone(),
            complete: false,
            created_at: ctx.app.clock.time(),
        };
        rows::emit(
            &mut tx,
            self.thread,
            &serde_json::to_value(api::AskEventData::Message {
                message: Box::new(started),
            })?,
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.calls.lock().await.insert(id, item);
        self.opaque_inputs.lock().await.insert(id, opaque_reusable);
        let summarizing = compaction.is_some();
        if let Some(pending) = compaction {
            self.compactions.lock().await.insert(id, pending);
        }
        Ok(Step {
            summarizing,
            turn: self.turn,
            segment: self.segment,
            id,
            root: self.origin.root,
            thread: self.thread,
            person: self.origin.principal.caller().user(),
            machine: chosen.machine,
            request,
        })
    }

    async fn generate(
        &self,
        engine: &orch_engine::Engine,
        step: &Step,
        repair: bool,
    ) -> Result<orch_engine::ask::ModelReply, EngineError> {
        if matches!(&step.request, orch_engine::ask::dialogue::Request::Api(_)) {
            return engine.generate_ask_api(self, step).await;
        }
        let ctx = &self.origin.source;
        let source = {
            let (mut tx, guard) = transaction_guard::begin(ctx).await.map_err(failed)?;
            let raw = self
                .program_identity
                .as_ref()
                .ok_or_else(|| failed("The original Ask request is unavailable."))?;
            crate::key_operations::native::http_origin::current_preparation(ctx, &mut tx, raw)
                .await
                .map_err(failed)?;
            let owner = orch_engine::ask::command::Controller {
                thread: self.thread,
                turn: self.turn,
                runner: self.owner,
            };
            let source = orch_engine::ask::program::Source::capture(
                &mut tx,
                ctx.app.clock.as_ref(),
                owner,
                step,
                raw.clone(),
                repair,
            )
            .await?;
            guard.before_effect(ctx).map_err(failed)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            source
        };
        engine.execute_program_ask(source).await
    }

    async fn window_exceeded(&self, step: &Step) -> Result<bool, EngineError> {
        use std::sync::atomic::Ordering::Relaxed;
        let output = step.request.input().max_output_tokens;
        if output > 1 {
            // The provider refused the input plus its answer allowance. Halving the allowance converges
            // without treating a question that fits with a shorter answer as an oversized input.
            self.reduced_output.store((output / 2).max(1), Relaxed);
        } else {
            // Even the smallest answer was refused: the combined window is at most this input estimate.
            // Restore the ordinary answer allowance for the shorter request produced by compaction.
            let below =
                orch_engine::ask::context::estimate(step.request.model(), step.request.input())?;
            let known = self.learned_window.load(Relaxed);
            if below < 1 || (known > 0 && below >= known) {
                return Ok(false);
            }
            self.learned_window.store(below, Relaxed);
            self.reduced_output.store(0, Relaxed);
        }
        self.discard(step).await?;
        Ok(true)
    }

    async fn discard(&self, step: &Step) -> Result<(), EngineError> {
        // The refused step wrote no answer: its call is settled and the item that stood for its answer ends.
        let ctx = &self.origin.source;
        let item = self.calls.lock().await.get(&step.id).copied();
        let mut tx = ctx.app.db.begin().await?;
        self.lock_active(&mut tx).await?;
        rows::settle(&mut tx, step.id).await?;
        if let Some(item) = item {
            let seq = orch_db::sequences::next(&mut tx, self.thread)
                .await?
                .ok_or_else(|| failed("Ask was deleted"))?;
            threads::change_item(
                &mut tx,
                item,
                &threads::ItemChange {
                    status: Some("failed"),
                    payload: &json!({"text":"","text_len":0,"phase":"commentary"}),
                    search_text: None,
                    at: ctx.app.clock.now(),
                },
                seq,
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    async fn assistant(&self, step: &Step, output: &GenerationOutput) -> Result<(), EngineError> {
        let ctx = &self.origin.source;
        let item = *self
            .calls
            .lock()
            .await
            .get(&step.id)
            .ok_or_else(|| failed("Ask step lost its retained item"))?;
        let text: String = output
            .message
            .content
            .iter()
            .filter_map(|part| match part {
                GenerationPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let mut tx = ctx.app.db.begin().await?;
        self.lock_active(&mut tx).await?;
        let seq = orch_db::sequences::next(&mut tx, self.thread)
            .await?
            .ok_or_else(|| failed("Ask was deleted"))?;
        let citations = self.citations.lock().await.clone();
        let opaque_reusable = self
            .opaque_inputs
            .lock()
            .await
            .remove(&step.id)
            .unwrap_or(false);
        let compaction = self.compactions.lock().await.get(&step.id).cloned();
        let summary = compaction.as_ref().map(|pending| api::AskSummary {
            through: pending.through,
            model: api::AskModel {
                backend: self.model.backend.clone(),
                params: None,
            },
        });
        let mut payload = json!({"text":text,"text_len":text.chars().count(),"phase":if output.stop==orch_model_contract::generation::GenerationStop::Tools {"commentary"} else {"final"},"ask_message":output.message,"ask_citations":citations,
            "ask_origin":{"backend":self.model.backend,"protocol":self.protocol,"person":step.person,"params":self.model.params,"opaque_reusable":opaque_reusable}});
        if let Some(summary) = &summary {
            payload["ask_summary"] = serde_json::to_value(summary)?;
            payload["ask_summary_thread"] = json!(self.thread);
            payload["ask_summary_view"] = json!(
                compaction
                    .as_ref()
                    .ok_or_else(|| failed("Ask summary lost its selected view"))?
                    .view
            );
            payload["ask_summary_sources"] = serde_json::to_value(
                &compaction
                    .as_ref()
                    .ok_or_else(|| failed("Ask summary lost its source frontier"))?
                    .sources,
            )?;
        }
        threads::change_item(
            &mut tx,
            item,
            &threads::ItemChange {
                status: Some("completed"),
                payload: &payload,
                search_text: None,
                at: ctx.app.clock.now(),
            },
            seq,
        )
        .await?;
        let message = api::AskMessage {
            id: item,
            turn_id: self.turn,
            role: api::AskRole::Assistant,
            content: vec![orch_api_contract::threads::ContentPart::Text { text }],
            citations,
            summary,
            complete: true,
            created_at: ctx.app.clock.time(),
        };
        rows::emit(
            &mut tx,
            self.thread,
            &serde_json::to_value(api::AskEventData::Message {
                message: Box::new(message),
            })?,
        )
        .await?;
        let deadline = if matches!(
            &step.request,
            orch_engine::ask::dialogue::Request::Program(_)
        ) {
            Some(
                ctx.app
                    .engine
                    .guard_program_ask_publication(&mut tx, step)
                    .await?,
            )
        } else {
            None
        };
        rows::settle(&mut tx, step.id).await?;
        if deadline.is_some_and(|until| ctx.app.clock.time() >= until) {
            return Err(EngineError::StaleToolSource);
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    async fn summary(&self, step: &Step, output: &GenerationOutput) -> Result<(), EngineError> {
        use orch_model_contract::generation::GenerationStop;
        if output.stop != GenerationStop::Complete {
            return Err(EngineError::AskContextTooLong(
                step.request.model().to_owned(),
            ));
        }
        let text: String = output
            .message
            .content
            .iter()
            .filter_map(|part| match part {
                GenerationPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if text.trim().is_empty() {
            return Err(EngineError::AskContextTooLong(
                step.request.model().to_owned(),
            ));
        }
        self.assistant(step, output).await?;
        let pending = self
            .compactions
            .lock()
            .await
            .remove(&step.id)
            .ok_or_else(|| failed("Ask summary lost its original history"))?;
        if orch_engine::ask::context::material(
            step.request.model(),
            &[context::summary_message(text)],
        )? >= pending.material_tokens
        {
            return Err(EngineError::AskContextTooLong(
                step.request.model().to_owned(),
            ));
        }
        Ok(())
    }

    async fn tool(
        &self,
        id: &str,
        name: &str,
        arguments: &Json,
    ) -> Result<ToolAnswer, EngineError> {
        let result = match tools::execute(&self.origin, name, arguments).await {
            Ok((value, trace)) => {
                self.remember_citations(&value).await;
                self.pending.lock().await.insert(
                    id.into(),
                    (trace, json!({"name":name,"arguments":arguments})),
                );
                ToolAnswer {
                    content: value.to_string(),
                    is_error: false,
                }
            }
            Err(error) => {
                self.pending.lock().await.insert(
                    id.into(),
                    (None, json!({"name":name,"arguments":arguments})),
                );
                ToolAnswer {
                    content: error.problem.detail,
                    is_error: true,
                }
            }
        };
        Ok(result)
    }

    async fn tool_answer(&self, id: &str, answer: &ToolAnswer) -> Result<Option<Id>, EngineError> {
        let ctx = &self.origin.source;
        let (trace, call) = self
            .pending
            .lock()
            .await
            .remove(id)
            .unwrap_or((None, json!({"name":"read","arguments":{}})));
        let mut tx = ctx.app.db.begin().await?;
        self.lock_active(&mut tx).await?;
        let seq = orch_db::sequences::next(&mut tx, self.thread)
            .await?
            .ok_or_else(|| failed("Ask was deleted"))?;
        let witness = new_id();
        let generation = GenerationMessage {
            role: GenerationRole::User,
            content: vec![GenerationPart::ToolResult {
                call_id: id.into(),
                content: answer.content.clone(),
                is_error: answer.is_error,
                witness: Some(witness),
            }],
            continuation: vec![],
        };
        let input = orch_api_contract::threads::OutputRef::of(&call["arguments"].to_string());
        let output = orch_api_contract::threads::OutputRef::of(&answer.content);
        let payload = json!({"tool":{"kind":"mcp","server":"orch","tool":call["name"],"arguments":call["arguments"],"result":answer.content,"error":if answer.is_error {Some("Source unavailable")} else {None}},"title":call["name"],"started_at":ctx.app.clock.time(),"duration_ms":null,"raw":{"name":call["name"],"input":input,"output":output,"is_error":answer.is_error},"ask_message":generation,"ask_call_id":id,"ask_trace":trace,"ask_witness":witness});
        threads::insert_item(
            &mut tx,
            &threads::NewItem {
                id: witness,
                thread_id: self.thread,
                segment_id: self.segment,
                turn_id: Some(self.turn),
                stream: "main",
                parent_item_id: None,
                kind: "tool_call",
                status: "completed",
                author_id: None,
                payload: &payload,
                native_ref: Some(&format!("ask:{}:{witness}", self.turn)),
                search_text: None,
                at: ctx.app.clock.now(),
            },
            seq,
        )
        .await?;
        rows::emit(
            &mut tx,
            self.thread,
            &json!({"kind":"tool","call_id":id,"name":call["name"],"complete":true,"citations":[]}),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Some(witness))
    }
}

#[path = "runner/context.rs"]
pub mod context;
