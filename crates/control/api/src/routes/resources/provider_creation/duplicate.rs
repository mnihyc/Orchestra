//! Duplication is another factual source for the existing provider creator, including its initial Make.
use super::*;
use crate::key_operations::native;
use crate::routes::{duplicates, resources};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::{
    duplicate_sources::{self as declarations, PROVIDER, Source as Role},
    duplicates::{
        DuplicateBody, DuplicateKind, DuplicateReceipt, MoveOutBody, MoveOutDeleteEffect,
        MoveOutDeletePlan, ProviderDuplicate, ProviderMoveOut, ProviderMoveOutPreview,
    },
    resources::{ProviderCreateResponse, Upstream},
};
use orch_db::{
    DbError,
    queries::resources::{grouping, routes as rows},
};
use std::collections::{BTreeMap, BTreeSet};
mod configuration;

#[derive(Clone)]
pub(super) struct Source {
    pub provider: Id,
    pub moving: Option<orch_types::Hash>,
    delete: Option<MoveOutDeletePlan>,
    tags: Vec<Id>,
    models: Vec<resources::models::Definition>,
    group_options: orch_types::Json,
    service_offerings: orch_types::Json,
    runtime_secrets: BTreeMap<String, String>,
}
struct MovingIdentity;
impl duplicates::CreationIdentity for MovingIdentity {
    const CREATED_KIND: DuplicateKind = DuplicateKind::Provider;
    const CREATED_REPLAY: &'static str = "provider_move_out";
}
struct Identity;
impl duplicates::CreationIdentity for Identity {
    const CREATED_KIND: DuplicateKind = DuplicateKind::Provider;
    const CREATED_REPLAY: &'static str = "provider_duplicate";
}
pub(in crate::routes::resources) fn mount(registry: &mut crate::registry::Registry) {
    registry.on_with_operation(ProviderDuplicate, prepare, copy);
    registry.on_with_operation(ProviderMoveOut, prepare_move, move_out);
    registry.creation(
        ProviderMoveOut,
        duplicates::creation_binding::<MovingIdentity, MoveOutBody>(),
    );
    registry.on_snapshot_body_read_with_optional_operation(
        ProviderMoveOutPreview,
        preview_prepare,
        preview,
    );
    registry.creation(
        ProviderDuplicate,
        duplicates::creation_binding::<Identity, DuplicateBody>(),
    );
}
pub(super) fn creator_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::Unreadable(_) => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the original provider copy is no longer authorized",
        ),
        error => ApiError::engine(error),
    }
}
enum Intent {
    Copy(DuplicateBody),
    Preview(DuplicateBody),
    Move(MoveOutBody),
}
fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DuplicateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (value, _) = prepare_native(ctx, conn, caller, Intent::Copy(body)).await?;
        Ok(value.plan.clone().with_data(value))
    })
}
fn prepare_move<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: MoveOutBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (value, _) = prepare_native(ctx, conn, caller, Intent::Move(body)).await?;
        Ok(value.plan.clone().with_data(value))
    })
}
async fn prepare_native(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    intent: Intent,
) -> ApiResult<(Operands, Vec<native::Facts<Role>>)> {
    let preview = matches!(&intent, Intent::Preview(_));
    let moving = !matches!(&intent, Intent::Copy(_));
    let declaration = match &intent {
        Intent::Copy(_) => &PROVIDER,
        Intent::Preview(_) => &declarations::PROVIDER_MOVE_PREVIEW,
        Intent::Move(_) => &declarations::PROVIDER_MOVE,
    };
    let (body, confirmation) = match intent {
        Intent::Copy(body) | Intent::Preview(body) => (body, None),
        Intent::Move(body) => (body.copy, Some(body.delete_plan)),
    };
    let id = ctx.id("id")?;
    let locked: Option<Id> = sqlx::query_scalar(if preview {
        "SELECT id FROM providers WHERE id=$1 AND deleted_at IS NULL"
    } else {
        "SELECT id FROM providers WHERE id=$1 AND deleted_at IS NULL FOR UPDATE"
    })
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if locked.is_none() {
        return Err(ApiError::not_found());
    }
    let row = rows::providers(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let home = duplicates::destination(&body, Some(row.home_id)).ok_or_else(|| {
        crate::routes::nodes::invalid("destination", "a provider needs an org or project")
    })?;
    let mut facts = duplicates::origins(conn, ObjectKind::Provider, id, Some(home)).await?;
    if preview {
        facts.push(native::Facts::one(
            Role::PreviewContent,
            KeyTargetKind::Object(ObjectKind::Provider),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id,
                },
            )
            .await?,
        ));
    }
    let plan = native::bind(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    resources::require(
        conn,
        &principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderView,
    )
    .await?;
    duplicates::revision(&body, row.revision)?;
    duplicates::copy_to(
        conn,
        &principal,
        duplicates::subject(DuplicateKind::Provider, id),
        Some(home),
    )
    .await?;
    let (_, kind) = crate::routes::nodes::placed(conn, home).await?;
    if !matches!(
        kind,
        orch_api_contract::nodes::NodeKind::Org | orch_api_contract::nodes::NodeKind::Project
    ) {
        return Err(ApiError::invalid(
            "destination",
            "a provider needs an org or project",
        ));
    }
    match orch_authz::ownership::may_make_here(conn, &principal, ResourceKind::Provider, Some(home))
        .await
    {
        Ok(orch_authz::ownership::Making::Allowed(_)) => {}
        Ok(orch_authz::ownership::Making::Refused(why)) => {
            return Err(resources::providers::refused_making(conn, home, why).await);
        }
        Err(error) => return Err(authorization::refusals::problem(conn, error).await),
    }
    let name = duplicates::display_name(&body, &row.name);
    let owner = duplicates::owns(
        conn,
        &principal,
        ObjectRef {
            kind: ObjectKind::Provider,
            id,
        },
    )
    .await?;
    let (moving, delete) = if moving {
        if !owner {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "only an actual owner can move this provider into another organization",
            ));
        }
        resources::require(
            conn,
            &principal,
            ResourceKind::Provider,
            id,
            Permission::ProviderManage,
        )
        .await?;
        let destination_org = duplicates::copy_to(
            conn,
            &principal,
            duplicates::subject(DuplicateKind::Provider, id),
            Some(home),
        )
        .await?;
        if destination_org == Some(row.org_id) {
            return Err(ApiError::invalid(
                "destination",
                "use the existing Move within this organization",
            ));
        }
        if row.kind == "subscription" && row.held_by_environment_id.is_some() {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::ResourceInUse,
                "return the existing handed-over sign-in before moving its provider",
            ));
        }
        if let Some(job) = orch_engine::providers::deleting::pending_move(conn, id, None)
            .await
            .map_err(ApiError::engine)?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::ResourceInUse,
                "the original provider's sign-in or copy is still being settled",
            )
            .with("job_id", job));
        }
        let fingerprint = orch_engine::providers::deleting::fingerprint(conn, &row)
            .await
            .map_err(ApiError::engine)?;
        let delete = duplicates::moving::plan(
            orch_api_contract::duplicates::CopySubject {
                kind: orch_api_contract::duplicates::CopySubjectKind::Provider,
                id,
            },
            orch_api_contract::duplicates::DuplicateDestination::Node { id: home },
            row.revision,
            principal.user(),
            &body,
            serde_json::json!({"native_source":fingerprint}),
            vec![
                MoveOutDeleteEffect::StopBoundWork,
                MoveOutDeleteEffect::RemoveSource,
            ],
        )?;
        if let Some(confirmation) = &confirmation {
            duplicates::moving::confirm(&delete, confirmation)?;
        }
        (Some(fingerprint), Some(delete))
    } else {
        (None, None)
    };
    let sources = configuration::sources(conn, id, &row.sources, owner).await?;
    let (runtime_config, runtime_secrets) =
        configuration::runtime(conn, id, &row.runtime_config, owner).await?;
    let model_ids: Vec<Id> = sqlx::query_scalar(if preview {
        "SELECT id FROM provider_models WHERE provider_id=$1 ORDER BY id"
    } else {
        "SELECT id FROM provider_models WHERE provider_id=$1 ORDER BY id FOR SHARE"
    })
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let model_rows = rows::models_of(conn, &[id]).await?;
    if model_ids.len() != model_rows.len() {
        return Err(ApiError::internal("the copied model selection changed"));
    }
    let models = resources::models::copied_definitions(&model_rows)?;
    let group_options = resources::providers::checked_model_groups(
        &serde_json::from_value(row.model_group_options.clone()).map_err(ApiError::internal)?,
    )?;

    let request = match row.kind.as_str() {
        "api" => {
            let upstreams: Vec<Upstream> =
                serde_json::from_value(row.upstreams).map_err(ApiError::internal)?;
            let policy = resources::providers::home_policies(conn, home).await?;
            let checked = resources::upstreams::check(&upstreams, policy.http_allowed)?;
            let mut names: BTreeSet<String> = checked
                .secret_headers
                .iter()
                .map(|name| resources::sealing::header_secret(name))
                .collect();
            names.insert(resources::sealing::API_KEY.into());
            let mut secrets = if owner {
                duplicates::secret_values(conn, "provider", id, &names).await?
            } else {
                BTreeMap::new()
            };
            let api_key = secrets.remove(resources::sealing::API_KEY);
            let headers = checked
                .secret_headers
                .iter()
                .filter_map(|name| {
                    secrets
                        .remove(&resources::sealing::header_secret(name))
                        .map(|value| (name.clone(), value))
                })
                .collect();
            CreateProvider::Api {
                name,
                home_id: home,
                upstreams,
                api_key,
                secrets: Some(headers),
                runtime_machine_id: None,
                key_machine_id: None,
                access: None,
                arrangements: None,
                runtime_config: Some(runtime_config),
                sources,
            }
        }
        "subscription" => {
            let agent = row
                .agent_id
                .ok_or_else(|| ApiError::internal("the subscription has no native profile"))?;
            let _profile = orch_db::queries::resources::thread_engine::agent(conn, agent)
                .await?
                .filter(|profile| profile.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let mut runtime_config = runtime_config;
            runtime_config
                .as_object_mut()
                .ok_or_else(|| {
                    ApiError::internal("the copied runtime configuration changed shape")
                })?
                .insert(
                    orch_settings_contract::registry::keys::RUNTIME_HOME.into(),
                    serde_json::json!({"kind":"made_by_orch"}),
                );
            CreateProvider::Subscription {
                name,
                home_id: home,
                agent_id: agent,
                config: Some(row.config),
                sign_in: None,
                runtime_machine_id: None,
                key_machine_id: None,
                access: None,
                arrangements: None,
                runtime_config: Some(runtime_config),
                sources,
            }
        }
        _ => return Err(ApiError::internal("the provider kind is not native")),
    };
    let tags = duplicates::copy_tags(conn, ResourceKind::Provider, id, Some(home)).await?;
    duplicates::current(ctx, principal.caller())?;
    Ok((
        Operands {
            request,
            home_overridden: true,
            plan,
            duplicate: Some(Source {
                provider: id,
                moving,
                delete,
                tags,
                models,
                group_options,
                service_offerings: row.service_offerings.clone(),
                runtime_secrets,
            }),
        },
        facts,
    ))
}
async fn move_out(ctx: Ctx, (): (), _body: MoveOutBody) -> ApiResult<DuplicateReceipt> {
    copy_native(ctx).await
}
async fn copy(ctx: Ctx, (): (), _body: DuplicateBody) -> ApiResult<DuplicateReceipt> {
    copy_native(ctx).await
}
async fn copy_native(ctx: Ctx) -> ApiResult<DuplicateReceipt> {
    let request = ctx.operation_data::<Operands>()?.request.clone();
    let response = ctx.clone();
    let made = resources::providers::create(ctx, (), request).await?;
    let id = match made {
        ProviderCreateResponse::Full(value) => value.provider.base.id,
        ProviderCreateResponse::Receipt(value) => value.provider_id,
    };
    let receipt = DuplicateReceipt {
        projection: ReceiptProjection::Receipt,
        kind: DuplicateKind::Provider,
        id,
    };
    crate::key_operations::publication::receipt(&response, &receipt)?;
    Ok(receipt)
}
pub(super) async fn observed(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
    operands: &Operands,
) -> ApiResult<()> {
    let source = operands
        .duplicate
        .as_ref()
        .ok_or_else(|| ApiError::internal("the duplicate source is absent"))?;
    if source.moving.is_none() {
        duplicates::record_copy(ctx, conn, DuplicateKind::Provider, source.provider, id).await?;
    }
    resources::models::copy_into_new(ctx, conn, id, &source.models).await?;
    configuration::write_group_options(
        ctx,
        conn,
        id,
        &source.group_options,
        &source.service_offerings,
    )
    .await?;
    if !source.tags.is_empty() {
        grouping::set_tags(conn, ResourceKind::Provider, id, &source.tags, ctx.now).await?;
    }
    orch_db::idempotency::creation::observe(
        conn,
        Some(ctx.user()?.id()),
        if source.moving.is_some() {
            "provider_move_out"
        } else {
            "provider_duplicate"
        },
        id,
    )
    .await?;
    let current = transaction_guard::current_http_identity(
        ctx,
        conn,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    let (accepted, until) = orch_engine::jobs::provider_provision::copied_creator_checkpoint(
        conn,
        id,
        ctx.app.clock.as_ref(),
    )
    .await
    .map_err(creator_error)?;
    ctx.publication.retain_ceiling(&accepted)?;
    ctx.publication.retain_deadline(until)?;
    current.check_deadline(&ctx.app)?;
    duplicates::current(ctx, &accepted)
}

pub(super) async fn write_runtime_secrets(
    ctx: &Ctx,
    conn: &mut Conn,
    provider: Id,
    source: &Source,
) -> ApiResult<()> {
    let names = resources::providers::runtime_secret_names(conn, provider).await?;
    for (name, value) in &source.runtime_secrets {
        if name
            .strip_prefix("runtime:")
            .is_some_and(|name| names.iter().any(|allowed| allowed == name))
        {
            resources::sealing::put_provider_secret(
                conn,
                provider,
                name,
                value,
                ctx.user()?.id(),
                ctx.now,
            )
            .await?;
        }
    }
    Ok(())
}

struct Preview;
fn preview_prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DuplicateBody,
) -> crate::registry::OptionalOperationFuture<'a> {
    native::selected::<Preview>(ctx, conn, caller, body, ())
}
async fn preview(ctx: Ctx, (): (), body: DuplicateBody) -> ApiResult<MoveOutDeletePlan> {
    native::selected_read::<Preview>(ctx, body, ()).await
}
impl native::Selected for Preview {
    type Source = Role;
    type Query = DuplicateBody;
    type Value = Operands;
    type Reply = MoveOutDeletePlan;
    const DECLARATION: &'static orch_api_contract::operation_sources::Declaration<Role> =
        &declarations::PROVIDER_MOVE_PREVIEW;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        body: &'a DuplicateBody,
    ) -> futures::future::BoxFuture<'a, ApiResult<(Operands, Vec<native::Facts<Role>>)>> {
        Box::pin(prepare_native(
            ctx,
            conn,
            caller,
            Intent::Preview(body.clone()),
        ))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> futures::future::BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Operands,
    ) -> futures::future::BoxFuture<'a, ApiResult<MoveOutDeletePlan>> {
        Box::pin(async move {
            value
                .duplicate
                .as_ref()
                .and_then(|source| source.delete.clone())
                .ok_or_else(|| ApiError::internal("the provider has no native Move plan"))
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a DuplicateBody,
        old: &'a Operands,
        fresh: &'a Operands,
    ) -> futures::future::BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old
                .duplicate
                .as_ref()
                .and_then(|source| source.delete.as_ref())
                == fresh
                    .duplicate
                    .as_ref()
                    .and_then(|source| source.delete.as_ref()))
        })
    }
}
