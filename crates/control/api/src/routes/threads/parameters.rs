//! What a thread's agent is set to, and what it could be set to: the model with the models its provider lists, the effort, and the parameters its harness declares, each with where its value comes from and whether the caller may change it.
//!
//! The values are the settings of the thread's place, resolved for the caller
//! by the same code that lists a node's settings; whether the caller may write
//! one is that code's answer too. A model the provider has switched off, or
//! that a policy of the place leaves out, is listed with the reason it cannot
//! be chosen.
//!
//! **Only what exists for the thread is listed.** A parameter is shown when it is eligible for the thread's
//! model, provider and place and its feature is on; a speed, an access program or a context that the
//! thread's model does not have open is not there, whatever the harness supports. Each parameter carries its
//! definition, with the range of a number (the most of a context is what the model's managers opened) and
//! where its options and its start come from. The value is what was chosen for the thread; what it runs with
//! when nothing was chosen, or when the choice is no longer open, is what the model starts with, which the
//! model's entry says beside it.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::settings::resolved_keys;
use orch_agent_contract::descriptor::{EffortSupport, Placement, ProviderKind};
use orch_agent_contract::parameters::Eligibility;
use orch_api_contract::settings::{
    ModelOption, ModelOptionReason, ModelParameters, ParameterApply, ParameterView,
    ResolvedSetting, SettingScope, SettingSource, ThreadEffortParameter, ThreadModelParameter,
    ThreadParameterGet, ThreadParameters,
};
use orch_db::queries::resources::models_of_provider;
use orch_engine::models::allowed_by;
use orch_engine::views::thread_parts;
use orch_settings_contract::registry::keys;
use serde_json::Value;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(ThreadParameterGet, parameters);
}

/// A setting that is not there, as the view of one that nobody wrote.
fn unset(key: &str) -> ResolvedSetting {
    ResolvedSetting {
        key: key.to_owned(),
        value: None,
        source: SettingSource {
            scope: SettingScope::Default,
            node: None,
        },
        own: None,
        locked_by: None,
        writable: false,
        parts: Vec::new(),
        automation_reason: None,
        not_applicable_reason: None,
        available_values: None,
    }
}

/// Where the value of one member of the agent's parameters comes from: the nearest place whose
/// row names it and that no lock above leaves out. The members of the parameters are written at
/// different places by different people, and each says its own; a member nobody named is the
/// default's, whatever else the rows name.
fn member_source(params: &ResolvedSetting, member: &str) -> SettingSource {
    params
        .parts
        .iter()
        .rev()
        .filter(|part| !part.ignored)
        .find(|part| part.value.get(member).is_some())
        .map_or(
            SettingSource {
                scope: SettingScope::Default,
                node: None,
            },
            |part| part.source.clone(),
        )
}

/// A value of one contract read as the same words of another.
fn again<T: serde::Serialize, U: serde::de::DeserializeOwned>(value: &T) -> Option<U> {
    serde_json::from_value(serde_json::to_value(value).ok()?).ok()
}

/// What the thread's agent is set to.
pub(super) async fn parameters(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadParameters> {
    let thread = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let found = resolved_keys(
        &mut conn,
        &principal,
        thread,
        &[keys::AGENT_MODEL, keys::AGENT_EFFORT, keys::AGENT_PARAMS],
    )
    .await?;
    let of = |key: &str| {
        found
            .iter()
            .find(|setting| setting.key == key)
            .cloned()
            .unwrap_or_else(|| unset(key))
    };
    let (model, effort, params) = (
        of(keys::AGENT_MODEL),
        of(keys::AGENT_EFFORT),
        of(keys::AGENT_PARAMS),
    );
    let parts = thread_parts(&mut conn, thread, None, orch_types::Time::new(ctx.now))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let settings = orch_engine::resolved::load(&mut conn, thread)
        .await
        .map_err(ApiError::internal)?;
    let current = model
        .value
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut options = Vec::new();
    if let Some(binding) = &parts.binding {
        for row in models_of_provider(&mut conn, binding.provider.id).await? {
            let left_out = settings
                .models_allowed
                .as_ref()
                .is_some_and(|allowed| !allowed_by(allowed, &row.model_name, &row.groups));
            let reason = if !row.enabled {
                Some(ModelOptionReason::Disabled)
            } else if left_out {
                Some(ModelOptionReason::ExcludedByPolicy)
            } else {
                None
            };
            options.push(ModelOption {
                token_estimate: orch_engine::token_density::of(&row.model_name),
                groups: row.groups,
                display_name: if row.display_name.is_empty() {
                    row.model_name.clone()
                } else {
                    row.display_name.clone()
                },
                model_name: row.model_name,
                parameters: serde_json::from_value::<ModelParameters>(row.parameters)
                    .unwrap_or_default(),
                allowed: reason.is_none(),
                reason,
            });
        }
    }
    let default_effort = options
        .iter()
        .find(|option| Some(&option.model_name) == current.as_ref())
        .and_then(|option| option.parameters.default_effort.clone());
    let descriptor = parts
        .binding
        .as_ref()
        .and_then(|binding| binding.descriptor.as_ref());
    let supported =
        descriptor.is_some_and(|descriptor| descriptor.capabilities.effort != EffortSupport::None);
    let mut review_models = Vec::new();
    if orch_agent_programs::Programs::builtin().auto_possible()
        && descriptor.is_some_and(|descriptor| {
            descriptor
                .approval_modes
                .contains(&orch_agent_contract::descriptor::ApprovalMode::Auto)
        })
        && settings
            .allowed_modes
            .contains(&orch_agent_contract::descriptor::ApprovalMode::Auto)
        && let Some(binding) = parts.binding.as_ref()
    {
        let row = orch_db::queries::threads::thread(&mut conn, thread)
            .await?
            .ok_or_else(ApiError::not_found)?;
        review_models = ctx
            .app
            .engine
            .review_models(
                &mut conn,
                &row,
                binding.provider.id,
                Some(principal.user()),
                &settings,
            )
            .await
            .map_err(ApiError::internal)?;
    }
    let approval_modes = match descriptor {
        Some(descriptor) => ctx
            .app
            .engine
            .offered_approval_modes(
                &mut conn,
                parts
                    .facts
                    .environment
                    .as_ref()
                    .map(|environment| environment.id),
                descriptor,
                &settings.allowed_modes,
                &review_models,
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|mode| {
                serde_json::to_value(mode)
                    .map_err(ApiError::internal)
                    .map(|mode| mode.as_str().unwrap_or_default().to_owned())
            })
            .collect::<ApiResult<Vec<_>>>()?,
        None => Vec::new(),
    };
    let eligibility = parts.binding.as_ref().map(|binding| Eligibility {
        credential_path: again(&binding.credential_path)
            .unwrap_or(orch_agent_contract::credentials::CredentialPath::Native),
        // This explicit parameter predicate is about the credential. The chosen access is
        // carried independently above, including native and forwarded access on the same provider.
        provider_kind: match binding.provider.kind {
            orch_api_contract::resources::ProviderKind::Api => ProviderKind::Api,
            orch_api_contract::resources::ProviderKind::Subscription => ProviderKind::Subscription,
        },
        environment_placement: parts
            .facts
            .environment
            .as_ref()
            .and_then(|environment| again(&environment.placement))
            .unwrap_or(Placement::Managed),
        model: options
            .iter()
            .find(|option| Some(&option.model_name) == current.as_ref())
            .and_then(|option| again(&option.parameters))
            .unwrap_or_default(),
        harness_version: None,
    });
    let declared: Vec<_> = descriptor
        .map(|descriptor| descriptor.parameters.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|def| {
            eligibility
                .as_ref()
                .is_some_and(|eligibility| def.is_shown(eligibility, &settings.features))
        })
        .collect();
    let values = params.value.clone().unwrap_or(Value::Null);
    let parameters = declared
        .into_iter()
        .map(|def| ParameterView {
            value: values.get(&def.key).cloned(),
            key: def.key.clone(),
            apply: match def.apply {
                orch_agent_contract::parameters::ParameterApply::NextTurn => {
                    ParameterApply::NextTurn
                }
                orch_agent_contract::parameters::ParameterApply::NewSession => {
                    ParameterApply::NewSession
                }
                orch_agent_contract::parameters::ParameterApply::RecreateEnvironment => {
                    ParameterApply::RecreateEnvironment
                }
            },
            source: member_source(&params, &def.key),
            def: serde_json::to_value(&def).unwrap_or(Value::Null),
            locked_by: params.locked_by.clone(),
            writable: params.writable,
        })
        .collect();
    let provider = match &parts.binding {
        Some(binding) => {
            let holding = orch_authz::resources::effective(
                &mut conn,
                &principal,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ResourceKind::Provider.into(),
                    id: binding.provider.id,
                },
                orch_authz::Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            if holding.held.iter().any(|held| {
                held.permission == orch_access_contract::permissions::Permission::ProviderView
            }) {
                Some(
                    crate::routes::resources::providers::show(
                        &ctx,
                        &mut conn,
                        &principal,
                        binding.provider.id,
                    )
                    .await?,
                )
            } else {
                None
            }
        }
        None => None,
    };
    Ok(ThreadParameters {
        provider,
        model: ThreadModelParameter {
            value: current,
            source: model.source,
            writable: model.writable,
            options,
        },
        effort: ThreadEffortParameter {
            value: effort
                .value
                .as_ref()
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(default_effort),
            source: effort.source,
            writable: effort.writable,
            supported,
        },
        parameters,
        approval_mode: serde_json::to_value(settings.approval_mode)
            .map_err(ApiError::internal)?
            .as_str()
            .unwrap_or("on_request")
            .to_owned(),
        approval_modes: Some(approval_modes),
        approval_reviewer: orch_engine::approval_review::session_reviewer(&mut conn, thread)
            .await
            .map_err(ApiError::internal)?,
        subagent_limits: Some(orch_api_contract::settings::ThreadSubagentLimits {
            limits: settings.subagent_limits,
            supported: descriptor
                .filter(|descriptor| descriptor.capabilities.subagents)
                .map(|descriptor| {
                    descriptor
                        .capabilities
                        .subagent_limits
                        .iter()
                        .filter_map(|control| {
                            serde_json::to_value(control)
                                .ok()
                                .and_then(|value| value.as_str().map(str::to_owned))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }),
    })
}
