//! Actual schedule subjects and destinations, shared with the existing due-work producer.

use super::*;

use crate::key_operations::admission::binding_error;

use crate::key_operations::blob_sources;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationInitialFuture;

use crate::registry::OperationPlan;

use crate::routes::access::page::NameBounds;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;

use orch_engine::operation_scope::Prepared as SharedPlan;

use std::collections::BTreeSet;

use std::sync::Arc;
const SCHEDULE: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Schedule);

pub(super) fn definition(body: &InputScheduleUpdateBody) -> bool {
    body.name.is_some()
        || body.target.is_some()
        || body.content.is_some()
        || body.native_action.is_some()
        || body.timing.is_some()
}

pub(super) fn selection(body: &InputScheduleUpdateBody) -> &'static str {
    if definition(body) {
        "InputScheduleUpdate.definition"
    } else {
        "InputScheduleUpdate.state"
    }
}

fn slot(plan: &SharedPlan, role: TargetResolver) -> ApiResult<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| facet.target == role)
        .ok_or_else(|| ApiError::internal("the schedule target role is not declared"))
}

pub(super) async fn subject(
    conn: &mut Conn,
    row: &rows::ScheduleRow,
) -> ApiResult<KeyResolvedTarget> {
    let mut target = targets::node(conn, row.topic_id).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Err(ApiError::not_found());
    }
    target.object = KeyObjectRef {
        kind: SCHEDULE,
        id: Some(row.id),
    };
    Ok(target)
}

async fn primary(plan: &mut SharedPlan, conn: &mut Conn, row: &rows::ScheduleRow) -> ApiResult<()> {
    let target = if plan.variant().operation == "TopicInputScheduleCreate" {
        let topic = targets::node(conn, row.topic_id).await?;
        if topic.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
            return Err(ApiError::not_found());
        }
        topic
    } else {
        subject(conn, row).await?
    };
    let indices: Vec<_> = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(i, f)| (f.target == TargetResolver::Path("id")).then_some(i))
        .collect();
    for i in indices {
        plan.bind(i, SCHEDULE, std::slice::from_ref(&target))
            .map_err(binding_error)?;
    }
    Ok(())
}

pub(super) async fn definition_base(
    conn: &mut Conn,
    row: &rows::ScheduleRow,
    selected: &str,
) -> ApiResult<SharedPlan> {
    let mut plan = SharedPlan::for_selection(selected).map_err(binding_error)?;
    primary(&mut plan, conn, row).await?;
    let destination = targets::schedule_destination(conn, row.topic_id, &target(row)?).await?;
    if let InputScheduleTarget::Thread { thread_id } = target(row)?
        && !threads::thread(conn, thread_id)
            .await?
            .is_some_and(|t| t.topic_id == row.topic_id && t.deleted_at.is_none())
    {
        return Err(ApiError::not_found());
    }
    let index = slot(&plan, TargetResolver::Resolved("scheduled_destinations"))?;
    plan.bind(
        index,
        KeyTargetKind::Object(ObjectKind::Thread),
        std::slice::from_ref(&destination),
    )
    .map_err(binding_error)?;
    Ok(plan)
}

/// Both HTTP acceptance and due claims use current native classification and target facts.
/// Stored-byte source witnesses are completed by the shared planner for each actual caller.
pub(super) async fn definition_plan(
    app: &crate::state::AppState,
    conn: &mut Conn,
    row: &rows::ScheduleRow,
    selected: &str,
) -> ApiResult<SharedPlan> {
    let mut plan = definition_base(conn, row, selected).await?;
    let destination = targets::schedule_destination(conn, row.topic_id, &target(row)?).await?;
    let permission = native_permission(conn, row).await?;
    orch_engine::inputs::operation::bind_native_collection(
        &mut plan,
        "scheduled_management",
        &destination,
        permission,
    )
    .map_err(binding_error)?;
    bind_files(app, conn, row, &mut plan).await?;
    Ok(plan)
}

fn bind_no_files(plan: &mut SharedPlan) -> ApiResult<()> {
    for role in [
        "creator_file_environments",
        "creator_file_sources",
        "creator_reader_environments",
    ] {
        let index = slot(plan, TargetResolver::ResolvedCollection(role))?;
        plan.bind_empty(index).map_err(binding_error)?;
    }
    Ok(())
}

/// An existing-thread definition performs no creator source preparation.
async fn bind_files(
    app: &crate::state::AppState,
    conn: &mut Conn,
    row: &rows::ScheduleRow,
    plan: &mut SharedPlan,
) -> ApiResult<()> {
    let InputScheduleTarget::NewThread { config } = target(row)? else {
        return bind_no_files(plan);
    };
    let (new, birth) = crate::routes::threads::make::scheduled_choices(
        app,
        conn,
        row.topic_id,
        row.author_id,
        &config,
        &row.name,
    )
    .await?;
    let environment =
        orch_db::queries::environments::thread_engine::environment(conn, new.environment)
            .await?
            .ok_or_else(ApiError::not_found)?;
    let source = orch_engine::jobs::source::resolved(conn, row.topic_id)
        .await
        .map_err(ApiError::engine)?;
    let worktree = birth.worktree.is_some();
    let pending = orch_engine::jobs::source::preparation_pending(
        birth.prepare_source,
        worktree,
        source.repository_url.is_some(),
        environment.status == "running",
    );
    orch_engine::operation_scope::creator_files::bind(
        conn,
        plan,
        new.environment,
        pending,
        worktree,
    )
    .await
    .map_err(ApiError::engine)?;
    orch_engine::operation_scope::creator_files::bind_readers(
        conn,
        plan,
        new.environment,
        birth.let_into_environment,
    )
    .await
    .map_err(ApiError::engine)
}

async fn native_permission(
    conn: &mut Conn,
    row: &rows::ScheduleRow,
) -> ApiResult<Option<orch_api_contract::native::NativePermission>> {
    let native: Option<NativeInvocation> = row.native_action.clone().map(decode).transpose()?;
    let permission = match (target(row)?, native.as_ref()) {
        (InputScheduleTarget::Thread { thread_id }, Some(selected)) => {
            match orch_engine::native_actions::selected_action(
                conn,
                thread_id,
                selected,
                Delivery::Queue,
            )
            .await
            {
                Ok(action) => Some(action.permission),
                Err(error) => return Err(crate::routes::threads::native_failed(conn, error).await),
            }
        }
        _ => None,
    };
    Ok(permission)
}

fn draft(
    ctx: &Ctx,
    caller: &Caller,
    body: &InputScheduleCreateBody,
) -> ApiResult<rows::ScheduleRow> {
    Ok(rows::ScheduleRow {
        id: new_id(),
        topic_id: ctx.id("id")?,
        author_id: caller.user(),
        name: body
            .name
            .clone()
            .unwrap_or_else(|| "Scheduled input".into()),
        target: encode(&body.target)?,
        content: encode(&body.content)?,
        native_action: body.native_action.as_ref().map(encode).transpose()?,
        definition_authority: None,
        timing: encode(&body.timing)?,
        state: "active".into(),
        next_run_at: None,
        revision: 1,
        topic_seq: 0,
        created_at: ctx.now,
        updated_at: ctx.now,
        deleted_at: None,
    })
}

pub(super) fn apply(row: &mut rows::ScheduleRow, body: &InputScheduleUpdateBody) -> ApiResult<()> {
    if let Some(value) = &body.name {
        row.name = name(value)?;
    }
    if let Some(value) = &body.target {
        row.target = encode(value)?;
    }
    if let Some(value) = &body.content {
        row.content = encode(value)?;
    }
    if let Some(value) = &body.native_action {
        row.native_action = value.as_ref().map(encode).transpose()?;
    }
    if let Some(value) = &body.timing {
        row.timing = encode(value)?;
    }
    Ok(())
}

async fn prepare_definition(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: &rows::ScheduleRow,
    selected: &str,
) -> ApiResult<OperationPlan> {
    if selected == "InputScheduleUpdate.definition" && row.author_id != caller.user() {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only its author edits the prepared definition",
        ));
    }
    let mut plan = OperationPlan::new(ctx.route, selected)?;
    *plan.shared_mut() = definition_base(conn, row, selected).await?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let (destination, permission) = match target(row)? {
        InputScheduleTarget::Thread { thread_id } => (thread_id, Permission::ThreadInteract),
        InputScheduleTarget::NewThread { .. } => (row.topic_id, Permission::TopicStartThreads),
    };
    authorization::require(
        conn,
        &principal,
        destination,
        authorization::exact(permission),
        Act::Use,
        Policy::default(),
    )
    .await?;
    // Ordinary current authority precedes native catalogue validation/error projection.
    *plan.shared_mut() = definition_plan(&ctx.app, conn, row, selected).await?;
    let sources = blob_sources::prepare(
        ctx,
        conn,
        caller,
        &plan,
        &orch_chat::following::content_blobs(&content(row)?),
        &[],
    )
    .await?;
    blob_sources::bind(&mut plan, &sources.bindings)?;
    let permission = native_permission(conn, row).await?;
    Ok(plan.with_data(orch_engine::inputs::InputSourceOperands {
        blobs: sources.selected,
        replayed: None,
        bypass_subjects: Vec::new(),
        native_permission: permission,
    }))
}

pub(super) fn create<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: InputScheduleCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        authored(ctx, caller)?;
        let row = draft(ctx, caller, &body)?;
        prepare_definition(ctx, conn, caller, &row, "TopicInputScheduleCreate").await
    })
}

pub(super) fn update<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: InputScheduleUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        if definition(&body) || body.state == Some(InputScheduleState::Active) {
            authored(ctx, caller)?;
        }
        let mut row = rows::schedule(conn, ctx.id("id")?, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let selected = selection(&body);
        if definition(&body) {
            if super::agent::metadata(&row)?.is_some() {
                return Err(ApiError::invalid(
                    "definition",
                    "control or cancel this agent schedule instead of replacing its native author",
                ));
            }
            // Preserve the ordinary stale-version refusal before validating replacement content.
            if row.revision == body.revision {
                apply(&mut row, &body)?;
                return prepare_definition(ctx, conn, caller, &row, selected).await;
            }
            let mut plan = OperationPlan::new(ctx.route, selected)?;
            *plan.shared_mut() = definition_base(conn, &row, selected).await?;
            let dest = targets::schedule_destination(conn, row.topic_id, &target(&row)?).await?;
            orch_engine::inputs::operation::bind_native_collection(
                plan.shared_mut(),
                "scheduled_management",
                &dest,
                None,
            )
            .map_err(binding_error)?;
            blob_sources::bind(&mut plan, &[])?;
            // A stale edit returns its conflict and performs no source effects.
            bind_no_files(plan.shared_mut())?;
            return Ok(plan);
        }
        let mut plan = OperationPlan::new(ctx.route, selected)?;
        primary(plan.shared_mut(), conn, &row).await?;
        Ok(plan)
    })
}

pub(super) fn delete<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = rows::schedule(conn, ctx.id("id")?, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        primary(plan.shared_mut(), conn, &row).await?;
        Ok(plan)
    })
}

async fn initial_row(
    ctx: &Ctx,
    caller: &Caller,
    row: rows::ScheduleRow,
    selected: &str,
) -> ApiResult<()> {
    if row.native_action.is_none() {
        return Ok(());
    }
    let InputScheduleTarget::Thread { thread_id } = target(&row)? else {
        return Ok(());
    };
    let prepared_ctx = {
        let mut conn = ctx.app.db.acquire().await?;
        let mut plan = OperationPlan::new(ctx.route, selected)?;
        *plan.shared_mut() = definition_base(&mut conn, &row, selected).await?;
        let principal = plan.preparation_principal(ctx, &mut conn, caller).await?;
        if selected != "TopicInputScheduleCreate" {
            controller(&mut conn, &principal, &row).await?;
        }
        ctx.clone()
            .with_admitted_caller(principal.caller().clone())?
    };
    crate::routes::threads::native::refresh_for_input(&prepared_ctx, thread_id).await
}

pub(super) fn initial_create<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    (): (),
    body: InputScheduleCreateBody,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        authored(ctx, caller)?;
        let row = draft(ctx, caller, &body)?;
        initial_row(ctx, caller, row, "TopicInputScheduleCreate").await
    })
}

pub(super) fn initial_update<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    (): (),
    body: InputScheduleUpdateBody,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        if !definition(&body) && body.state != Some(InputScheduleState::Active) {
            return Ok(());
        }
        authored(ctx, caller)?;
        let mut row = {
            let mut conn = ctx.app.db.acquire().await?;
            rows::schedule(&mut conn, ctx.id("id")?, false)
                .await?
                .ok_or_else(ApiError::not_found)?
        };
        if row.revision != body.revision || (!definition(&body) && row.state == "active") {
            return Ok(());
        }
        if definition(&body) {
            if row.author_id != caller.user() {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "only its author edits the prepared definition",
                ));
            }
            apply(&mut row, &body)?;
            initial_row(ctx, caller, row, selection(&body)).await
        } else {
            // Resume validates the stored selection but remains a completed controller action.
            if row.native_action.is_none() {
                return Ok(());
            }
            let InputScheduleTarget::Thread { thread_id } = target(&row)? else {
                return Ok(());
            };
            let prepared_ctx = {
                let mut conn = ctx.app.db.acquire().await?;
                let mut plan = OperationPlan::new(ctx.route, selection(&body))?;
                primary(plan.shared_mut(), &mut conn, &row).await?;
                let principal = plan.preparation_principal(ctx, &mut conn, caller).await?;
                controller(&mut conn, &principal, &row).await?;
                ctx.clone()
                    .with_admitted_caller(principal.caller().clone())?
            };
            crate::routes::threads::native::refresh_for_input(&prepared_ctx, thread_id).await
        }
    })
}

#[derive(Clone)]
pub struct Reading {
    pub rows: Vec<rows::ScheduleRow>,
    pub runs: Vec<rows::RunRow>,
    pub names: NameBounds,
}

async fn names(
    plan: &mut OperationPlan,
    conn: &mut Conn,
    caller: &Caller,
    ids: &[Id],
) -> ApiResult<NameBounds> {
    let graph = orch_authz::engine::Graph::load(conn, ids)
        .await
        .map_err(ApiError::internal)?;
    let mut bounds = NameBounds {
        limited: caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some()),
        ..NameBounds::default()
    };
    for (kind, role) in [
        (ObjectKind::Topic, "schedule_name_topics"),
        (ObjectKind::Thread, "schedule_name_threads"),
    ] {
        let index = slot(plan.shared(), TargetResolver::ResolvedCollection(role))?;
        let mut facts = Vec::new();
        for id in ids.iter().copied().collect::<BTreeSet<_>>() {
            if let Some(fact) =
                orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                && fact.object.kind == KeyTargetKind::Object(kind)
                && plan.permits_binding(
                    caller,
                    index,
                    fact.object.kind,
                    std::slice::from_ref(&fact),
                )?
            {
                bounds.nodes.insert(id);
                facts.push(fact);
            }
        }
        if facts.is_empty() {
            plan.bind_empty(index)?;
        } else {
            plan.bind(index, KeyTargetKind::Object(kind), &facts)?;
        }
    }
    Ok(bounds)
}

fn named_ids(rows: &[rows::ScheduleRow], runs: &[rows::RunRow]) -> ApiResult<Vec<Id>> {
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.topic_id);
        if let InputScheduleTarget::Thread { thread_id } = target(row)? {
            ids.push(thread_id);
        }
    }
    ids.extend(runs.iter().filter_map(|run| run.thread_id));
    Ok(ids)
}

pub(super) fn read<'a, Q: Send + 'static>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: Q,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let listing = ctx.route.name == "TopicInputScheduleList";
        let mut found = if listing {
            let topic = ctx.id("id")?;
            let fact = targets::node(conn, topic).await?;
            if fact.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
                return Err(ApiError::not_found());
            }
            plan.bind(0, KeyTargetKind::Object(ObjectKind::Topic), &[fact])?;
            rows::topic(conn, topic, None).await?
        } else {
            let row = rows::schedule(conn, ctx.id("id")?, false)
                .await?
                .ok_or_else(ApiError::not_found)?;
            primary(plan.shared_mut(), conn, &row).await?;
            vec![row]
        };
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let mut kept = Vec::new();
        let mut facts = Vec::new();
        for row in found.drain(..) {
            if !readable(conn, &principal, &row).await? {
                continue;
            }
            if listing {
                let fact = subject(conn, &row).await?;
                if !plan.permits_binding(caller, 1, SCHEDULE, std::slice::from_ref(&fact))? {
                    continue;
                }
                facts.push(fact);
            }
            kept.push(row);
        }
        if !listing && kept.is_empty() {
            return Err(ApiError::not_found());
        }
        if listing {
            if facts.is_empty() {
                plan.bind_empty(1)?;
            } else {
                plan.bind(1, SCHEDULE, &facts)?;
            }
        }
        let mut runs = Vec::new();
        for row in &kept {
            runs.extend(rows::runs(conn, row.id).await?);
        }
        let bounds = names(&mut plan, conn, caller, &named_ids(&kept, &runs)?).await?;
        Ok(plan.with_data(Reading {
            rows: kept,
            runs,
            names: bounds,
        }))
    })
}

/// Response reads use fresh actual identities and the original/current caller intersection.
pub(super) async fn full(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: rows::ScheduleRow,
) -> ApiResult<Option<InputSchedule>> {
    full_as(ctx, conn, caller, row, false)
        .await
        .map(|value| value.map(|(body, _)| body))
}

pub(super) async fn replay_full(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: rows::ScheduleRow,
) -> ApiResult<Option<(InputSchedule, ReplaySource)>> {
    full_as(ctx, conn, caller, row, true)
        .await?
        .map(|(body, source)| {
            source
                .map(|source| (body, source))
                .ok_or_else(|| ApiError::internal("the native schedule replay lost its source"))
        })
        .transpose()
}

#[derive(Clone)]
pub struct ReplaySource {
    row: rows::ScheduleRow,
    names: NameBounds,
}

pub(super) fn retain_replay(ctx: &Ctx, caller: &Caller, source: ReplaySource) -> ApiResult<()> {
    crate::key_operations::publication::retain(
        ctx,
        caller,
        Arc::new(source),
        |ctx, conn, reader, source| Box::pin(check_replay(ctx, conn, reader, source)),
    )?;
    Ok(())
}

async fn check_replay(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    source: &ReplaySource,
) -> ApiResult<()> {
    let replay = ctx.for_completed_replay_projection();
    let ctx = &replay;
    let row = rows::schedule(conn, source.row.id, false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.id != source.row.id
        || row.topic_id != source.row.topic_id
        || row.author_id != source.row.author_id
        || row.revision != source.row.revision
        || row.topic_seq != source.row.topic_seq
        || row.deleted_at != source.row.deleted_at
    {
        return Err(ApiError::not_found());
    }
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let target = subject(conn, &row).await?;
    let root: Vec<_> = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| {
            (facet.usage == FacetUsage::ResponseContent && facet.resources == [SCHEDULE])
                .then_some(index)
        })
        .collect();
    for index in root {
        plan.bind(index, SCHEDULE, std::slice::from_ref(&target))?;
    }
    let runs = rows::runs(conn, row.id).await?;
    let current = names(
        &mut plan,
        conn,
        reader.caller(),
        &named_ids(std::slice::from_ref(&row), &runs)?,
    )
    .await?;
    if !source.names.nodes.is_subset(&current.nodes) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the schedule response names are no longer readable",
        ));
    }
    let indices = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| (facet.usage == FacetUsage::ResponseContent).then_some(index))
        .collect();
    let nodes = source.names.nodes.iter().copied().collect::<Vec<_>>();
    let checking = ctx.clone();
    plan.read_response_projection(
        ctx,
        conn,
        reader.caller(),
        crate::key_operations::admission::ResponseProjection::Replay(indices),
        move |conn, reader| {
            Box::pin(async move {
                if !readable(conn, reader, &row).await? {
                    return Err(ApiError::not_found());
                }
                checking.publication.retain_ceiling(reader.caller())?;
                let visible =
                    view::reference_visibility(conn, reader, &nodes, Policy::default()).await?;
                if !nodes.iter().all(|id| visible.contains(id)) {
                    return Err(ApiError::not_found());
                }
                Ok(())
            })
        },
    )
    .await?
    .ok_or_else(ApiError::not_found)
}

async fn full_as(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: rows::ScheduleRow,
    replay: bool,
) -> ApiResult<Option<(InputSchedule, Option<ReplaySource>)>> {
    let selected = caller.admitted_operation().unwrap_or(ctx.route.name);
    let mut plan = OperationPlan::new(ctx.route, selected)?;
    let target = subject(conn, &row).await?;
    let slots: Vec<_> = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            (f.usage == FacetUsage::ResponseContent && f.resources == [SCHEDULE]).then_some(i)
        })
        .collect();
    for i in slots {
        plan.bind(i, SCHEDULE, std::slice::from_ref(&target))?;
    }
    // A response performs no creator file preparation. Complete these read slots only
    // for the definition variants that declare them; state-only controls have none.
    if !replay
        && plan.variant().facets.iter().any(|facet| {
            facet.target == TargetResolver::ResolvedCollection("creator_file_environments")
        })
    {
        bind_no_files(plan.shared_mut())?;
    }
    // Projection returns retained authored content, without a new external blob retrieval.
    if !replay
        && plan
            .variant()
            .facets
            .iter()
            .any(|f| f.target == TargetResolver::ResolvedCollection("message_source_chats"))
    {
        blob_sources::bind(&mut plan, &[])?;
    }
    let runs = rows::runs(conn, row.id).await?;
    let bounds = names(
        &mut plan,
        conn,
        caller,
        &named_ids(std::slice::from_ref(&row), &runs)?,
    )
    .await?;
    if replay {
        let indices = plan
            .variant()
            .facets
            .iter()
            .enumerate()
            .filter_map(|(index, facet)| {
                (facet.usage == FacetUsage::ResponseContent).then_some(index)
            })
            .collect();
        let frozen = row.clone();
        let frozen_names = bounds.clone();
        let observing = ctx.clone();
        let result = plan
            .read_response_projection(
                ctx,
                conn,
                caller,
                crate::key_operations::admission::ResponseProjection::Replay(indices),
                move |conn, principal| {
                    Box::pin(async move {
                        if !readable(conn, principal, &row).await? {
                            return Err(ApiError::not_found());
                        }
                        observing.publication.retain_ceiling(principal.caller())?;
                        present_bounded(conn, principal, row, Some(&bounds)).await
                    })
                },
            )
            .await?;
        Ok(result.map(|value| {
            (
                value,
                Some(ReplaySource {
                    row: frozen,
                    names: frozen_names,
                }),
            )
        }))
    } else {
        plan.read_response(ctx, conn, caller, move |conn, principal| {
            Box::pin(async move {
                // Ordinary first writes keep their existing controller/author projection.
                controller(conn, principal, &row).await?;
                present_bounded(conn, principal, row, Some(&bounds)).await
            })
        })
        .await
        .map(|value| value.map(|value| (value, None)))
    }
}

pub(super) fn receipt(row: &rows::ScheduleRow) -> InputScheduleReceipt {
    InputScheduleReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        schedule_id: row.id,
        revision: row.revision,
        topic_seq: row.topic_seq,
    }
}
