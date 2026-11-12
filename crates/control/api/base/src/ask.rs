//! Personal Ask conversations, governed by their fixed root organization.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::authorization;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_access_contract::decisions::Decision;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::ask as api;

use orch_api_contract::common::ErrorCode;

use orch_authz::Act;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::ask as rows;

use orch_db::queries::hierarchy;

use orch_db::queries::threads;

use orch_settings::Place;

use orch_settings::get as setting;

use orch_types::Id;

use orch_types::Json;

use orch_types::Seq;

use orch_types::Time;

use orch_types::new_id;

use serde_json::json;

use std::sync::Arc;
pub(crate) fn denied(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::AuthzDenied, message)
}

pub(crate) fn mount(registry: &mut Registry) {
    registry
        .creation(api::AskCreate, creation::conversation())
        .creation(api::AskKeep, creation::keep());
    registry
        .on_with_operation(api::AskAttachmentCreate, files::prepare, files::create)
        .on_raw_read_with_operation(
            api::AskAttachmentGet,
            crate::key_operations::direct::declared,
            files::get,
        );
    registry.on_snapshot_read_with_optional_operation(
        api::AskModelsGet,
        models::prepare,
        models::get,
    );
    let _ = registry
        .on_with_operation(
            api::AskCreate,
            crate::key_operations::direct::declared,
            create,
        )
        .on_with_operation(api::AskList, prepare_list, list)
        .on_with_operation(api::AskGet, crate::key_operations::direct::declared, get)
        .on_with_operation(api::AskMessageCreate, runner::prepare, message)
        .on_raw_read_with_operation(
            api::AskEvents,
            crate::key_operations::direct::declared,
            events,
        )
        .on_with_operation(api::AskStop, crate::key_operations::direct::declared, stop)
        .on_with_operation(
            api::AskDelete,
            crate::key_operations::direct::declared,
            delete,
        )
        .on_with_operation(api::AskKeep, keep::prepare, keep::keep)
        .on_with_operation(
            api::AskUpdate,
            crate::key_operations::direct::declared,
            update,
        )
        .on_raw_read_with_operation(
            api::AskExport,
            crate::key_operations::direct::declared,
            export::export,
        );
}

pub(crate) async fn root_access(conn: &mut Conn, principal: &Principal, root: Id) -> ApiResult<()> {
    use orch_engine::ask::admission::RootAccess;
    match orch_engine::ask::admission::root_access(conn, principal, root)
        .await
        .map_err(|error| match error {
            orch_engine::EngineError::Db(error) => ApiError::from(error),
            other => ApiError::internal(other),
        })? {
        RootAccess::Allowed => Ok(()),
        RootAccess::Missing => Err(ApiError::not_found()),
        RootAccess::Refused => Err(denied(
            "You can no longer open this conversation's organization",
        )),
    }
}

pub(crate) async fn administers(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
) -> ApiResult<bool> {
    if !principal.caller().as_admin() {
        return Ok(false);
    }
    let graph = orch_authz::engine::Graph::load(conn, &[root])
        .await
        .map_err(ApiError::internal)?;
    let site = graph.site_of(root).ok_or_else(ApiError::not_found)?;
    let policy = crate::routes::nodes::policy_at(conn, root).await?;
    let frame = orch_authz::engine::Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &[Permission::OrgView],
    )
    .await
    .map_err(ApiError::internal)?;
    Ok(orch_authz::admins::administers(&frame, &site))
}

pub(crate) async fn retention(conn: &mut Conn, root: Id, chosen: Option<u32>) -> ApiResult<u32> {
    let minimum = setting::<i64>(conn, Place::Node(root), "ask.retention_min_days")
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(1);
    let maximum = setting::<i64>(conn, Place::Node(root), "ask.retention_max_days")
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(3650);
    if minimum < 1 || minimum > maximum {
        return Err(denied(
            "The organization must set a valid Ask retention range",
        ));
    }
    let value = match chosen {
        Some(value) => i64::from(value),
        None => setting::<i64>(conn, Place::Node(root), "ask.retention_days")
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(30),
    };
    u32::try_from(value.clamp(minimum, maximum)).map_err(ApiError::internal)
}

async fn root_of_context(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    context: Option<&str>,
    fallback: Option<Id>,
) -> ApiResult<Id> {
    if let Some(context) = context {
        if context.starts_with('/') || context.starts_with("http:") || context.starts_with("https:")
        {
            let path = tools::own_path(ctx, context)?;
            if !matches!(
                path.trim_start_matches('/').split('/').next(),
                Some(
                    "org"
                        | "project"
                        | "topic"
                        | "thread"
                        | "doc"
                        | "document"
                        | "nodes"
                        | "documents"
                )
            ) {
                return fallback
                    .or(ctx.user()?.user.personal_org_id)
                    .ok_or_else(|| denied("Choose an organization for this page"));
            }
        }
        let (kind, id, _) = tools::reference_id(ctx, conn, context).await?;
        let object_kind = match kind.as_str() {
            "doc" | "document" => ObjectKind::Document,
            "thread" => ObjectKind::Thread,
            "topic" => ObjectKind::Topic,
            "project" => ObjectKind::Project,
            "org" => ObjectKind::Org,
            _ => return fallback.ok_or_else(|| denied("Choose an organization for this page")),
        };
        let target = crate::key_operations::targets::page_object(
            conn,
            orch_access_contract::decisions::ObjectRef {
                kind: object_kind,
                id,
            },
        )
        .await?;
        let path = target.node_path().ok_or_else(ApiError::not_found)?;
        let place = *path.last().ok_or_else(ApiError::not_found)?;
        let policy = crate::routes::nodes::policy_at(conn, place).await?;
        let decision = if object_kind == ObjectKind::Document {
            orch_authz::documents::check(conn, principal, id, Permission::DocView, policy).await
        } else {
            orch_authz::nodes::check_named(
                conn,
                principal,
                id,
                "node.view".parse().map_err(ApiError::internal)?,
                Act::Use,
                policy,
            )
            .await
        }
        .map_err(ApiError::internal)?;
        if !matches!(decision, Decision::Allowed { .. }) {
            return Err(ApiError::not_found());
        }
        return target
            .node_path()
            .and_then(|path| path.first().copied())
            .ok_or_else(ApiError::not_found);
    }
    fallback
        .or(ctx.user()?.user.personal_org_id)
        .ok_or_else(|| denied("Choose an organization before starting Ask"))
}

pub(crate) async fn opening(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    lock: bool,
) -> ApiResult<(rows::Conversation, bool, u32)> {
    opening_for(ctx, conn, principal, id, lock, &[Permission::ThreadView]).await
}

pub(crate) async fn opening_for(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    lock: bool,
    permissions: &[Permission],
) -> ApiResult<(rows::Conversation, bool, u32)> {
    let row = rows::get(conn, id, lock)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !row.temporary {
        return Err(ApiError::not_found());
    }
    let inspecting = row.user_id != principal.caller().user();
    if inspecting && !administers(conn, principal, row.root_org_id).await? {
        return Err(ApiError::not_found());
    }
    let ordinary = if inspecting {
        None
    } else {
        Some(principal.ordinary(conn).await.map_err(ApiError::internal)?)
    };
    let reader = ordinary.as_ref().unwrap_or(principal);
    root_access(conn, reader, row.root_org_id).await?;
    let policy = crate::routes::nodes::policy_at(conn, id).await?;
    for permission in permissions {
        authorization::require(
            conn,
            reader,
            id,
            authorization::exact(*permission),
            Act::Use,
            policy,
        )
        .await?;
    }
    if inspecting {
        crate::middleware::admin_openings::note(
            ctx,
            conn,
            "thread",
            id,
            json!({"kind":"org","id":row.root_org_id}),
            ctx.opening,
        )
        .await?;
    }
    let days = retention(
        conn,
        row.root_org_id,
        Some(u32::try_from(row.retention_days).map_err(ApiError::internal)?),
    )
    .await?;
    let expires = row
        .last_message_at
        .checked_add_signed(chrono::Duration::days(i64::from(days)))
        .ok_or_else(|| denied("Ask retention is outside the supported date range"))?;
    if row.temporary && expires <= ctx.app.clock.now() {
        return Err(ApiError::not_found());
    }
    Ok((row, inspecting, days))
}

pub(crate) fn header(
    row: &rows::Conversation,
    inspecting: bool,
    days: u32,
) -> ApiResult<api::AskConversation> {
    let state = match row.state.as_str() {
        "running" => api::AskState::Running,
        "closed" => api::AskState::Closed,
        "kept" => api::AskState::Kept,
        _ => api::AskState::Ready,
    };
    let expires = row
        .last_message_at
        .checked_add_signed(chrono::Duration::days(i64::from(days)))
        .ok_or_else(|| denied("Ask retention is outside the supported date range"))?;
    let active = !inspecting && matches!(state, api::AskState::Ready | api::AskState::Running);
    Ok(api::AskConversation {
        id: row.id,
        user_id: row.user_id,
        root_org_id: row.root_org_id,
        home_org_id: row.home_org_id,
        title: row.title.clone(),
        state,
        model: row
            .model
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?,
        payer_id: row.payer_id,
        created_at: Time::new(row.created_at),
        last_message_at: Time::new(row.last_message_at),
        expires_at: Time::new(expires),
        retention_days: days,
        shown: row.shown,
        admin_inspectable: true,
        inspecting,
        can: api::AskCan {
            message: active && state == api::AskState::Ready,
            stop: active && state == api::AskState::Running,
            keep: active && state == api::AskState::Ready,
            delete: !inspecting,
            export: true,
        },
        seq: row.seq,
    })
}

pub(crate) fn item_message(item: &threads::ItemRow) -> ApiResult<Option<api::AskMessage>> {
    let Some(turn_id) = item.turn_id else {
        return Ok(None);
    };
    let (role, content) = match item.kind.as_str() {
        "user_message" => (
            api::AskRole::User,
            serde_json::from_value(
                item.payload
                    .get("parts")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .map_err(ApiError::internal)?,
        ),
        "agent_message" => (
            api::AskRole::Assistant,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: item
                    .payload
                    .get("text")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .into(),
            }],
        ),
        _ => return Ok(None),
    };
    let citations = serde_json::from_value(
        item.payload
            .get("ask_citations")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(ApiError::internal)?;
    Ok(Some(api::AskMessage {
        id: item.id,
        turn_id,
        role,
        content,
        citations,
        summary: if matches!(
            item.status.as_str(),
            "completed" | "in_progress" | "pending"
        ) {
            item.payload
                .get("ask_summary")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(ApiError::internal)?
        } else {
            None
        },
        complete: !matches!(item.status.as_str(), "in_progress" | "pending"),
        created_at: Time::new(item.created_at),
    }))
}

pub(crate) async fn emit_header(_ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<Seq> {
    let row = rows::get(conn, id, false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !row.temporary {
        return Ok(row.seq);
    }
    let days = retention(
        conn,
        row.root_org_id,
        Some(u32::try_from(row.retention_days).map_err(ApiError::internal)?),
    )
    .await?;
    let mut conversation = header(&row, false, days)?;
    conversation.seq = conversation.seq.saturating_add(1);
    rows::emit(
        conn,
        id,
        &serde_json::to_value(api::AskEventData::Conversation {
            conversation: Box::new(conversation),
        })
        .map_err(ApiError::internal)?,
    )
    .await
    .map_err(Into::into)
}

pub(crate) async fn emit_settled(
    ctx: &Ctx,
    conn: &mut Conn,
    thread: Id,
    turn: Id,
) -> ApiResult<()> {
    for item in threads::forking::items_of_turns(conn, thread, &[turn]).await? {
        if let Some(message) = item_message(&item)? {
            rows::emit(
                conn,
                thread,
                &serde_json::to_value(api::AskEventData::Message {
                    message: Box::new(message),
                })
                .map_err(ApiError::internal)?,
            )
            .await?;
        }
    }
    emit_header(ctx, conn, thread).await?;
    Ok(())
}

async fn create(ctx: Ctx, (): (), body: api::AskCreateBody) -> ApiResult<api::AskReceipt> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard
        .principal()
        .ordinary(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let root = root_of_context(
        &ctx,
        &mut tx,
        &principal,
        body.context.as_deref(),
        body.root_org_id,
    )
    .await?;
    root_access(&mut tx, &principal, root).await?;
    models::check_preference(&mut tx, &principal, root, body.model.as_ref()).await?;
    let enabled = setting::<bool>(&mut tx, Place::System, "orgs.personal_enabled")
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(true);
    let home = if enabled {
        ctx.user()?.user.personal_org_id.unwrap_or(root)
    } else {
        root
    };
    let owner = hierarchy::node(&mut tx, home)
        .await?
        .ok_or_else(ApiError::not_found)?
        .owner_id;
    let project = orch_engine::ids::derived("ask.home.project", &[home.as_bytes()]);
    let topic = orch_engine::ids::derived("ask.home.topic", &[home.as_bytes()]);
    let days = retention(&mut tx, root, body.retention_days).await?;
    guard.check_deadline(&ctx)?;
    rows::system_container(&mut tx, project, home, "project", owner, ctx.now).await?;
    rows::system_container(&mut tx, topic, project, "topic", owner, ctx.now).await?;
    let id = new_id();
    let person = ctx.user()?.id();
    if !threads::insert_thread(&mut tx, id, topic, "Ask", person, ctx.now).await? {
        return Err(ApiError::not_found());
    }
    start_private(&mut tx, id, person, &principal, ctx.now).await?;
    let model = body
        .model
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    rows::bind(
        &mut tx,
        id,
        root,
        body.context.as_deref(),
        model.as_ref(),
        i32::try_from(days).map_err(ApiError::internal)?,
        ctx.now,
    )
    .await?;
    let seq = rows::emit(
        &mut tx,
        id,
        &json!({"kind":"notice","code":"created","message":"Ask is ready"}),
    )
    .await?;
    orch_engine::binding::finish_thread_birth(&mut tx, id, topic, person, ctx.now)
        .await
        .map_err(ApiError::engine)?;
    creation::observe_conversation(&mut tx, guard.user().id(), id, seq).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let receipt = api::AskReceipt {
        conversation_id: id,
        turn_id: None,
        seq,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    Ok(receipt)
}

pub(crate) async fn start_private(
    conn: &mut Conn,
    id: Id,
    person: Id,
    principal: &Principal,
    at: chrono::DateTime<chrono::Utc>,
) -> ApiResult<()> {
    orch_authz::pages::start(
        conn,
        orch_access_contract::pages::StoredKind::Node,
        id,
        person,
        Time::new(at),
    )
    .await
    .map_err(ApiError::internal)?;
    let policy = crate::routes::nodes::policy_at(conn, id).await?;
    let change = orch_access_contract::pages::PageChange {
        whole: Some(orch_access_contract::pages::Whole::Private),
        ..Default::default()
    };
    let outcome = orch_authz::pages::change(
        conn,
        principal,
        orch_authz::pages::Target::Live(id),
        &change,
        policy,
        &orch_db::audit::current_origin(),
    )
    .await
    .map_err(ApiError::internal)?;
    if !outcome.skipped.is_empty() {
        return Err(denied(
            "A locked access rule prevents a Private conversation here",
        ));
    }
    Ok(())
}

fn prepare_list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    mut query: api::AskListQuery,
    (): (),
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        if let Some(origin) = &ctx.ask_read {
            query.root_org_id = Some(origin.root());
            query.inspect = false;
        }
        let principal =
            authorization::load_principal(conn, caller.clone(), ctx.app.clock.time()).await?;
        let person = if query.inspect {
            let root = query
                .root_org_id
                .ok_or_else(|| denied("Choose one organization to inspect"))?;
            if !administers(conn, &principal, root).await? {
                return Err(denied(
                    "Use As admin for this organization's Ask conversations",
                ));
            }
            None
        } else {
            Some(caller.user())
        };
        if let Some(root) = query.root_org_id {
            root_access(conn, &principal, root).await?;
        }
        let before = query
            .cursor
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|_| denied("Invalid history cursor"))?;
        let selected = rows::list(
            conn,
            person,
            query.root_org_id,
            before,
            i64::from(query.limit.unwrap_or(50).clamp(1, 200)) + 1,
        )
        .await?;
        let mut roots = Vec::new();
        for row in &selected {
            if !roots.iter().any(
                |target: &orch_access_contract::key_scope::KeyResolvedTarget| {
                    target.object.id == Some(row.root_org_id)
                },
            ) {
                roots.push(crate::key_operations::targets::node(conn, row.root_org_id).await?);
            }
        }
        let mut plan = crate::registry::OperationPlan::new(ctx.route, ctx.route.name)?;
        let account = crate::key_operations::targets::account(conn, caller.user()).await?;
        plan.bind(
            0,
            orch_access_contract::key_scope::KeyTargetKind::Identity(
                orch_access_contract::key_scope::KeyIdentityKind::Account,
            ),
            &[account],
        )?;
        if roots.is_empty() {
            plan.bind_empty(1)?;
        } else {
            plan.bind_collection(
                1,
                &[(
                    orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Org),
                    roots,
                )],
            )?;
        }
        Ok(plan.with_data(selected))
    })
}

async fn list(ctx: Ctx, mut query: api::AskListQuery, (): ()) -> ApiResult<api::AskListReply> {
    if let Some(origin) = &ctx.ask_read {
        query.root_org_id = Some(origin.root());
    }
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let rows = ctx.operation_data::<Vec<rows::Conversation>>()?;
    let next_cursor = if rows.len() > limit as usize {
        rows.get(limit as usize - 1).map(|row| row.id.to_string())
    } else {
        None
    };
    let mut items = Vec::new();
    for row in rows.iter().take(limit as usize) {
        if root_access(&mut conn, &principal, row.root_org_id)
            .await
            .is_ok()
        {
            let policy = crate::routes::nodes::policy_at(&mut conn, row.id).await?;
            match authorization::require(
                &mut conn,
                &principal,
                row.id,
                authorization::exact(Permission::ThreadView),
                Act::Use,
                policy,
            )
            .await
            {
                Ok(()) => {}
                Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => continue,
                Err(error) => return Err(error),
            }
            let days = retention(
                &mut conn,
                row.root_org_id,
                Some(u32::try_from(row.retention_days).map_err(ApiError::internal)?),
            )
            .await?;
            let inspecting = query.inspect && row.user_id != ctx.user()?.id();
            let value = header(row, inspecting, days)?;
            if value.expires_at.get() > ctx.app.clock.now() {
                items.push(value);
            }
        }
    }
    Ok(api::AskListReply { items, next_cursor })
}

async fn get(ctx: Ctx, query: api::AskGetQuery, (): ()) -> ApiResult<api::AskGetReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let (row, inspecting, days) =
        opening(&ctx, &mut conn, &principal, ctx.id("id")?, false).await?;
    let before = query
        .before
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(|_| denied("Invalid message cursor"))?;
    let view = threads::thread(&mut conn, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?
        .active_continuation_id;
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as usize;
    let mut rows =
        rows::messages_in_view(&mut conn, row.id, view, before, limit as i64 + 1).await?;
    if rows.len() > limit {
        rows.remove(0);
    }
    let previous_cursor = if let Some(first) = rows.first() {
        if rows::messages_in_view(&mut conn, row.id, view, Some(first.seq_created), 1)
            .await?
            .is_empty()
        {
            None
        } else {
            Some(first.seq_created.to_string())
        }
    } else {
        None
    };
    let messages = rows
        .iter()
        .filter_map(|item| item_message(item).transpose())
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(api::AskGetReply {
        conversation: header(&row, inspecting, days)?,
        messages,
        previous_cursor,
    })
}

async fn message(ctx: Ctx, (): (), body: api::AskMessageBody) -> ApiResult<api::AskReceipt> {
    runner::message(ctx, body).await
}

async fn stop(ctx: Ctx, (): (), (): ()) -> ApiResult<api::AskReceipt> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let (row, inspecting, _) = opening_for(
        &ctx,
        &mut tx,
        guard.principal(),
        ctx.id("id")?,
        true,
        &[Permission::ThreadInteract],
    )
    .await?;
    if inspecting {
        return Err(denied(
            "Administrators may inspect and export, but cannot direct Ask",
        ));
    }
    guard.check_deadline(&ctx)?;
    let calls = if let Some(turn) = row.turn {
        let calls = rows::calls(&mut tx, turn).await?;
        let seq = orch_db::sequences::next(&mut tx, row.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        threads::finish_turn(&mut tx, turn, "interrupted", None, ctx.now, seq).await?;
        threads::settle_items_of_turn(&mut tx, turn, "cancelled", ctx.now, seq).await?;
        rows::finish(&mut tx, row.id, turn).await?;
        emit_settled(&ctx, &mut tx, row.id, turn).await?;
        calls
    } else {
        vec![]
    };
    let seq = rows::emit(
        &mut tx,
        row.id,
        &json!({"kind":"notice","code":"stopped","message":"Ask stopped"}),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app
        .engine
        .cancel_ask_calls(&calls)
        .await
        .map_err(ApiError::internal)?;
    Ok(api::AskReceipt {
        conversation_id: row.id,
        turn_id: row.turn,
        seq,
    })
}

async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let (row, inspecting, _) = opening_for(
        &ctx,
        &mut tx,
        guard.principal(),
        ctx.id("id")?,
        true,
        &[Permission::ThreadManage],
    )
    .await?;
    if inspecting {
        return Err(denied("Only the person may delete their Ask conversation"));
    }
    guard.check_deadline(&ctx)?;
    let calls = match row.turn {
        Some(turn) => rows::calls(&mut tx, turn).await?,
        None => vec![],
    };
    rows::delete(&mut tx, row.id).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app
        .engine
        .cancel_ask_calls(&calls)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

async fn update(ctx: Ctx, (): (), body: api::AskUpdateBody) -> ApiResult<api::AskReceipt> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let (row, inspecting, _) = opening_for(
        &ctx,
        &mut tx,
        guard.principal(),
        ctx.id("id")?,
        true,
        &[Permission::ThreadInteract],
    )
    .await?;
    if inspecting {
        return Err(denied("Only the person may change their Ask preferences"));
    }
    let days = match body.retention_days {
        Some(days) => Some(
            i32::try_from(retention(&mut tx, row.root_org_id, Some(days)).await?)
                .map_err(ApiError::internal)?,
        ),
        None => None,
    };
    guard.check_deadline(&ctx)?;
    rows::preferences(&mut tx, row.id, body.shown, days).await?;
    let updated = rows::get(&mut tx, row.id, false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let days = retention(
        &mut tx,
        row.root_org_id,
        Some(u32::try_from(updated.retention_days).map_err(ApiError::internal)?),
    )
    .await?;
    let seq = rows::emit(
        &mut tx,
        row.id,
        &serde_json::to_value(api::AskEventData::Conversation {
            conversation: Box::new(header(&updated, false, days)?),
        })
        .map_err(ApiError::internal)?,
    )
    .await?;
    hierarchy::stamp(&mut tx, row.id).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(api::AskReceipt {
        conversation_id: row.id,
        turn_id: row.turn,
        seq,
    })
}

async fn events(
    ctx: Ctx,
    query: api::AskEventsQuery,
    _body: axum::body::Body,
) -> ApiResult<axum::response::Response> {
    events::get(ctx, query).await
}

#[path = "ask/addresses.rs"]
pub mod addresses;

#[path = "ask/birth.rs"]
pub mod birth;

#[path = "ask/creation.rs"]
pub mod creation;

#[path = "ask/events.rs"]
pub mod events;

#[path = "ask/export.rs"]
pub mod export;

#[path = "ask/files.rs"]
pub mod files;

#[path = "ask/keep.rs"]
pub mod keep;

#[path = "ask/models.rs"]
pub mod models;

#[path = "ask/program.rs"]
pub mod program;

#[path = "ask/recovery.rs"]
pub mod recovery;

#[path = "ask/runner.rs"]
pub mod runner;

#[path = "ask/search.rs"]
pub mod search;

#[path = "ask/tools.rs"]
pub mod tools;
