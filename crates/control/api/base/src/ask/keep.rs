//! Keep uses the existing conversation-cut copier and a new Private creation.

use super::*;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyTargetKind;
#[derive(Debug, Clone, Copy)]
struct Destination {
    node: Id,
    new_topic: bool,
}

async fn source(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    lock: bool,
) -> ApiResult<rows::Conversation> {
    let row = rows::get(conn, id, lock)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.temporary {
        let (row, inspecting, _) = opening(ctx, conn, principal, id, false).await?;
        if inspecting || row.user_id != principal.caller().user() {
            return Err(denied("Only the person may keep their temporary Ask"));
        }
        return Ok(row);
    }
    root_access(conn, principal, row.root_org_id).await?;
    let policy = crate::routes::nodes::policy_at(conn, id).await?;
    for permission in [Permission::ThreadView, Permission::ThreadFork] {
        authorization::require(
            conn,
            principal,
            id,
            authorization::exact(permission),
            Act::Use,
            policy,
        )
        .await?;
    }
    Ok(row)
}

async fn may(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    permission: Permission,
) -> ApiResult<bool> {
    let policy = crate::routes::nodes::policy_at(conn, node).await?;
    Ok(matches!(
        orch_authz::nodes::check(conn, principal, node, permission, Act::Use, policy)
            .await
            .map_err(ApiError::internal)?,
        Decision::Allowed { .. }
    ))
}

async fn destination(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::Conversation,
    body: &api::AskKeepBody,
) -> ApiResult<Destination> {
    if body.topic_id.is_some() && body.project_id.is_some() {
        return Err(ApiError::invalid(
            "topic_id",
            "Choose a topic or a project, not both",
        ));
    }
    if let Some(id) = body.topic_id.or(body.project_id) {
        let node = hierarchy::node(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let new_topic = body.project_id.is_some();
        let permission = if new_topic {
            Permission::ProjectCreateTopics
        } else {
            Permission::TopicStartThreads
        };
        if node.kind != if new_topic { "project" } else { "topic" }
            || !may(conn, principal, id, permission).await?
        {
            return Err(denied("Ask for access to create a thread here"));
        }
        return Ok(Destination {
            node: id,
            new_topic,
        });
    }
    let context = if row.temporary {
        row.context.clone()
    } else {
        Some(format!("/thread/{}", row.id))
    };
    if let Some(context) = context {
        let (kind, id, _) = tools::reference_id(ctx, conn, &context).await?;
        let object = match kind.as_str() {
            "doc" | "document" => ObjectKind::Document,
            "thread" => ObjectKind::Thread,
            "topic" => ObjectKind::Topic,
            "project" => ObjectKind::Project,
            "org" => ObjectKind::Org,
            _ => {
                return Err(denied(
                    "Choose a place in this organization or Ask for access",
                ));
            }
        };
        let target = crate::key_operations::targets::page_object(
            conn,
            orch_access_contract::decisions::ObjectRef { kind: object, id },
        )
        .await?;
        for id in target.node_path().unwrap_or_default().into_iter().rev() {
            let Some(node) = hierarchy::node(conn, id).await? else {
                continue;
            };
            if node.org_id != row.root_org_id {
                continue;
            }
            if node.kind == "topic"
                && may(conn, principal, id, Permission::TopicStartThreads).await?
            {
                return Ok(Destination {
                    node: id,
                    new_topic: false,
                });
            }
            if node.kind == "project"
                && may(conn, principal, id, Permission::ProjectCreateTopics).await?
            {
                return Ok(Destination {
                    node: id,
                    new_topic: true,
                });
            }
        }
    }
    Err(denied(
        "Choose a place in this organization or Ask for access",
    ))
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    body: api::AskKeepBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(
            0,
            KeyTargetKind::Object(ObjectKind::Thread),
            &[crate::key_operations::targets::node(conn, ctx.id("id")?).await?],
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let row = source(ctx, conn, &principal, ctx.id("id")?, false).await?;
        let place = destination(ctx, conn, &principal, &row, &body).await?;
        crate::routes::duplicates::copy_to(
            conn,
            &principal,
            orch_engine::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
                kind: ObjectKind::Thread,
                id: row.id,
            }),
            Some(place.node),
        )
        .await?;
        let target = crate::key_operations::targets::node(conn, place.node).await?;
        plan.bind(1, KeyTargetKind::Object(ObjectKind::Thread), &[target])?;
        Ok(plan.with_data(place))
    })
}

async fn history_range(
    conn: &mut Conn,
    row: &rows::Conversation,
    through: Option<i64>,
) -> ApiResult<Option<(i32, i32)>> {
    let Some(through) = through else {
        return Ok(None);
    };
    let through = i32::try_from(through)
        .ok()
        .filter(|turn| *turn >= 1)
        .ok_or_else(|| ApiError::invalid("to_turn", "Choose a visible finished turn"))?;
    let thread = threads::thread(conn, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    // Read the actual active continuation through the same owner spans as the
    // history copier. An inactive suffix with the same ordinal is not a source.
    let selected = orch_db::queries::thread_continuations::reading::turns(
        conn,
        row.id,
        thread.active_continuation_id,
        through.checked_add(1),
        1,
    )
    .await?;
    if !selected.first().is_some_and(|turn| {
        turn.ordinal == through
            && matches!(turn.state.as_str(), "completed" | "interrupted" | "failed")
    }) {
        return Err(ApiError::invalid(
            "to_turn",
            "Choose a visible finished turn",
        ));
    }
    Ok(Some((1, through)))
}

pub(super) async fn keep(ctx: Ctx, (): (), body: api::AskKeepBody) -> ApiResult<api::AskKeepReply> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = source(&ctx, &mut tx, guard.principal(), ctx.id("id")?, true).await?;
    let person = guard.user().id();
    if row.state != "ready" {
        return Err(denied("Stop Ask before keeping it as a thread"));
    }
    let range = history_range(&mut tx, &row, body.to_turn).await?;
    let destination = *guard.operation_data::<Destination>()?;
    let copy_boundary = crate::routes::duplicates::copy_boundary(
        &mut tx,
        guard.principal(),
        orch_engine::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
            kind: ObjectKind::Thread,
            id: row.id,
        }),
        Some(destination.node),
    )
    .await?;
    guard.check_deadline(&ctx)?;
    let topic = if destination.new_topic {
        let id = new_id();
        hierarchy::create(
            &mut tx,
            &hierarchy::NewNode {
                id,
                kind: "topic",
                parent: Some(destination.node),
                name: "Ask",
                name_by_person: true,
                description: "",
                owner: person,
                org: None,
                at: ctx.now,
            },
        )
        .await?;
        orch_authz::pages::start(
            &mut tx,
            orch_access_contract::pages::StoredKind::Node,
            id,
            person,
            Time::new(ctx.now),
        )
        .await
        .map_err(ApiError::internal)?;
        orch_engine::binding::finish_node_birth(&mut tx, id, destination.node, person, ctx.now)
            .await
            .map_err(ApiError::engine)?;
        id
    } else {
        destination.node
    };
    let thread = new_id();
    let title = body.title.as_deref().unwrap_or(&row.title).trim();
    if title.is_empty() || title.chars().count() > orch_engine::binding::NAME_MAX_CHARS {
        return Err(ApiError::invalid(
            "title",
            "Choose a bounded nonempty thread title",
        ));
    }
    if !threads::insert_thread(&mut tx, thread, topic, title, person, ctx.now).await? {
        return Err(ApiError::not_found());
    }
    start_private(&mut tx, thread, person, guard.principal(), ctx.now).await?;
    rows::bind_kept(
        &mut tx,
        thread,
        row.root_org_id,
        row.model.as_ref(),
        row.payer_id,
        ctx.now,
    )
    .await?;
    if let Some(source_segment) = row.segment {
        let binding = rows::segment_binding(&mut tx, row.id, source_segment)
            .await?
            .ok_or_else(|| {
                ApiError::internal("The retained conversation segment has no provider")
            })?;
        let segment = new_id();
        let mut unit = orch_engine::unit::Unit::new(&mut tx, ctx.now);
        let seq = unit.stamp(thread).await.map_err(ApiError::internal)?;
        rows::segment(unit.conn, segment, thread, &binding, person, ctx.now, seq).await?;
        orch_engine::fork_history::copy_conversation(
            &mut unit,
            row.id,
            thread,
            segment,
            range,
            row.segment,
        )
        .await
        .map_err(ApiError::internal)?;
        unit.finish().await.map_err(ApiError::internal)?;
    }
    rows::copy_files(&mut tx, row.id, thread, ctx.now).await?;
    if row.temporary {
        rows::kept(&mut tx, row.id, thread).await?;
        let days = retention(
            &mut tx,
            row.root_org_id,
            Some(u32::try_from(row.retention_days).map_err(ApiError::internal)?),
        )
        .await?;
        let updated = rows::get(&mut tx, row.id, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        rows::emit(
            &mut tx,
            row.id,
            &serde_json::to_value(api::AskEventData::Conversation {
                conversation: Box::new(header(&updated, false, days)?),
            })
            .map_err(ApiError::internal)?,
        )
        .await?;
    }
    orch_engine::binding::finish_thread_birth(&mut tx, thread, topic, person, ctx.now)
        .await
        .map_err(ApiError::engine)?;
    super::creation::observe_keep(&mut tx, guard.user().id(), row.id, thread, topic).await?;
    if let Some(boundary) = copy_boundary {
        orch_engine::copy_out::record(
            &mut tx,
            boundary,
            orch_db::audit::Actor::User(person),
            &crate::sign_in::origin(&ctx),
            thread,
            ctx.now,
        )
        .await
        .map_err(ApiError::engine)?;
    }
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let receipt = api::AskKeepReply {
        conversation_id: row.id,
        thread_id: thread,
        topic_id: topic,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    Ok(receipt)
}
