//! Interactive built-in programs use the native thread creator and its original guarded request.

use super::*;

use crate::key_operations::admission::Prepared;

use crate::key_operations::blob_sources;

use crate::key_operations::targets;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::nodes::AgentAccessWhere;

use orch_api_contract::nodes::CreateThread;

use orch_api_contract::nodes::ThreadCreateResponse;

use orch_api_contract::share::ShareParts;

use orch_api_contract::threads::ContentPart;
#[derive(Clone)]
struct Choice {
    topic: Id,
    root: Id,
    plan: Prepared,
}

fn validate(body: &CreateThread) -> ApiResult<()> {
    for (present, field) in [
        (body.binding.is_some(), "binding"),
        (body.environment.is_some(), "environment"),
        (body.let_into_environment.is_some(), "let_into_environment"),
        (body.directory.is_some(), "directory"),
        (body.worktree.is_some(), "worktree"),
        (body.prepare_source.is_some(), "prepare_source"),
        (body.cwd.is_some(), "cwd"),
        (body.model.is_some(), "model"),
        (body.effort.is_some(), "effort"),
        (body.params.is_some(), "params"),
    ] {
        if present {
            return Err(ApiError::invalid(
                field,
                "This program uses an API model and has no native workspace or agent binding",
            ));
        }
    }
    if body.agent_access.is_some_and(|choice| {
        choice.where_ != AgentAccessWhere::Nothing || choice.parts != ShareParts::default()
    }) {
        return Err(ApiError::invalid(
            "agent_access",
            "This read-only program reads as the person asking, without an agent contribution",
        ));
    }
    Ok(())
}

/// Bind only actual creation/input operands; no environment or agent singleton exists for this program.
pub async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &orch_authz::Caller,
    topic: Id,
    body: &CreateThread,
    mut plan: Prepared,
) -> ApiResult<Prepared> {
    validate(body)?;
    let target = targets::node(conn, topic).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Err(ApiError::not_found());
    }
    plan.bind(0, KeyTargetKind::Object(ObjectKind::Thread), &[target])?;
    for role in [
        "creator_new_environment_homes",
        "creator_initial_file_threads",
        "creator_initial_file_environments",
        "creator_file_environments",
        "creator_reader_environments",
        "creator_file_sources",
        "creator_agent_projects",
        "creator_agent_topics",
        "creator_agent_threads",
    ] {
        let index = plan
            .variant()
            .facets
            .iter()
            .position(|facet| {
                facet.target
                    == orch_api_contract::operations::TargetResolver::ResolvedCollection(role)
            })
            .ok_or_else(|| ApiError::internal("The thread creation operand is not declared"))?;
        plan.bind_empty(index)?;
    }
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let policy = crate::routes::nodes::policy_at(conn, topic).await?;
    authorization::require(
        conn,
        &principal,
        topic,
        authorization::exact(Permission::TopicStartThreads),
        Act::Use,
        policy,
    )
    .await?;
    let root = hierarchy::node(conn, topic)
        .await?
        .ok_or_else(ApiError::not_found)?
        .org_id;
    root_access(conn, &principal, root).await?;
    let program = body
        .program
        .as_ref()
        .ok_or_else(|| ApiError::invalid("program", "Choose a built-in program"))?;
    match program.id {
        orch_api_contract::threads::BuiltinThreadProgramId::Orch => {}
    }
    if let Some(model) = &program.model {
        let choices = crate::routes::threads::binding::ask_models(conn, &principal, root).await?;
        if !choices.iter().any(|choice| {
            choice.selection.provider_id == model.provider_id
                && choice.selection.model == model.model
                && choice.allowed
        }) {
            return Err(ApiError::invalid(
                "program.model",
                "Choose an allowed API model in this organization",
            ));
        }
    }
    let hashes = body
        .first_input
        .as_ref()
        .map(|first| orch_chat::following::content_blobs(&first.content))
        .unwrap_or_default();
    let sources = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &[]).await?;
    blob_sources::bind(&mut plan, &sources.bindings)?;
    Ok(plan.clone().with_data(Choice { topic, root, plan }))
}

async fn files(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    thread: Id,
    content: &[ContentPart],
) -> ApiResult<()> {
    let mut names = std::collections::HashMap::new();
    for part in content {
        match part {
            ContentPart::Image { blob, name, .. }
            | ContentPart::File {
                blob: Some(blob),
                name,
                ..
            } => {
                names.entry(*blob).or_insert_with(|| name.clone());
            }
            ContentPart::PastedText(paste) => {
                names
                    .entry(paste.blob)
                    .or_insert_with(|| paste.name.clone());
            }
            ContentPart::Forwarded(message) => {
                for image in &message.images {
                    names
                        .entry(image.blob)
                        .or_insert_with(|| image.name.clone());
                }
                for file in &message.attachments {
                    names.entry(file.blob).or_insert_with(|| file.name.clone());
                }
            }
            ContentPart::File { blob: None, .. } => {
                return Err(ApiError::invalid(
                    "first_input.content",
                    "Attach a stored file; this program has no workspace path",
                ));
            }
            ContentPart::Text { .. } => {}
        }
    }
    let limit = setting::<i64>(conn, Place::Node(root), "chat.attachment_max_bytes")
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(i64::MAX);
    for (blob, name) in names {
        let meta = orch_db::queries::storage::blob(conn, &blob)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if meta.size > limit {
            return Err(ApiError::new(
                ErrorCode::RequestTooLarge,
                "The file exceeds the ordinary attachment limit",
            )
            .with("limit", limit));
        }
        let taken = crate::attachments::Taken {
            meta: orch_blob_contract::range::BlobMeta {
                hash: blob,
                size: meta.size,
            },
            name: crate::uploads::clean_name("first_input.content.name", &name)?,
            supplied_bytes: false,
        };
        // Retain through the original native source witness before inspecting any stored bytes.
        crate::attachments::retain(ctx, conn, principal, &taken).await?;
        let described = crate::attachments::describe(ctx, &taken.meta, &taken.name).await?;
        rows::attach(
            conn,
            thread,
            &blob,
            &taken.name,
            &described.media_type,
            meta.size,
            ctx.now,
        )
        .await?;
    }
    Ok(())
}

pub async fn create(ctx: Ctx, body: CreateThread) -> ApiResult<ThreadCreateResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let plan = prepare(
        &ctx,
        &mut tx,
        guard.principal().caller(),
        ctx.id("id")?,
        &body,
        Prepared::new(ctx.route, "ThreadCreate")?,
    )
    .await?;
    let admitted = plan.admit(guard.principal().caller())?;
    let choice = admitted.data.get::<Choice>()?;
    let principal = guard.principal();
    let person = guard.user().id();
    let name = body
        .name
        .as_deref()
        .map(|name| crate::routes::nodes::clean_name("name", name))
        .transpose()?
        .unwrap_or_else(|| crate::routes::threads::make::NEW_THREAD.into());
    guard.check_deadline(&ctx)?;
    let thread = new_id();
    if !threads::insert_thread(&mut tx, thread, choice.topic, &name, person, ctx.now).await? {
        return Err(ApiError::not_found());
    }
    orch_authz::pages::start(
        &mut tx,
        orch_access_contract::pages::StoredKind::Node,
        thread,
        person,
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    if let Some(page) = crate::routes::threads::make::page_of(body.access.as_ref())? {
        let policy = crate::routes::nodes::policy_at(&mut tx, thread).await?;
        let changed = orch_authz::pages::change(
            &mut tx,
            principal,
            orch_authz::pages::Target::Live(thread),
            &page,
            policy,
            &orch_db::audit::current_origin(),
        )
        .await
        .map_err(ApiError::internal)?;
        if !changed.skipped.is_empty() {
            return Err(ApiError::invalid(
                "access",
                "A locked access rule prevents this thread's requested audience",
            ));
        }
    }
    if body.name.is_none() {
        threads::leave_title_to_automation(&mut tx, thread).await?;
    }
    let model = body
        .program
        .as_ref()
        .and_then(|program| program.model.as_ref())
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    rows::bind_kept(&mut tx, thread, choice.root, model.as_ref(), None, ctx.now).await?;
    orch_engine::binding::finish_thread_birth(&mut tx, thread, choice.topic, person, ctx.now)
        .await
        .map_err(ApiError::engine)?;
    let pending = if let Some(first) = &body.first_input {
        files(
            &ctx,
            &mut tx,
            principal,
            choice.root,
            thread,
            &first.content,
        )
        .await?;
        Some(
            super::runner::accept_in(
                &ctx,
                &mut tx,
                &guard,
                thread,
                api::AskMessageBody {
                    client_id: first.client_id.clone(),
                    content: first.content.clone(),
                    context: None,
                    model: None,
                },
            )
            .await?,
        )
    } else {
        None
    };
    let receipt = crate::routes::threads::creation_operation::receipt(
        &mut tx,
        person,
        thread,
        choice.topic,
        body.first_input.as_ref(),
    )
    .await?;
    orch_db::idempotency::creation::observe_with(
        &mut tx,
        Some(person),
        "thread",
        thread,
        json!({"thread":thread,"topic":choice.topic,"input":receipt.input_id}),
    )
    .await?;
    let shown = crate::routes::threads::creation_operation::read_program(
        &ctx,
        &mut tx,
        principal,
        choice.topic,
        &choice.plan,
        thread,
    )
    .await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if let Some(pending) = pending {
        pending.start(&ctx.app);
    }
    Ok(match shown {
        Some(thread) if !ctx.key_receipt_required() => ThreadCreateResponse::full(thread),
        _ => ThreadCreateResponse::Receipt(receipt),
    })
}
