//! Native setting prepare/apply and optional readback share the canonical source rows.

use super::*;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::native;

use crate::key_operations::publication;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::settings_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::queries::hierarchy;

use orch_db::queries::resources as resource_rows;

use orch_db::queries::threads;

use orch_engine::setting_operations as operation;

use orch_settings::write::PreparedClear;

use orch_settings::write::PreparedWrite;

use orch_settings::write::WriteFailure;

use std::collections::BTreeSet;

use std::marker::PhantomData;
pub(super) struct Editor<K>(PhantomData<K>);

pub(super) type Set = Editor<operation::Set>;

pub(super) type Clear = Editor<operation::Clear>;

pub(super) trait Input: operation::Kind {
    const VALIDATE_STANDING: bool;
    type Query: Send + 'static;
    type Body: Send + 'static;
    fn request(query: Self::Query, body: Self::Body) -> Request;
}

pub(super) enum Request {
    Set(api::NodeSettingSetBody),
    Clear(api::NodeSettingDeleteQuery),
}

impl Input for operation::Set {
    const VALIDATE_STANDING: bool = true;
    type Query = ();
    type Body = api::NodeSettingSetBody;
    fn request(_: (), body: Self::Body) -> Request {
        Request::Set(body)
    }
}

impl Input for operation::Clear {
    const VALIDATE_STANDING: bool = false;
    type Query = api::NodeSettingDeleteQuery;
    type Body = ();
    fn request(query: Self::Query, _: ()) -> Request {
        Request::Clear(query)
    }
}

enum Edit {
    Set(PreparedWrite),
    Clear(PreparedClear),
}

impl Edit {
    fn changes_lock(&self) -> bool {
        match self {
            Self::Set(value) => value.changes_lock(),
            Self::Clear(value) => value.changes_lock(),
        }
    }
    fn proposed(&self, actor: Actor, at: chrono::DateTime<chrono::Utc>) -> SettingView {
        match self {
            Self::Set(value) => value.proposed_view(actor, at),
            Self::Clear(value) => value.proposed_view(),
        }
    }
}

pub(super) struct Prepared {
    node: Id,
    key: String,
    edit: Edit,
    needs: validation::Needs,
    before: Option<Before>,
    provider: Option<Id>,
    required: Vec<native::Facts<Source>>,
}

pub(super) struct Outcome {
    node: Id,
    key: String,
    written: orch_settings::write::Written,
    change: operation::Change,
    required: Vec<native::Facts<Source>>,
}

async fn lock_footprint(conn: &mut Conn, node: Id, kind: NodeKind, key: &str) -> ApiResult<()> {
    if kind == NodeKind::Thread {
        threads::tools::lock_threads(conn, &[node]).await?;
    }
    let rows = hierarchy::with_ancestors(conn, &[node]).await?;
    let ids = rows
        .into_iter()
        .map(|row| row.id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for id in &ids {
        hierarchy::lock(conn, *id).await?;
    }
    sqlx::query("SELECT node_id,key FROM settings WHERE key=$1 AND (node_id=ANY($2) OR node_id IS NULL) ORDER BY node_id NULLS FIRST FOR UPDATE")
        .bind(key).bind(&ids).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    Ok(())
}

async fn lock_need(conn: &mut Conn, need: Need) -> ApiResult<()> {
    match need {
        Need::OnEnvironment { environment, .. } => {
            resource_rows::lock_resource(conn, "environment", environment).await?;
        }
        Need::OnProvider { provider, .. } => {
            resource_rows::lock_resource(conn, "provider", provider).await?;
        }
        _ => {}
    }
    Ok(())
}

impl<K: Input> effects::Effect for Editor<K> {
    type Source = Source;
    type Query = K::Query;
    type Body = K::Body;
    type Value = Prepared;
    type Output = Outcome;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        K::DECLARATION
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: K::Query,
        body: K::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let node = ctx.id("id")?;
            let key = ctx.param("key")?.to_owned();
            let needs = setting_needs(conn, node, &key).await?;
            let preliminary = operation::subject_facts(conn, node, needs, false, None)
                .await
                .map_err(ApiError::engine)?;
            let known = native::bind_known(ctx, K::DECLARATION, &preliminary)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let (_, kind) = placed(conn, node).await?;
            let policy = policy_at(conn, node).await?;
            permit(conn, &principal, node, &key, policy).await?;
            let request = K::request(query, body);
            if let Request::Set(body) = &request {
                super::super::resources::validation::setting(&ctx.app.engine, &key, &body.value)?;
                super::super::resources::validation::named_things_exist(conn, &key, &body.value)
                    .await?;
            }
            lock_footprint(conn, node, kind, &key).await?;
            let needs = setting_needs(conn, node, &key).await?;
            lock_need(conn, needs.value).await?;
            let policy = policy_at(conn, node).await?;
            let may_lock = permit_needs(conn, &principal, needs, policy, None).await?;
            let actor = Actor::User(principal.user());
            let edit = match request {
                Request::Set(body) => {
                    let request = Write {
                        actor,
                        target: Target::Node(node),
                        key: &key,
                        value: body.value,
                        locked: body.locked,
                        expected_revision: body.expected_revision,
                        may_lock,
                        origin: &origin(ctx),
                    };
                    match orch_settings::write::prepare_write(conn, &request).await {
                        Ok(value) => Edit::Set(value),
                        Err(error) => return Err(write_failed(conn, &principal, node, error).await),
                    }
                }
                Request::Clear(query) => {
                    let request = orch_settings::write::Clear {
                        actor,
                        target: Target::Node(node),
                        key: &key,
                        expected_revision: query.expected_revision,
                        may_lock,
                        origin: &origin(ctx),
                    };
                    match orch_settings::write::prepare_clear(conn, &request).await {
                        Ok(value) => Edit::Clear(value),
                        Err(error) => return Err(write_failed(conn, &principal, node, error).await),
                    }
                }
            };
            let proposed = edit.proposed(actor, ctx.now);
            let provider = if kind == NodeKind::Thread
                && (key == keys::AGENT_MODEL || key == keys::AGENT_EFFORT)
            {
                let model = if key == keys::AGENT_MODEL {
                    proposed.value().and_then(Json::as_str).map(str::to_owned)
                } else {
                    orch_engine::resolved::load(conn, node)
                        .await
                        .map_err(ApiError::engine)?
                        .model
                };
                if model.is_some() {
                    threads::thread(conn, node)
                        .await?
                        .and_then(|row| row.provider_id)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(provider) = provider {
                resource_rows::lock_resource(conn, "provider", provider).await?;
            }
            let required =
                operation::subject_facts(conn, node, needs, edit.changes_lock(), provider)
                    .await
                    .map_err(ApiError::engine)?;
            let before = before_on_a_thread(conn, node, kind, &key).await?;
            Ok((
                Prepared {
                    node,
                    key,
                    edit,
                    needs,
                    before,
                    provider,
                    required: required.clone(),
                },
                required,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let policy = policy_at(conn, value.node).await?;
            let may_lock = permit_needs(conn, principal, value.needs, policy, None).await?;
            if value.edit.changes_lock() && !may_lock {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the setting lock requires its ordinary authority",
                ));
            }
            Ok(())
        })
    }
}

impl<K: Input> effects::Apply for Editor<K> {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Outcome>>> {
        Box::pin(async move {
            let actor = Actor::User(principal.user());
            let written = match &value.edit {
                Edit::Set(prepared) => {
                    orch_settings::write::apply_write_before(
                        conn,
                        prepared,
                        actor,
                        &origin(ctx),
                        ctx.now,
                        move || before.check(),
                    )
                    .await
                }
                Edit::Clear(prepared) => {
                    orch_settings::write::apply_clear_before(
                        conn,
                        prepared,
                        actor,
                        &origin(ctx),
                        ctx.now,
                        move || before.check(),
                    )
                    .await
                }
            };
            let written = match written {
                Ok(written) => written,
                Err(WriteFailure::BeforeEffect(error)) => return Err(error),
                Err(WriteFailure::Native(error)) => {
                    return Err(write_failed(conn, principal, value.node, error).await);
                }
            };
            if K::VALIDATE_STANDING
                && written.changed
                && value.key == keys::AGENT_INSTRUCTIONS
                && let Some(characters) =
                    orch_engine::sessions::standing_past_limit(conn, value.node)
                        .await
                        .map_err(ApiError::engine)?
            {
                let limit = orch_engine::sessions::STANDING_LIMIT;
                let scope = if native::modern(principal.caller()) {
                    None
                } else {
                    view::refs(conn, principal, &[value.node], Policy::default())
                        .await?
                        .remove(&value.node)
                };
                return Err(Problem::new(ErrorCode::SettingsInstructionsTooLarge,
                        format!("the standing instructions would come to {characters} characters with their headings, and {limit} is the most"))
                        .with("limit", limit).with("characters", characters).with("scope", scope).into());
            }
            if written.changed {
                if value.key == keys::ENV_MOUNTS {
                    let environments = orch_db::queries::environments::hosting::environments_under(
                        conn, value.node,
                    )
                    .await?;
                    resource_rows::mounts::changed(conn, &environments, ctx.now).await?;
                }
                hierarchy::stamp(conn, value.node).await?;
                if let Some(before) = &value.before {
                    thread_changed(
                        conn,
                        ctx.now,
                        principal,
                        value.node,
                        &value.key,
                        Before {
                            model: before.model.clone(),
                            effort: before.effort.clone(),
                            value: before.value.clone(),
                        },
                    )
                    .await?;
                }
            }
            let change = operation::Change {
                node: value.node,
                key: value.key.clone(),
                fingerprint: operation::fingerprint(&written.view).map_err(ApiError::engine)?,
                lock_changed: value.edit.changes_lock(),
                provider: value.provider,
            };
            let changed = written.changed;
            let output = Outcome {
                node: value.node,
                key: value.key.clone(),
                written,
                change,
                required: value.required.clone(),
            };
            Ok(if changed {
                effects::Outcome::Applied(output)
            } else {
                effects::Outcome::Unchanged(output)
            })
        })
    }
}

pub(super) async fn run<K: Input>(
    ctx: Ctx,
    _query: K::Query,
    _body: K::Body,
) -> ApiResult<api::NodeSettingWriteResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Editor<K>>(&ctx, &mut tx).await?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&completed.ctx.app)?;
    tx.commit().await.map_err(DbError::from)?;
    let output = completed.output;
    let receipt = api::NodeSettingReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        node_id: output.node,
        key: output.key.clone(),
        own_revision: output.written.view.own.as_ref().map(|own| own.revision),
        changed: output.written.changed,
    };
    publication::receipt(&completed.ctx, &receipt)?;
    if native::modern(&completed.accepted) {
        let origin = orch_authz::CapturedOperation::capture(&completed.accepted)
            .map_err(ApiError::internal)?;
        if let Err(error) = completed
            .ctx
            .app
            .engine
            .setting_changed(&origin, output.change.clone())
            .await
        {
            tracing::warn!(node=%output.node,key=%output.key,%error,"accepted inherited setting changes remain pending");
        }
    } else {
        if output.key == keys::AUTH_SECOND_STEP {
            second_step_policy_changed(&completed.ctx).await;
        }
        if output.key == keys::ENV_MOUNTS
            && let Err(error) = completed
                .ctx
                .app
                .engine
                .mount_scope_changed(output.node)
                .await
        {
            tracing::warn!(node=%output.node,%error,"inherited mount changes remain pending");
        }
        if (output.key == keys::ENV_RESOURCES || output.key == keys::RUNTIME_RESOURCES)
            && let Err(error) = completed
                .ctx
                .app
                .engine
                .resource_scope_changed(output.node)
                .await
        {
            tracing::warn!(node=%output.node,%error,"inherited resource changes remain pending");
        }
        if matches!(
            output.key,
            keys::WORKSPACE_IGNORE
                | keys::WORKSPACE_INDEX
                | keys::WORKSPACE_INDEX_ENTRIES
                | keys::WORKSPACE_INDEX_SCAN_SECS
                | keys::WORKSPACE_IGNORE_MAX_BYTES
                | keys::WORKSPACE_IGNORE_CACHE_BYTES
                | keys::WORKSPACE_DIRECTORY_IDLE_SECS
                | keys::WORKSPACE_DIRECTORY_SLOTS
                | keys::WORKSPACE_DIRECTORY_CACHE_BYTES
                | keys::WORKSPACE_DIRECTORY_BUILDS
                | keys::WORKSPACE_DIRECTORY_PAGE_BYTES
                | keys::WORKSPACE_SEARCH_SLICE_SECS
                | keys::WORKSPACE_GIT_DIFF_BYTES
        ) && let Err(error) = completed
            .ctx
            .app
            .engine
            .workspace_scope_changed(output.node)
            .await
        {
            tracing::warn!(node=%output.node,%error,"inherited workspace changes remain pending");
        }
    }
    let full = reply::render::<Setting<K>>(&completed.ctx, &completed.accepted, output).await?;
    Ok(full.map_or_else(
        || api::NodeSettingWriteResponse::Receipt(receipt),
        api::NodeSettingWriteResponse::full,
    ))
}

struct Setting<K>(PhantomData<K>);

struct Names {
    bounds: NameBounds,
}

impl<K: Input> reply::Projection for Setting<K> {
    type Source = Source;
    type Input = Outcome;
    type Subject = Id;
    type Value = Names;
    type Reply = api::ResolvedSetting;
    const READ_SOURCE: Source = Source::SettingResult;
    fn declaration(_: &Outcome) -> &'static Declaration<Source> {
        K::DECLARATION
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Outcome,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let target = targets::node(conn, input.node).await?;
            let mut facts = input.required.clone();
            facts.push(native::Facts::one(
                Source::SettingResult,
                target.object.kind,
                target,
            ));
            Ok((input.node, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Outcome,
        node: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if *node == input.node {
                Ok(())
            } else {
                Err(ApiError::not_found())
            }
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        _node: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(Names, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = input
                .written
                .view
                .nodes()
                .into_iter()
                .collect::<BTreeSet<_>>();
            let graph =
                orch_authz::engine::Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(ApiError::internal)?;
            let visible = view::reference_visibility(
                conn,
                principal,
                &ids.iter().copied().collect::<Vec<_>>(),
                Policy::default(),
            )
            .await?;
            let plan = crate::registry::OperationPlan::new(ctx.route, K::DECLARATION.operation)?;
            let (facts, nodes) = native::node_name_facts(
                &plan,
                K::DECLARATION,
                principal.caller(),
                &graph,
                &visible,
                &[
                    (ObjectKind::Org, Source::OrgNames),
                    (ObjectKind::Project, Source::ProjectNames),
                    (ObjectKind::Topic, Source::TopicNames),
                    (ObjectKind::Thread, Source::ThreadNames),
                ],
            )?;
            Ok((
                Names {
                    bounds: NameBounds {
                        limited: native::modern(principal.caller()),
                        nodes,
                        ..NameBounds::default()
                    },
                },
                facts,
            ))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::ResolvedSetting>> {
        Box::pin(async move {
            let mut row = answer(
                conn,
                principal,
                input.node,
                input.written.view.clone(),
                &prepared.value.bounds,
            )
            .await?;
            add_mode_choices(
                ctx,
                conn,
                principal,
                Place::Node(input.node),
                std::slice::from_mut(&mut row),
            )
            .await?;
            Ok(row)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Outcome,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject == fresh.subject
                && opened
                    .value
                    .bounds
                    .nodes
                    .is_subset(&fresh.value.bounds.nodes))
        })
    }
}
