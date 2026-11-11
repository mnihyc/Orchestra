//! Native home moves and display-tag assignments over the existing resource rows.

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::routes::access::subject;

use crate::routes::resources;

use crate::sign_in::origin;

use futures::future::BoxFuture;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resource_groups as contract;

use orch_api_contract::resource_groups::Source;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_authz::things::Thing;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::audit;

use orch_db::audit::Actor;

use orch_db::audit::Entry;

use orch_db::queries::hierarchy;

use orch_db::queries::resources::grouping as stored;

use orch_types::Id;

use orch_types::Json;

use serde_json::json;
struct Root {
    object: ObjectRef,
    kind: ResourceKind,
    row: stored::Row,
}

fn safe(caller: &Caller, error: ApiError) -> ApiError {
    if native::modern(caller) {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

async fn selected(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Root> {
    let Thing::Other(object) = subject::named(ctx)?.thing else {
        return Err(ApiError::not_found());
    };
    let kind = object
        .kind
        .as_str()
        .parse::<ResourceKind>()
        .map_err(|_| ApiError::not_found())?;
    let row = stored::among(conn, &[(kind.as_str().to_owned(), object.id)])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if row.container_environment_id.is_some() {
        return Err(ApiError::not_found());
    }
    Ok(Root { object, kind, row })
}

async fn fact(conn: &mut Conn, root: &Root) -> ApiResult<native::Facts<Source>> {
    let target = if root.kind == ResourceKind::Machine {
        targets::machine_subject(conn, root.object.id).await?
    } else {
        targets::page_object(conn, root.object).await?
    };
    Ok(native::Facts::one(
        Source::Object,
        KeyTargetKind::Object(root.object.kind),
        target,
    ))
}

async fn require_source(conn: &mut Conn, principal: &Principal, root: &Root) -> ApiResult<()> {
    let permission = match root.kind {
        ResourceKind::Environment => Permission::EnvironmentManage,
        ResourceKind::Provider => Permission::ProviderManage,
        ResourceKind::Agent => Permission::AgentManage,
        ResourceKind::Host => Permission::HostManage,
        ResourceKind::Machine => Permission::MachineManage,
        ResourceKind::Template => Permission::TemplateManage,
        ResourceKind::McpServer => Permission::McpManage,
        ResourceKind::Mount => Permission::MountManage,
        ResourceKind::Skill => Permission::SkillManage,
    };
    let result = if root.kind == ResourceKind::Machine {
        resources::machines::require_machine(conn, principal, root.object.id, permission).await
    } else {
        resources::require(conn, principal, root.kind, root.object.id, permission).await
    };
    result.map_err(|error| safe(principal.caller(), error))
}

async fn locked(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
) -> ApiResult<Root> {
    let initial = selected(ctx, conn).await?;
    let known = native::bind_known(ctx, declaration, &[fact(conn, &initial).await?])?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require_source(conn, &principal, &initial).await?;
    if orch_db::queries::transaction_guard::in_transaction(conn) {
        if initial.kind == ResourceKind::Environment {
            crate::routes::environments::updates::ensure_unheld(conn, initial.object.id)
                .await
                .map_err(|error| safe(caller, error))?;
        }
        if !stored::lock(conn, initial.kind, initial.object.id).await? {
            return Err(ApiError::not_found());
        }
    }
    selected(ctx, conn).await
}

fn revision(root: &Root, expected: Option<i64>) -> ApiResult<()> {
    if expected.is_some_and(|expected| expected != root.row.revision) {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The resource changed. Read it and try again.",
        ));
    }
    Ok(())
}

async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    root: &Root,
    action: AuditAction,
    home: Option<Id>,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        action.as_str(),
        root.kind.as_str(),
        Some(root.object.id),
        summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = home;
    entry.org_id = match home {
        Some(home) => hierarchy::with_ancestors(conn, &[home])
            .await?
            .into_iter()
            .find(|row| row.id == home)
            .map(|row| row.org_id),
        None => None,
    };
    entry.details = details;
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}

pub(super) struct HomeMove;

pub(super) struct HomeValue {
    root: Root,
    destination: hierarchy::NodeRow,
    expected_revision: Option<i64>,
}

impl effects::Effect for HomeMove {
    type Source = Source;
    type Query = ();
    type Body = contract::ResourceHomeSetBody;
    type Value = HomeValue;
    type Output = contract::ResourceHomeSetReceipt;
    fn declaration(_: &HomeValue) -> &'static Declaration<Source> {
        &contract::HOME
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(HomeValue, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = locked(ctx, conn, caller, &contract::HOME).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                    .bind(body.home_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            let destination = hierarchy::with_ancestors(conn, &[body.home_id])
                .await?
                .into_iter()
                .find(|node| node.id == body.home_id && node.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            if !matches!(destination.kind.as_str(), "org" | "project") {
                return Err(ApiError::invalid("home_id", "Choose an org or project."));
            }
            let target = targets::node(conn, destination.id).await?;
            let facts = vec![
                fact(conn, &root).await?,
                native::Facts::one(Source::Home, target.object.kind, target),
            ];
            Ok((
                HomeValue {
                    root,
                    destination,
                    expected_revision: body.expected_revision,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a HomeValue,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_source(conn, principal, &value.root).await?;
            crate::routes::nodes::require_manage(conn, principal, value.destination.id, Act::Use)
                .await
                .map_err(|error| safe(principal.caller(), error))?;
            let (_, source_org) = orch_engine::copy_out::source_place(
                conn,
                orch_engine::copy_out::Subject::Object(value.root.object),
            )
            .await
            .map_err(crate::routes::duplicates::copy_error)?;
            if source_org != Some(value.destination.org_id) {
                return Err(ApiError::new(
                    ErrorCode::NodeWrongParent,
                    "Use Move to another org to copy there and delete here.",
                )
                .with("next_step", "move_to_another_org")
                .with("field", "home_id"));
            }
            revision(&value.root, value.expected_revision)
        })
    }
}

impl effects::Apply for HomeMove {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a HomeValue,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let root = &value.root;
            let changed = root.row.home_id != Some(value.destination.id)
                || root.row.home_overridden_at.is_none();
            let receipt = contract::ResourceHomeSetReceipt {
                projection: ReceiptProjection::Receipt,
                object_id: root.object.id,
                kind: root.kind.as_str().to_owned(),
                home_id: value.destination.id,
                changed,
            };
            let deadline = before.deadline();
            before.check()?;
            if !changed {
                return Ok(effects::Outcome::Unchanged(receipt));
            }
            stored::set_home(
                conn,
                root.kind,
                root.object.id,
                value.destination.id,
                value.destination.org_id,
                ctx.now,
            )
            .await?;
            if root.kind == ResourceKind::Environment {
                orch_db::queries::resources::mounts::changed(conn, &[root.object.id], ctx.now)
                    .await?;
                orch_db::queries::jobs::stamp_environment(conn, root.object.id, ctx.now).await?;
            }
            record(ctx, conn, root, orch_db::audit::action!(AuditAction::ResourceHomeChanged), Some(value.destination.id),
                &format!("Moved {} to {}", root.row.name, value.destination.name),
                json!({"from_home":root.row.home_id,"home_id":value.destination.id,"origin_machine":root.row.origin_machine_id})).await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(receipt))
        })
    }
}

pub(super) async fn home(
    ctx: Ctx,
    (): (),
    _: contract::ResourceHomeSetBody,
) -> ApiResult<contract::ResourceHomeSetReceipt> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<HomeMove>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    publication::receipt(&done.ctx, &done.output)?;
    if done.output.changed && done.output.kind == "environment" {
        if let Err(error) = done
            .ctx
            .app
            .engine
            .apply_environment_mounts(done.output.object_id)
            .await
        {
            tracing::warn!(environment=%done.output.object_id,%error,"environment mount changes remain pending");
        }
        if let Err(error) = done
            .ctx
            .app
            .engine
            .apply_environment_resources(done.output.object_id)
            .await
        {
            tracing::warn!(environment=%done.output.object_id,%error,"environment resource changes remain pending");
        }
    }
    Ok(done.output)
}

pub(super) struct Tags;

pub(super) struct TagValue {
    root: Root,
    desired: Vec<Id>,
    changed: bool,
}

impl effects::Effect for Tags {
    type Source = Source;
    type Query = ();
    type Body = contract::ResourceTagsSetBody;
    type Value = TagValue;
    type Output = contract::ResourceTagsSetReceipt;
    fn declaration(_: &TagValue) -> &'static Declaration<Source> {
        &contract::TAGS
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        mut body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(TagValue, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = locked(ctx, conn, caller, &contract::TAGS).await?;
            revision(&root, body.expected_revision)?;
            body.tag_ids.sort_unstable();
            body.tag_ids.dedup();
            let carried = stored::tag_ids(conn, root.kind, root.object.id).await?;
            crate::routes::nodes::tags::check(conn, root.row.home_id, &body.tag_ids, &carried)
                .await?;
            let changed = carried != body.tag_ids;
            let facts = vec![fact(conn, &root).await?];
            Ok((
                TagValue {
                    root,
                    desired: body.tag_ids,
                    changed,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a TagValue,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_source(conn, principal, &value.root))
    }
}

impl effects::Apply for Tags {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a TagValue,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let root = &value.root;
            let receipt = contract::ResourceTagsSetReceipt {
                projection: ReceiptProjection::Receipt,
                object_id: root.object.id,
                kind: root.kind.as_str().to_owned(),
                tag_ids: value.desired.clone(),
                changed: value.changed,
            };
            let deadline = before.deadline();
            before.check()?;
            if !value.changed {
                return Ok(effects::Outcome::Unchanged(receipt));
            }
            stored::set_tags(conn, root.kind, root.object.id, &value.desired, ctx.now).await?;
            record(
                ctx,
                conn,
                root,
                orch_db::audit::action!(AuditAction::ResourceTagsChanged),
                root.row.home_id,
                &format!("Changed the tags of {}", root.row.name),
                json!({"tag_ids":value.desired}),
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(receipt))
        })
    }
}

pub(super) async fn tags(
    ctx: Ctx,
    (): (),
    _: contract::ResourceTagsSetBody,
) -> ApiResult<contract::ResourceTagsSetReceipt> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Tags>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    publication::receipt(&done.ctx, &done.output)?;
    Ok(done.output)
}
