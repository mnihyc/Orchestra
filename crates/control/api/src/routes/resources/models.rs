//! The models a provider lists, with the name that is sent and the name that is shown, adding, changing, enabling and removing them, and the models that discovery suggests, asked of the machine that holds the key.
//!
//! A provider's models are its explicit allowlist: nothing is usable on it that is not listed and
//! enabled, and discovery never adds to it by itself. The routing name is the model's identity and is
//! taken as it is given; two models of one provider never show under one name.
//!
//! The entry of a model also says what it can be run with at the provider: its efforts, the speeds and the
//! access programs its managers enabled, the largest context a thread may be given, and what a thread on it
//! starts with of each. Whatever costs more or reaches further is enabled by a manager and by no one else,
//! so accepting a suggestion puts what it lists under `offered` and enables none of it. The groups a manager
//! puts a model in are the names by which a cap, an allowlist or an entry's models name several at once.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::same;
use crate::routes::nodes::{actor_name, invalid};
use crate::routes::resources::providers::{model_view, provider_for, refresh_standing};
use crate::routes::resources::{Home, record};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    self as api, MODEL_GROUP_NAME_MAX, MODEL_GROUPS_DEFAULT, ModelParameters, ModelSuggestion,
    ProviderModel, is_model_group_name,
};
use orch_db::DbError;
use orch_db::queries::resources::routes::{
    self as queries, ModelChange, ModelRow, NewModel, ProviderRow,
};
use orch_types::{Id, Json};
use serde_json::json;

/// The longest a routing name is: what the schema allows.
const ROUTING_NAME_MAX_CHARS: usize = 200;
/// The longest a display name is: what the schema allows.
const DISPLAY_NAME_MAX_CHARS: usize = 120;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            api::ProviderModelList,
            crate::key_operations::direct::declared,
            list,
        )
        .on_with_operation(
            api::ProviderModelCreate,
            crate::key_operations::direct::declared,
            create,
        )
        .creation(api::ProviderModelCreate, model_creation_binding())
        .on_with_operation(
            api::ProviderModelUpdate,
            crate::key_operations::direct::declared,
            update,
        )
        .on(api::ProviderModelDelete, delete)
        .on(api::ProviderModelSuggestionList, suggestions)
        .on_with_operation(
            api::ProviderModelSuggestionAccept,
            super::provider_projection::prepare,
            accept,
        );
}

/// A routing name: taken exactly as it is given, so long as it is something that can be sent.
fn clean_routing_name(path: &str, name: &str) -> ApiResult<()> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(invalid(
            path,
            "a routing name is the name sent upstream, as it is written there",
        ));
    }
    if name.chars().count() > ROUTING_NAME_MAX_CHARS {
        return Err(invalid(
            path,
            &format!("a routing name has at most {ROUTING_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(())
}

/// A display name as it is kept: without the space around it, and not longer than the schema
/// allows. Empty means that the routing name is shown.
fn clean_display_name(path: &str, name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.chars().count() > DISPLAY_NAME_MAX_CHARS || name.chars().any(char::is_control) {
        return Err(invalid(
            path,
            &format!("a display name has at most {DISPLAY_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}

/// The id of the model of `models` that already goes by `routing` or shows as `display`, apart from
/// `except`.
fn clash(
    models: &[ModelRow],
    routing: Option<&str>,
    display: Option<&str>,
    except: Option<Id>,
) -> Option<Id> {
    models
        .iter()
        .filter(|model| Some(model.id) != except)
        .find(|model| {
            routing.is_some_and(|name| model.model_name == name)
                || display.filter(|name| !name.is_empty()).is_some_and(|name| {
                    model.display_name.eq_ignore_ascii_case(name)
                        || model.display_name.to_lowercase() == name.to_lowercase()
                })
        })
        .map(|model| model.id)
}

fn exists(ctx: &Ctx, existing: Id) -> ApiError {
    let error = ApiError::new(
        ErrorCode::ConflictExists,
        "this provider has a model by that name already",
    );
    if ctx.key_receipt_required() {
        error
    } else {
        error.with("existing_id", existing)
    }
}

/// Groups retain their written order without duplicates. Names use one to MODEL_GROUP_NAME_MAX
/// lower-case letters, digits, '-' and '_', starting with a letter or digit.
fn clean_groups(path: &str, groups: &[String]) -> ApiResult<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        if !is_model_group_name(group) {
            return Err(invalid(
                &format!("{path}[{index}]"),
                &format!(
                    "a group is named by 1 to {MODEL_GROUP_NAME_MAX} of a-z, 0-9, - and _, beginning with a letter or a digit"
                ),
            ));
        }
        if !kept.contains(group) {
            kept.push(group.clone());
        }
    }
    Ok(kept)
}

fn check_group_count(path: &str, groups: &[String], maximum: i64) -> ApiResult<()> {
    if i64::try_from(groups.len()).unwrap_or(i64::MAX) > maximum {
        return Err(invalid(
            path,
            &format!("a model is in at most {maximum} groups here"),
        ));
    }
    Ok(())
}

pub(super) async fn group_limit(conn: &mut orch_db::Conn, home: Id) -> ApiResult<i64> {
    orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Node(home),
        orch_settings_contract::registry::keys::PROVIDERS_MODEL_GROUPS_MAX,
    )
    .await
    .map_err(ApiError::internal)
    .map(|value| value.unwrap_or(MODEL_GROUPS_DEFAULT))
}

/// What an entry's parameters may not be, for the refusals the model requests give: a default that is not
/// among what is enabled (an effort the model lists, a speed or an access program a manager enabled), a
/// `context_max` below the model's own window, a `default_context` above `context_max` or given without
/// one, and a size that is no size.
fn check_parameters(parameters: &ModelParameters) -> ApiResult<()> {
    parameters
        .check_option_schemas()
        .map_err(|reason| invalid("parameters.request_options", reason))?;
    if parameters
        .web_search_modes()
        .iter()
        .any(|mode| !matches!(mode.as_str(), "disabled" | "cached" | "indexed" | "live"))
    {
        return Err(invalid(
            "parameters.web_search",
            "search modes are disabled, cached, indexed or live",
        ));
    }
    if parameters
        .default_web_search
        .as_deref()
        .is_some_and(|mode| {
            mode != "disabled"
                && !parameters
                    .web_search_modes()
                    .iter()
                    .any(|enabled| enabled == mode)
        })
    {
        return Err(invalid(
            "parameters.default_web_search",
            "the default search mode must be enabled on the model",
        ));
    }
    for (member, size) in [
        ("context_window", parameters.context_window),
        ("context_max", parameters.context_max),
        ("default_context", parameters.default_context),
    ] {
        if size.is_some_and(|size| size < 1) {
            return Err(invalid(
                &format!("parameters.{member}"),
                "a number of tokens is at least one",
            ));
        }
    }
    if let Some(effort) = &parameters.default_effort
        && !parameters
            .efforts
            .as_deref()
            .unwrap_or_default()
            .contains(effort)
    {
        return Err(invalid(
            "parameters.default_effort",
            "an effort a thread starts with is one of the efforts the model lists",
        ));
    }
    if let Some(tier) = &parameters.default_service_tier
        && !parameters
            .service_tiers
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|enabled| &enabled.id == tier)
    {
        return Err(invalid(
            "parameters.default_service_tier",
            "a speed a thread starts with is one of the speeds enabled on the model",
        ));
    }
    if let Some(mode) = &parameters.default_reasoning_mode
        && !parameters.reasoning_modes().contains(mode)
    {
        return Err(invalid(
            "parameters.default_reasoning_mode",
            "a reasoning mode a thread starts with is one of the modes enabled on the model",
        ));
    }
    if let Some(tier) = &parameters.default_capacity_tier
        && !parameters
            .capacity_tiers()
            .iter()
            .any(|enabled| &enabled.id == tier)
    {
        return Err(invalid(
            "parameters.default_capacity_tier",
            "a capacity tier a thread starts with is one of the tiers enabled on the model",
        ));
    }
    if let Some(program) = &parameters.default_access_program
        && !parameters
            .access_programs
            .as_deref()
            .unwrap_or_default()
            .contains(program)
    {
        return Err(invalid(
            "parameters.default_access_program",
            "an access program a thread starts with is one of the programs enabled on the model",
        ));
    }
    if let (Some(largest), Some(window)) = (parameters.context_max, parameters.context_window)
        && largest < window
    {
        return Err(invalid(
            "parameters.context_max",
            "the largest context is not below the model's own window",
        ));
    }
    if let Some(start) = parameters.default_context {
        match parameters.context_max {
            None => {
                return Err(invalid(
                    "parameters.default_context",
                    "a start for the context needs the largest context to be enabled",
                ));
            }
            Some(largest) if start > largest => {
                return Err(invalid(
                    "parameters.default_context",
                    "the context a thread starts with is not above the largest",
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn parameters_json(parameters: &ModelParameters) -> ApiResult<Json> {
    serde_json::to_value(parameters).map_err(ApiError::internal)
}

/// Lists the models of a provider.
async fn list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<ProviderModel>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    provider_for(&mut conn, &principal, id, Permission::ProviderView).await?;
    queries::models_of(&mut conn, &[id])
        .await?
        .iter()
        .map(model_view)
        .collect()
}

/// Adds a model to a provider, enabled.
async fn create(
    ctx: Ctx,
    (): (),
    body: api::ProviderModelCreateBody,
) -> ApiResult<api::ProviderModelWriteResponse> {
    let id = ctx.id("id")?;
    let definition = definition(&body)?;
    // The provider's Manage is asked again in the transaction that writes, once the body has come.
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    check_group_count(
        "groups",
        &definition.groups,
        group_limit(&mut tx, row.home_id).await?,
    )?;
    let models = queries::models_of(&mut tx, &[id]).await?;
    if let Some(existing) = clash(
        &models,
        Some(&definition.routing),
        Some(&definition.display),
        None,
    ) {
        return Err(exists(&ctx, existing));
    }
    let model = insert_native(&ctx, &mut tx, &row, &definition).await?;
    orch_db::idempotency::creation::observe(&mut tx, Some(principal.user()), MODEL_CREATION, model)
        .await?;
    tx.commit().await.map_err(DbError::from)?;
    model_reply(&ctx, principal.caller(), id, model, false).await
}

#[derive(Clone)]
pub(crate) struct Definition {
    routing: String,
    display: String,
    parameters: Json,
    groups: Vec<String>,
    enabled: bool,
}
fn definition(body: &api::ProviderModelCreateBody) -> ApiResult<Definition> {
    clean_routing_name("model_name", &body.model_name)?;
    let display = clean_display_name("display_name", body.display_name.as_deref().unwrap_or(""))?;
    let asked = body.parameters.clone().unwrap_or_default();
    check_parameters(&asked)?;
    Ok(Definition {
        routing: body.model_name.clone(),
        display,
        parameters: parameters_json(&asked)?,
        groups: clean_groups("groups", body.groups.as_deref().unwrap_or_default())?,
        enabled: true,
    })
}
/// Source rows already identify the readable configuration. No discovery or usage is retained.
pub(crate) fn copied_definitions(rows: &[ModelRow]) -> ApiResult<Vec<Definition>> {
    let mut routing = std::collections::BTreeSet::new();
    let mut display = std::collections::BTreeSet::new();
    let mut prepared = Vec::with_capacity(rows.len());
    for row in rows {
        let mut next = definition(&api::ProviderModelCreateBody {
            model_name: row.model_name.clone(),
            display_name: Some(row.display_name.clone()),
            parameters: Some(
                serde_json::from_value(row.parameters.clone()).map_err(ApiError::internal)?,
            ),
            groups: Some(row.groups.clone()),
        })?;
        if !routing.insert(next.routing.clone())
            || !next.display.is_empty() && !display.insert(next.display.to_lowercase())
        {
            return Err(invalid("models", "the copied model names must be distinct"));
        }
        next.enabled = row.enabled;
        prepared.push(next);
    }
    Ok(prepared)
}
/// A genuinely new provider has no old models or paused users. Validate the whole batch
/// against its current native group limit before using the same ordinary model writer.
pub(crate) async fn copy_into_new(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    provider: Id,
    definitions: &[Definition],
) -> ApiResult<()> {
    if definitions.is_empty() {
        return Ok(());
    }
    let row = queries::providers(conn, &[provider])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if !queries::models_of(conn, &[provider]).await?.is_empty() {
        return Err(ApiError::internal(
            "the new provider already has model configuration",
        ));
    }
    let limit = group_limit(conn, row.home_id).await?;
    for definition in definitions {
        check_group_count("groups", &definition.groups, limit)?;
    }
    for definition in definitions {
        insert_native(ctx, conn, &row, definition).await?;
    }
    Ok(())
}
async fn insert_native(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    row: &ProviderRow,
    definition: &Definition,
) -> ApiResult<Id> {
    let model = orch_types::new_id();
    queries::insert_model(
        conn,
        &NewModel {
            id: model,
            provider_id: row.id,
            model_name: &definition.routing,
            display_name: &definition.display,
            parameters: &definition.parameters,
            groups: &definition.groups,
            source: "manual",
            created_by: ctx.user()?.id(),
        },
        ctx.now,
    )
    .await?;
    if !definition.enabled {
        queries::change_model(
            conn,
            row.id,
            model,
            &ModelChange {
                enabled: Some(false),
                ..Default::default()
            },
            ctx.now,
        )
        .await?;
    }
    models_changed(ctx, conn, row, json!({"added":[definition.routing]})).await?;
    refresh_standing(ctx, conn, row.id).await?;
    Ok(model)
}

const MODEL_CREATION: &str = "provider_model";
fn model_creation_binding() -> crate::middleware::idempotency::Creation<
    (),
    api::ProviderModelCreateBody,
    api::ProviderModelWriteResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: MODEL_CREATION,
        replay: replay_model_creation,
        identity: |value| {
            let model = match value {
                api::ProviderModelWriteResponse::Full(v) => v.id,
                api::ProviderModelWriteResponse::Receipt(v) => v.model_id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: MODEL_CREATION.into(),
                id: model,
                data: Json::Null,
            })
        },
    }
}
fn replay_model_creation(
    ctx: Ctx,
    (): (),
    _: api::ProviderModelCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> futures::future::BoxFuture<'static, ApiResult<api::ProviderModelWriteResponse>> {
    Box::pin(async move {
        if created.kind != MODEL_CREATION {
            return Err(ApiError::internal(
                "the accepted provider model kind changed",
            ));
        }
        let provider = ctx.id("id")?;
        let receipt = api::ProviderModelWriteResponse::Receipt(api::ProviderModelWriteReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            provider_id: provider,
            model_id: created.id,
        });
        crate::key_operations::publication::receipt(&ctx, &receipt)?;
        let ctx = ctx.for_completed_replay_projection();
        let current = crate::middleware::authorization::caller_of(ctx.user()?)?;
        match model_reply(&ctx, &current, provider, created.id, true).await {
            Ok(value) => Ok(value),
            Err(e) if matches!(e.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(e) => Err(e),
        }
    })
}

/// Project the actual current model only through this write's optional Provider Read.
async fn model_reply(
    ctx: &Ctx,
    accepted: &orch_authz::Caller,
    provider: Id,
    model: Id,
    replaying: bool,
) -> ApiResult<api::ProviderModelWriteResponse> {
    let full = crate::key_operations::completed_projection::run(ctx, async {
        let mut reading = ctx.app.db.begin_snapshot().await?;
        let full = if replaying {
            super::provider_projection::read_completed_replay(
                ctx,
                &mut reading,
                accepted,
                provider,
                move |conn, _, _| Box::pin(current_model(conn, provider, model)),
            )
            .await?
        } else {
            super::provider_projection::read(
                ctx,
                &mut reading,
                accepted,
                provider,
                move |conn, _, _| Box::pin(current_model(conn, provider, model)),
            )
            .await?
        };
        reading.commit().await.map_err(DbError::from)?;
        Ok(full)
    })
    .await?;
    Ok(match full {
        Some(model) => api::ProviderModelWriteResponse::full(model),
        None => api::ProviderModelWriteResponse::Receipt(api::ProviderModelWriteReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            provider_id: provider,
            model_id: model,
        }),
    })
}

async fn current_model(
    conn: &mut orch_db::Conn,
    provider: Id,
    model: Id,
) -> ApiResult<api::ProviderModel> {
    let row = queries::model_of(conn, provider, model)
        .await?
        .ok_or_else(ApiError::not_found)?;
    model_view(&row)
}

/// Writes the audit row of a change to the models a provider lists.
pub(crate) async fn models_changed(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    provider: &ProviderRow,
    details: Json,
) -> ApiResult<()> {
    let who = actor_name(ctx)?;
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::ProviderModelsChanged),
        provider.id,
        Home {
            node: Some(provider.home_id),
            org: Some(provider.org_id),
        },
        &format!("{who} changed the models of the provider {}", provider.name),
        details,
    )
    .await
}

/// Changes a model: its display name, what it can do, whether it may be chosen. Its routing name
/// is not changed: another model is added.
async fn update(
    ctx: Ctx,
    query: orch_api_contract::resource_facts::ResourcePauseQuery,
    body: api::ProviderModelUpdateBody,
) -> ApiResult<api::ProviderModelWriteResponse> {
    let id = ctx.id("id")?;
    let note = super::impact::note(query.note.as_deref())?;
    if note.is_some() && body.enabled != Some(false) {
        return Err(invalid(
            "note",
            "is supplied only when switching the model off",
        ));
    }
    let model_id = ctx.id("model_id")?;
    let display = body
        .display_name
        .as_deref()
        .map(|name| clean_display_name("display_name", name))
        .transpose()?;
    if let Some(parameters) = &body.parameters {
        check_parameters(parameters)?;
    }
    let groups = body
        .groups
        .as_deref()
        .map(|groups| clean_groups("groups", groups))
        .transpose()?;
    // The provider's Manage is asked again in the transaction that writes, once the body has come.
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let stored = queries::model_of(&mut tx, id, model_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(groups) = &groups
        && groups.iter().any(|group| !stored.groups.contains(group))
    {
        check_group_count("groups", groups, group_limit(&mut tx, row.home_id).await?)?;
    }
    if let Some(display) = &display {
        let models = queries::models_of(&mut tx, &[id]).await?;
        if let Some(existing) = clash(&models, None, Some(display), Some(model_id)) {
            return Err(exists(&ctx, existing));
        }
    }
    // What the harness or the endpoint last reported stays when a manager writes what is on.
    let parameters = match &body.parameters {
        Some(parameters) => {
            let mut written = parameters_json(parameters)?;
            if written.get("offered").is_none()
                && let (Some(members), Some(offered)) =
                    (written.as_object_mut(), stored.parameters.get("offered"))
            {
                members.insert("offered".to_owned(), offered.clone());
            }
            Some(written)
        }
        None => None,
    };
    let change = ModelChange {
        display_name: display.as_deref(),
        parameters: parameters.as_ref(),
        groups: groups.as_deref(),
        enabled: body.enabled,
    };
    let target = orch_api_contract::resource_facts::FactTarget {
        object: orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Provider,
            id,
        },
        model_id: Some(model_id),
    };
    let mut stopping = None;
    let mut enable_target = None;
    let mut resumed = Vec::new();
    if let Some(enabled) = body.enabled.filter(|enabled| *enabled != stored.enabled) {
        if enabled {
            if orch_engine::resource_facts::activity::stopping(&mut tx, &target)
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::PlanBlocked,
                    "Waiting for the accepted stop to finish",
                ));
            }
            enable_target = Some(target);
        } else {
            let name = if stored.display_name.is_empty() {
                &stored.model_name
            } else {
                &stored.display_name
            };
            stopping = Some(
                ctx.app
                    .engine
                    .stop_resource_in(
                        &mut tx,
                        orch_engine::resource_pause::ResourceStop {
                            target,
                            name,
                            by: principal.user(),
                            note,
                            deleted: false,
                        },
                        ctx.now,
                    )
                    .await
                    .map_err(ApiError::internal)?,
            );
        }
    }
    queries::change_model(&mut tx, id, model_id, &change, ctx.now).await?;
    if let Some(enabled) = body.enabled {
        sqlx::query("UPDATE provider_models SET pause_note=$2 WHERE id=$1 AND provider_id=$3")
            .bind(model_id)
            .bind(if enabled { None } else { note })
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
    }
    models_changed(
        &ctx,
        &mut tx,
        &row,
        json!({
            "changed": stored.model_name,
            "display_name": display,
            "parameters": parameters.is_some(),
            "groups": groups,
            "enabled": body.enabled,
        }),
    )
    .await?;
    refresh_standing(&ctx, &mut tx, id).await?;
    let changed = queries::model_of(&mut tx, id, model_id)
        .await?
        .ok_or_else(|| ApiError::internal("a model was changed and is not there"))?;
    if let Some(target) = enable_target {
        let finished = ctx
            .app
            .engine
            .enable_resource_waiting_in(
                &mut tx,
                orch_engine::resource_pause::ResourceResume {
                    target: &target,
                    name: if changed.display_name.is_empty() {
                        &changed.model_name
                    } else {
                        &changed.display_name
                    },
                    by: principal.user(),
                    run_waiting: query.run_waiting.unwrap_or(false),
                },
                ctx.now,
            )
            .await
            .map_err(ApiError::internal)?;
        resumed.extend(finished.wakes);
    }
    tx.commit().await.map_err(DbError::from)?;
    for thread in resumed {
        ctx.app.engine.wake(thread);
    }
    if let Some((job, finished)) = stopping {
        for thread in finished.wakes {
            ctx.app.engine.wake(thread);
        }
        ctx.app
            .engine
            .stop_resource_now(job.id)
            .await
            .map_err(ApiError::internal)?;
    }
    model_reply(&ctx, principal.caller(), id, model_id, false).await
}

/// Removes a model from a provider.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let model_id = ctx.id("model_id")?;
    // The provider's Manage is asked again in the transaction that writes.
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let stored = queries::model_of(&mut tx, id, model_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let target = orch_api_contract::resource_facts::FactTarget {
        object: orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Provider,
            id,
        },
        model_id: Some(model_id),
    };
    let name = if stored.display_name.is_empty() {
        &stored.model_name
    } else {
        &stored.display_name
    };
    let (job, finished) = ctx
        .app
        .engine
        .stop_resource_in(
            &mut tx,
            orch_engine::resource_pause::ResourceStop {
                target,
                name,
                by: principal.user(),
                note: None,
                deleted: true,
            },
            ctx.now,
        )
        .await
        .map_err(ApiError::internal)?;
    queries::delete_model(&mut tx, id, model_id).await?;
    models_changed(
        &ctx,
        &mut tx,
        &row,
        json!({ "removed": [stored.model_name] }),
    )
    .await?;
    refresh_standing(&ctx, &mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    for thread in finished.wakes {
        ctx.app.engine.wake(thread);
    }
    ctx.app
        .engine
        .stop_resource_now(job.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

// ---- discovery ----

/// What discovery could not do: the provider cannot be asked.
fn unavailable(detail: &str) -> ApiError {
    ApiError::new(ErrorCode::ProviderUnavailable, detail.to_owned())
}

/// Asks the machine that holds the key, or else the one the provider's runtime is, which models
/// the harness or the endpoint lists. A provider that has no such machine, or whose machine is
/// away, cannot be asked.
async fn discover(ctx: &Ctx, row: &ProviderRow) -> ApiResult<Vec<ModelSuggestion>> {
    let found = ctx
        .app
        .engine
        .discover_provider_models(row.id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| unavailable("the provider's current model list could not be read"))?;
    found
        .iter()
        .map(|each| {
            same(each).ok_or_else(|| ApiError::internal("a suggestion does not read as the API's"))
        })
        .collect()
}

/// Lists the models that discovery suggests; nothing is added by it.
async fn suggestions(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<ModelSuggestion>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let row = queries::providers(&mut conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    drop(conn);
    let found = discover(&ctx, &row).await?;
    let (mut tx, guard) = Box::pin(crate::middleware::transaction_guard::begin(&ctx)).await?;
    provider_for(&mut tx, guard.principal(), id, Permission::ProviderManage).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(found)
}

/// What a suggestion becomes when it is accepted: the speeds and access programs it lists, and a context
/// larger than the model's own window, are on offer, and none of them is enabled; a manager enables one by
/// writing it in. What a thread starts with is a manager's choice and never discovery's, so no start comes
/// from a suggestion.
fn accepted_parameters(suggested: &ModelParameters) -> ApiResult<Json> {
    let mut parameters = suggested.clone();
    let mut offered = parameters.offered.take().unwrap_or_default();
    if let Some(modes) = parameters.web_search.take() {
        offered.web_search.get_or_insert(modes);
    }
    if let Some(lifetimes) = parameters.cache_lifetimes.take() {
        offered.cache_lifetimes.get_or_insert(lifetimes);
    }
    parameters.default_web_search = None;
    if let Some(modes) = parameters.reasoning_modes.take() {
        offered.reasoning_modes.get_or_insert(modes);
    }
    parameters.default_reasoning_mode = None;
    if let Some(tiers) = parameters.service_tiers.take() {
        offered.service_tiers.get_or_insert(tiers);
    }
    if let Some(tiers) = parameters.capacity_tiers.take() {
        offered.capacity_tiers.get_or_insert(tiers);
    }
    parameters.default_capacity_tier = None;
    if let Some(programs) = parameters.access_programs.take() {
        offered.access_programs.get_or_insert(programs);
    }
    if let Some(largest) = parameters.context_max.take() {
        offered.context_max.get_or_insert(largest);
    }
    parameters.default_service_tier = None;
    parameters.default_access_program = None;
    parameters.default_context = None;
    parameters.request_options = None;
    // A context that is no more than the model's own window offers nothing.
    if let (Some(largest), Some(window)) = (offered.context_max, parameters.context_window)
        && largest <= window
    {
        offered.context_max = None;
    }
    if offered != api::ModelOffered::default() {
        parameters.offered = Some(offered);
    }
    parameters_json(&parameters)
}

/// Adds the suggested models that were chosen, enabled, none of their speeds or access programs
/// enabled.
async fn accept(
    ctx: Ctx,
    (): (),
    body: api::ProviderModelSuggestionAcceptBody,
) -> ApiResult<api::ProviderModelSuggestionAcceptResponse> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let row = queries::providers(&mut conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    drop(conn);
    let found = discover(&ctx, &row).await?;
    // A completed discovery cannot lend its old authority to validation or writes.
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let mut chosen: Vec<&ModelSuggestion> = Vec::new();
    for (index, name) in body.model_names.iter().enumerate() {
        let Some(suggestion) = found.iter().find(|each| &each.model_name == name) else {
            return Err(invalid(
                &format!("model_names[{index}]"),
                "discovery does not suggest this model",
            ));
        };
        if !chosen
            .iter()
            .any(|each| each.model_name == suggestion.model_name)
        {
            chosen.push(suggestion);
        }
    }
    let mut models = queries::models_of(&mut tx, &[id]).await?;
    let mut added: Vec<Id> = Vec::new();
    for suggestion in chosen {
        // A model the provider lists already is not added a second time.
        if models
            .iter()
            .any(|model| model.model_name == suggestion.model_name)
        {
            continue;
        }
        let display = clean_display_name("display_name", &suggestion.display_name)?;
        if let Some(existing) = clash(&models, None, Some(&display), None) {
            if added.contains(&existing) {
                return Err(invalid(
                    "model_names",
                    "these suggestions have the same display name; add one and rename it before adding the other",
                ));
            }
            return Err(exists(&ctx, existing));
        }
        let model = orch_types::new_id();
        let parameters = accepted_parameters(&suggestion.parameters)?;
        queries::insert_model(
            &mut tx,
            &NewModel {
                id: model,
                provider_id: id,
                model_name: &suggestion.model_name,
                display_name: &display,
                parameters: &parameters,
                groups: &[],
                source: "suggested",
                created_by: ctx.user()?.id(),
            },
            ctx.now,
        )
        .await?;
        added.push(model);
        models = queries::models_of(&mut tx, &[id]).await?;
    }
    if !added.is_empty() {
        let names: Vec<&String> = models
            .iter()
            .filter(|model| added.contains(&model.id))
            .map(|model| &model.model_name)
            .collect();
        models_changed(
            &ctx,
            &mut tx,
            &row,
            json!({ "added": names, "source": "suggested" }),
        )
        .await?;
        refresh_standing(&ctx, &mut tx, id).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    let projection = ctx.clone();
    let actual = added.clone();
    let full = crate::key_operations::completed_projection::run(&ctx, async {
        let mut reading = ctx.app.db.begin_snapshot().await?;
        let full = super::provider_projection::read(
            &ctx,
            &mut reading,
            principal.caller(),
            id,
            move |conn, reader, prepared| {
                Box::pin(async move {
                    let models = queries::models_of(conn, &[id]).await?;
                    let added = models
                        .iter()
                        .filter(|model| actual.contains(&model.id))
                        .map(model_view)
                        .collect::<ApiResult<Vec<_>>>()?;
                    let provider = super::providers::show_prepared(
                        &projection,
                        conn,
                        reader,
                        prepared.ok_or_else(|| {
                            ApiError::internal("the bounded provider projection is absent")
                        })?,
                    )
                    .await?;
                    Ok(api::ProviderModelSuggestionAcceptReply { added, provider })
                })
            },
        )
        .await?;
        reading.commit().await.map_err(DbError::from)?;
        Ok(full)
    })
    .await?;
    Ok(match full {
        Some(full) => api::ProviderModelSuggestionAcceptResponse::full(full),
        None => api::ProviderModelSuggestionAcceptResponse::Receipt(
            api::ProviderModelSuggestionAcceptReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                provider_id: id,
                model_ids: added,
            },
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::resources::{ModelOffered, ServiceTier};

    #[test]
    fn a_routing_name_is_taken_as_it_is_given_and_a_display_name_is_trimmed() {
        assert!(clean_routing_name("model_name", "openai/gpt-6-sol").is_ok());
        assert!(
            clean_routing_name("model_name", " padded ").is_ok(),
            "taken as it is given"
        );
        assert!(clean_routing_name("model_name", "   ").is_err());
        assert!(clean_routing_name("model_name", "two\nlines").is_err());
        assert!(clean_routing_name("model_name", &"m".repeat(201)).is_err());
        assert_eq!(
            clean_display_name("display_name", "  GPT-6 Sol (A) ").unwrap(),
            "GPT-6 Sol (A)"
        );
        assert!(clean_display_name("display_name", &"d".repeat(121)).is_err());
    }

    fn tier(id: &str) -> api::ServiceTier {
        api::ServiceTier {
            id: id.into(),
            label: id.into(),
        }
    }

    #[test]
    fn a_default_has_to_be_among_what_is_enabled_and_a_context_within_what_is_open() {
        let enabled = ModelParameters {
            efforts: Some(vec!["low".into(), "high".into()]),
            default_effort: Some("high".into()),
            service_tiers: Some(vec![tier("fast")]),
            default_service_tier: Some("fast".into()),
            access_programs: Some(vec!["blue".into()]),
            default_access_program: Some("blue".into()),
            context_window: Some(272_000),
            context_max: Some(1_000_000),
            default_context: Some(272_000),
            ..ModelParameters::default()
        };
        assert!(check_parameters(&enabled).is_ok());
        assert!(check_parameters(&ModelParameters::default()).is_ok());
        let refused = |change: fn(&mut ModelParameters), member: &str| {
            let mut parameters = enabled.clone();
            change(&mut parameters);
            let error = check_parameters(&parameters).unwrap_err();
            assert!(
                error
                    .problem
                    .detail
                    .starts_with(&format!("parameters.{member}:")),
                "{member}: {}",
                error.problem.detail
            );
        };
        refused(|p| p.default_effort = Some("max".into()), "default_effort");
        refused(
            |p| p.default_service_tier = Some("flex".into()),
            "default_service_tier",
        );
        refused(
            |p| p.default_access_program = Some("red".into()),
            "default_access_program",
        );
        refused(|p| p.context_max = Some(100_000), "context_max");
        refused(|p| p.default_context = Some(2_000_000), "default_context");
        refused(
            |p| {
                p.context_max = None;
                p.default_context = Some(100_000);
            },
            "default_context",
        );
        refused(|p| p.context_window = Some(0), "context_window");
        refused(|p| p.service_tiers = None, "default_service_tier");
        let mut open_only = enabled;
        open_only.context_max = None;
        open_only.default_context = None;
        assert!(
            check_parameters(&open_only).is_ok(),
            "a model without a larger context needs no start for it"
        );
    }

    #[test]
    fn reasoning_starts_require_enablement_and_suggestions_only_offer_them() {
        let mut parameters = ModelParameters {
            reasoning_modes: Some(vec!["pro".into()]),
            default_reasoning_mode: Some("pro".into()),
            ..Default::default()
        };
        assert!(check_parameters(&parameters).is_ok());
        let accepted = accepted_parameters(&parameters).unwrap();
        assert!(accepted.get("reasoning_modes").is_none());
        assert!(accepted.get("default_reasoning_mode").is_none());
        assert_eq!(
            accepted["offered"]["reasoning_modes"],
            serde_json::json!(["pro"])
        );
        parameters.reasoning_modes = None;
        let error = check_parameters(&parameters).unwrap_err();
        assert!(
            error
                .problem
                .detail
                .starts_with("parameters.default_reasoning_mode:")
        );
    }

    #[test]
    fn capacity_starts_require_enablement_and_suggestions_never_open_capacity() {
        let mut parameters = ModelParameters {
            capacity_tiers: Some(vec![tier("auto")]),
            default_capacity_tier: Some("auto".into()),
            ..Default::default()
        };
        assert!(check_parameters(&parameters).is_ok());
        let accepted = accepted_parameters(&parameters).unwrap();
        assert!(accepted.get("capacity_tiers").is_none());
        assert!(accepted.get("default_capacity_tier").is_none());
        assert_eq!(accepted["offered"]["capacity_tiers"][0]["id"], "auto");
        parameters.capacity_tiers = None;
        assert!(
            check_parameters(&parameters)
                .unwrap_err()
                .problem
                .detail
                .starts_with("parameters.default_capacity_tier:")
        );
    }

    #[test]
    fn accepting_a_cache_lifetime_suggestion_only_offers_the_additional_lifetime() {
        let parameters = ModelParameters {
            cache_lifetimes: Some(vec!["1h".into()]),
            ..Default::default()
        };
        let accepted = accepted_parameters(&parameters).unwrap();
        assert!(accepted.get("cache_lifetimes").is_none());
        assert_eq!(accepted["offered"]["cache_lifetimes"], json!(["1h"]));
    }

    #[test]
    fn group_names_are_deduplicated_and_the_current_configured_count_is_independent() {
        let names = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            clean_groups("groups", &names(&["frontier", "vision", "frontier"])).unwrap(),
            ["frontier", "vision"],
            "a group named twice is one"
        );
        assert!(clean_groups("groups", &[]).unwrap().is_empty());
        assert!(clean_groups("groups", &names(&["Frontier"])).is_err());
        assert!(clean_groups("groups", &names(&["-x"])).is_err());
        assert!(clean_groups("groups", &names(&[""])).is_err());
        assert!(clean_groups("groups", &["g".repeat(41)]).is_err());
        let nine: Vec<String> = (0..9).map(|n| format!("g{n}")).collect();
        let groups = clean_groups("groups", &nine).unwrap();
        assert!(check_group_count("groups", &groups, MODEL_GROUPS_DEFAULT).is_err());
        assert!(check_group_count("groups", &groups[..8], MODEL_GROUPS_DEFAULT).is_ok());
        assert!(check_group_count("groups", &groups, 9).is_ok());
        assert!(check_group_count("groups", &[], 0).is_ok());
        assert!(check_group_count("groups", &groups, 0).is_err());
    }

    #[test]
    fn a_larger_context_that_a_suggestion_lists_is_on_offer_and_no_start_comes_from_discovery() {
        let suggested = ModelParameters {
            context_window: Some(272_000),
            context_max: Some(1_000_000),
            default_service_tier: Some("fast".into()),
            default_access_program: Some("blue".into()),
            default_context: Some(500_000),
            default_effort: Some("high".into()),
            efforts: Some(vec!["low".into(), "high".into()]),
            ..ModelParameters::default()
        };
        let kept = accepted_parameters(&suggested).unwrap();
        assert!(kept.get("context_max").is_none(), "none of it is enabled");
        assert_eq!(kept["offered"]["context_max"], 1_000_000);
        assert_eq!(kept["context_window"], 272_000);
        for member in [
            "default_service_tier",
            "default_access_program",
            "default_context",
        ] {
            assert!(kept.get(member).is_none(), "{member} is a manager's choice");
        }
        assert_eq!(kept["default_effort"], "high", "an effort is not a cost");
        let same = ModelParameters {
            context_window: Some(272_000),
            offered: Some(api::ModelOffered {
                web_search: None,
                context_max: Some(272_000),
                ..api::ModelOffered::default()
            }),
            ..ModelParameters::default()
        };
        assert!(
            accepted_parameters(&same).unwrap().get("offered").is_none(),
            "a context no larger than the window offers nothing"
        );
    }

    #[test]
    fn accepting_a_report_without_enabled_options_preserves_its_future_facts() {
        let suggested: ModelParameters = serde_json::from_value(json!({
            "offered":{"report":{"capabilities":{"pdf_input":{"supported":true},"future_capability":{"supported":true}},
            "output_max":64000,"lifecycle":"deprecated","retires_at":"2027-01-01T00:00:00Z","future_member":{"kept":true}}}
        })).unwrap();
        let kept = accepted_parameters(&suggested).unwrap();
        assert_eq!(
            kept["offered"],
            serde_json::to_value(&suggested.offered).unwrap()
        );
        assert!(kept.get("web_search").is_none());
        assert!(kept.get("service_tiers").is_none());
        assert!(kept.get("context_max").is_none());
        assert!(check_parameters(&suggested).is_ok());
    }

    #[test]
    fn what_a_suggestion_lists_is_on_offer_and_none_of_it_is_enabled() {
        let suggested = ModelParameters {
            service_tiers: Some(vec![ServiceTier {
                id: "fast".into(),
                label: "Fast".into(),
            }]),
            access_programs: Some(vec!["early".into()]),
            context_window: Some(200_000),
            ..ModelParameters::default()
        };
        let kept = accepted_parameters(&suggested).unwrap();
        assert!(kept.get("service_tiers").is_none() && kept.get("access_programs").is_none());
        assert_eq!(kept["offered"]["service_tiers"][0]["id"], "fast");
        assert_eq!(kept["offered"]["access_programs"][0], "early");
        assert_eq!(kept["context_window"], 200_000);
        let plain = accepted_parameters(&ModelParameters::default()).unwrap();
        assert!(plain.get("offered").is_none());
        let _ = ModelOffered::default();
    }
    #[test]
    fn search_offers_never_enable_the_tool_or_its_default() {
        let suggested: ModelParameters =
            serde_json::from_value(json!({"web_search":["live"],"default_web_search":"live"}))
                .unwrap();
        let accepted = accepted_parameters(&suggested).unwrap();
        assert_eq!(accepted["offered"]["web_search"], json!(["live"]));
        assert!(accepted.get("web_search").is_none());
        assert!(accepted.get("default_web_search").is_none());
        let invalid: ModelParameters =
            serde_json::from_value(json!({"default_web_search":"live"})).unwrap();
        assert!(check_parameters(&invalid).is_err());
    }
}
