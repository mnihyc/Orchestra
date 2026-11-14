//! Root-scoped model choices use the native model, Use and policy producer.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::Selected;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::operation_sources::Binding;

use orch_api_contract::operation_sources::Cardinality;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Root,
    Providers,
}

const SOURCES: Declaration<Source> = Declaration {
    operation: "AskModelsGet",
    bindings: &[
        Binding {
            source: Source::Root,
            target: TargetResolver::Resolved("ask_model_root"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Providers,
            target: TargetResolver::ResolvedCollection("ask_model_providers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

struct Models;

impl Selected for Models {
    type Source = Source;
    type Query = api::AskModelsQuery;
    type Value = api::AskModelsReply;
    type Reply = api::AskModelsReply;
    const DECLARATION: &'static Declaration<Source> = &SOURCES;
    const WATCH_ORDINARY: bool = true;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a orch_authz::Caller,
        query: &'a Self::Query,
    ) -> native::SourceFuture<'a, Self::Value, Source> {
        Box::pin(async move {
            let root = crate::key_operations::targets::node(conn, query.root_org_id).await?;
            let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
            plan.bind(
                0,
                KeyTargetKind::Object(ObjectKind::Org),
                std::slice::from_ref(&root),
            )?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            let ordinary = principal.ordinary(conn).await.map_err(ApiError::internal)?;
            root_access(conn, &ordinary, query.root_org_id).await?;
            let reply = resolve(conn, &principal, query.root_org_id).await?;
            let identities = reply
                .models
                .iter()
                .filter_map(|model| model.provider.as_ref().map(|provider| provider.id))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|id| orch_access_contract::decisions::ObjectRef {
                    kind: ObjectKind::Provider,
                    id,
                })
                .collect::<Vec<_>>();
            let providers =
                crate::key_operations::targets::resource_objects(conn, &identities).await?;
            let provider_groups = if providers.is_empty() {
                vec![]
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Provider), providers)]
            };
            Ok((
                reply,
                vec![
                    native::Facts::one(Source::Root, KeyTargetKind::Object(ObjectKind::Org), root),
                    native::Facts::collection(Source::Providers, provider_groups),
                ],
            ))
        })
    }

    // Selection, rendering and refresh each use the actual query root. This API has
    // no path parameter from which the generic pre-selection hook can infer it.
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let ordinary = principal.ordinary(conn).await.map_err(ApiError::internal)?;
            root_access(conn, &ordinary, value.root_org_id).await?;
            Ok(value.clone())
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        query: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let ordinary = principal.ordinary(conn).await.map_err(ApiError::internal)?;
            root_access(conn, &ordinary, query.root_org_id).await?;
            // Eligibility, schemas and edit hints must still describe the producing body.
            Ok(serde_json::to_value(opened).map_err(ApiError::internal)?
                == serde_json::to_value(fresh).map_err(ApiError::internal)?)
        })
    }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    query: api::AskModelsQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    native::selected::<Models>(ctx, conn, caller, query, ())
}

pub(super) async fn get(
    ctx: Ctx,
    query: api::AskModelsQuery,
    (): (),
) -> ApiResult<api::AskModelsReply> {
    native::selected_read::<Models>(ctx, query, ()).await
}

/// The configured Ask list, current native eligibility and the settings view's exact write hint.
pub(crate) async fn resolve(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
) -> ApiResult<api::AskModelsReply> {
    use api::AskModelsBlockedReason as Blocked;
    let mut views = crate::routes::settings::readers::resolved_views(
        conn,
        principal,
        root,
        Some(&["ask.models"]),
    )
    .await?;
    let (view, permitted) = views
        .pop()
        .ok_or_else(|| ApiError::internal("ask.models is absent from the setting registry"))?;
    let configured = view
        .value()
        .map(|value| serde_json::from_value::<Vec<api::AskModelIdentity>>(value.clone()))
        .transpose()
        .map_err(ApiError::internal)?;
    let writable = view.into_setting(permitted).writable;
    let ordinary = principal.ordinary(conn).await.map_err(ApiError::internal)?;
    let models = crate::routes::threads::binding::ask_models(conn, &ordinary, root).await?;
    let default_model = models
        .iter()
        .find(|model| model.allowed)
        .map(|model| model.selection.clone());
    let blocked_reason = if configured.is_none() {
        Some(Blocked::NotConfigured)
    } else if configured.as_ref().is_some_and(Vec::is_empty) {
        Some(Blocked::Empty)
    } else if default_model.is_some() {
        None
    } else {
        let policy = setting::<Vec<String>>(conn, Place::Node(root), "models.allowed")
            .await
            .map_err(ApiError::internal)?;
        if policy.as_ref().is_some_and(Vec::is_empty)
            || models
                .iter()
                .all(|model| model.unavailable_reason.as_deref() == Some("excluded_by_policy"))
        {
            Some(Blocked::ExcludedByPolicy)
        } else {
            Some(Blocked::NoUsableModels)
        }
    };
    Ok(api::AskModelsReply {
        root_org_id: root,
        default_model,
        models,
        blocked_reason,
        writable,
    })
}

pub(crate) fn blocked(reason: api::AskModelsBlockedReason) -> ApiError {
    use api::AskModelsBlockedReason as Blocked;
    let message = match reason {
        Blocked::NotConfigured => "Ask is not configured. An administrator must set ask.models.",
        Blocked::Empty => {
            "Ask's configured model list is empty. An administrator must set ask.models."
        }
        Blocked::ExcludedByPolicy => {
            "The root organization's model policy excludes every configured Ask model."
        }
        Blocked::NoUsableModels => "No configured Ask model is usable by you in this organization.",
    };
    ApiError::invalid("model", message)
        .with("blocked_reason", reason)
        .with("setting", "ask.models")
}

pub(crate) async fn check_preference(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    model: Option<&api::AskModel>,
) -> ApiResult<()> {
    let Some(model) = model else {
        return Ok(());
    };
    let choices = resolve(conn, principal, root).await?;
    if let Some(reason) = choices.blocked_reason {
        return Err(blocked(reason));
    }
    if !choices.models.iter().any(|choice| {
        choice.allowed
            && choice.selection.provider_id == model.provider_id
            && choice.selection.model == model.model
    }) {
        return Err(ApiError::invalid(
            "model",
            "This model cannot be used from this organization's Ask list. Choose another model.",
        ));
    }
    Ok(())
}
