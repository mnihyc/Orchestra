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

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::access::page::NameBounds;

use orch_agent_contract::descriptor::EffortSupport;

use orch_agent_contract::descriptor::Placement;

use orch_agent_contract::descriptor::ProviderKind;

use orch_agent_contract::parameters::Eligibility;

use orch_api_contract::settings::ModelOption;

use orch_api_contract::settings::ModelOptionReason;

use orch_api_contract::settings::ModelParameters;

use orch_api_contract::settings::ParameterApply;

use orch_api_contract::settings::ParameterView;

use orch_api_contract::settings::ResolvedSetting;

use orch_api_contract::settings::SettingScope;

use orch_api_contract::settings::SettingSource;

use orch_api_contract::settings::ThreadEffortParameter;

use orch_api_contract::settings::ThreadModelParameter;

use orch_api_contract::settings::ThreadParameterGet;

use orch_api_contract::settings::ThreadParameters;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::resources::models_of_provider;

use orch_engine::models::allowed_by;

use orch_engine::views::thread_parts;

use orch_settings_contract::registry::keys;

use serde_json::Value;
/// Narrow declared controls with the same resolution the inspector publishes. A stored mode
/// stays visible as a stale choice; it cannot make an unavailable route appear in the menu.
fn capability_options(
    parameters: &mut [ParameterView],
    resolutions: &[orch_provider_contract::capabilities::Resolution],
) {
    for parameter in parameters {
        let setting = format!("agent.params.{}", parameter.key);
        let declared = parameter.def.get("capability").and_then(Value::as_str);
        let resolved = resolutions.iter().find(|resolved| {
            resolved.setting == setting
                && declared.is_none_or(|capability| resolved.capability == capability)
        });
        if resolved.is_none() && declared.is_none() {
            continue;
        }
        if let Some(options) = parameter
            .def
            .get_mut("resolved_options")
            .and_then(Value::as_array_mut)
        {
            options.retain(|option| {
                option
                    .get("value")
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        resolved
                            .is_some_and(|resolved| resolved.modes.iter().any(|mode| mode == value))
                    })
            });
        }
    }
}

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
        not_applicable_program: None,
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
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    parameters_in(
        &ctx,
        &mut conn,
        &principal,
        &NameBounds::default(),
        ProviderView::Native,
    )
    .await
}

/// An enclosing reader supplies its already admitted optional Provider projection.
pub(super) enum ProviderView<'a> {
    Native,
    Selected(Option<&'a crate::routes::resources::providers::PreparedProjection>),
}

/// The same native parameter reader, on the enclosing operation's admitted snapshot.
pub(super) async fn parameters_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    bounds: &NameBounds,
    provider_view: ProviderView<'_>,
) -> ApiResult<ThreadParameters> {
    let thread = ctx.id("id")?;
    let mut setting_keys = vec![keys::AGENT_MODEL, keys::AGENT_EFFORT, keys::AGENT_PARAMS];
    setting_keys.extend(
        orch_engine::capability_routes::setting_definitions(&ctx.app.builtin_sources)
            .iter()
            .map(|definition| definition.key),
    );
    setting_keys.sort_unstable();
    setting_keys.dedup();
    let paired = crate::routes::settings::readers::resolved_views(
        conn,
        principal,
        thread,
        Some(&setting_keys),
    )
    .await?;
    let found =
        crate::routes::settings::readers::settings_shown(conn, principal, paired, bounds).await?;
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
    let parts = thread_parts(conn, thread, None, orch_types::Time::new(ctx.now))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let settings = orch_engine::resolved::load(conn, thread)
        .await
        .map_err(ApiError::internal)?;
    let current = model
        .value
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut options = Vec::new();
    let mut offering_rows = Vec::new();
    if let Some(binding) = &parts.binding {
        for row in models_of_provider(conn, binding.provider.id).await? {
            offering_rows.push(row.clone());
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
            let parameters: ModelParameters =
                orch_provider_contract::models::composed_model_parameters(
                    &row.parameters,
                    &row.groups,
                    &row.option_layers,
                    None,
                )
                .map_err(ApiError::internal)?;
            options.push(ModelOption {
                token_estimate: orch_engine::token_density::of(&row.model_name),
                groups: row.groups,
                display_name: if row.display_name.is_empty() {
                    row.model_name.clone()
                } else {
                    row.display_name.clone()
                },
                model_name: row.model_name,
                option_constraints: orch_model_contract::parameters::constraint_catalogue(
                    &parameters,
                ),
                parameters,
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
        let row = orch_db::queries::threads::thread(conn, thread)
            .await?
            .ok_or_else(ApiError::not_found)?;
        review_models = ctx
            .app
            .engine
            .review_models(
                conn,
                &row,
                binding.provider.id,
                Some(principal.user()),
                &settings,
            )
            .await
            .map_err(ApiError::internal)?;
    }
    let review_binding = match (parts.binding.as_ref(), settings.model.as_deref()) {
        (Some(binding), Some(model)) => Some((
            again(&binding.credential_path)
                .ok_or_else(|| ApiError::internal("the credential path cannot be represented"))?,
            model,
        )),
        _ => None,
    };
    let approval_modes = match descriptor {
        Some(descriptor) => ctx
            .app
            .engine
            .offered_approval_modes_with_binding(
                conn,
                parts
                    .facts
                    .environment
                    .as_ref()
                    .map(|environment| environment.id),
                descriptor,
                &settings.allowed_modes,
                &review_models,
                review_binding,
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
    let mut parameters: Vec<ParameterView> = declared
        .into_iter()
        .map(|def| {
            let model = eligibility.as_ref().map(|ctx| &ctx.model);
            let projection = model.map_or_else(
                || serde_json::to_value(&def).unwrap_or(Value::Null),
                |model| def.projection(model),
            );
            let fixed = projection
                .get("provider_constraint")
                .and_then(|rule| rule.get("const"));
            ParameterView {
                value: fixed.cloned().or_else(|| values.get(&def.key).cloned()),
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
                writable: params.writable && fixed.is_none(),
                def: projection,
                locked_by: params.locked_by.clone(),
            }
        })
        .collect::<Vec<_>>();
    if eligibility.as_ref().is_some_and(|context| {
        context.credential_path == orch_agent_contract::credentials::CredentialPath::Forwarded
    }) && let Some(model) = eligibility.as_ref().map(|context| &context.model)
    {
        for (interface, schema) in orch_model_contract::parameters::thread::forms(model) {
            let key = format!(
                "{}{interface}",
                orch_model_contract::parameters::thread::PREFIX
            );
            let Some(declared) = ctx
                .app
                .builtin_sources
                .capabilities()
                .declarations()
                .interfaces
                .get(&interface)
                .filter(|declared| declared.kind == "generation")
            else {
                continue;
            };
            let title = Some(declared.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or(&interface)
                .to_owned();
            parameters.push(ParameterView {
                value: values.get(&key).cloned(),
                source: member_source(&params, &key),
                key,
                def: serde_json::json!({
                    "title":title, "group":"provider", "type":"object",
                    "provider_interface":interface, "provider_constraint":schema,
                }),
                locked_by: params.locked_by.clone(),
                writable: params.writable,
                apply: ParameterApply::NextTurn,
            });
        }
    }
    let provider = match provider_view {
        ProviderView::Selected(Some(prepared)) => Some(
            crate::routes::resources::providers::show_prepared(
                ctx,
                conn,
                principal,
                prepared.clone(),
            )
            .await?,
        ),
        ProviderView::Selected(None) => None,
        ProviderView::Native => match &parts.binding {
            Some(binding) => {
                let holding = orch_authz::resources::effective(
                    conn,
                    principal,
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
                            ctx,
                            conn,
                            principal,
                            binding.provider.id,
                        )
                        .await?,
                    )
                } else {
                    None
                }
            }
            None => None,
        },
    };
    let capabilities = if let Some(binding) = &parts.binding
        && let Some(descriptor) = descriptor
    {
        let caller_allows = principal
            .caller()
            .permits(orch_access_contract::permissions::Permission::ProviderUse);
        let allowed = caller_allows
            && ctx
                .app
                .engine
                .capability_use_preview(
                    conn,
                    &orch_engine::access::ProviderUse {
                        person: principal.user(),
                        node: thread,
                        provider: binding.provider.id,
                        model: current.as_deref(),
                        effort: settings.effort.as_deref(),
                    },
                )
                .await
                .map_err(ApiError::internal)?;
        let access = serde_json::to_value(binding.credential_path).map_err(ApiError::internal)?;
        let arrangement = serde_json::to_value(binding.arrangement).map_err(ApiError::internal)?;
        let capability_settings = {
            let mut values: std::collections::BTreeMap<String, Value> = found
                .iter()
                .filter_map(|value| {
                    value
                        .value
                        .as_ref()
                        .map(|resolved| (value.key.clone(), resolved.clone()))
                })
                .collect();
            if let Some(value) = found.iter().find(|value| value.key == keys::CALLS_RULES) {
                values.insert(
                    "calls.rules.source_scope".into(),
                    serde_json::to_value(value.source.scope).map_err(ApiError::internal)?,
                );
            }
            values
        };
        let mut capability_snapshot = orch_engine::capability_routes::project_with_sources(
            orch_engine::capability_routes::ThreadProjection {
                descriptor,
                access: access.as_str().unwrap_or_default(),
                arrangement: arrangement.as_str(),
                provider_id: binding.provider.id,
                provider: provider.as_ref(),
                models: &offering_rows,
                current: current.as_deref(),
                allowed,
                settings: capability_settings.clone(),
            },
            &ctx.app.builtin_sources,
        )
        .map_err(ApiError::internal)?;
        ctx.app
            .engine
            .join_capability_backends(
                conn,
                &mut capability_snapshot,
                &capability_settings,
                orch_engine::capability_routes::BackendRead {
                    place: thread,
                    principals: std::slice::from_ref(principal),
                    person: Some(principal.user()),
                    harness_kind: Some(&descriptor.kind),
                    provider: provider.as_ref(),
                },
            )
            .await
            .map_err(ApiError::internal)?;
        capability_snapshot.resolve()
    } else {
        Vec::new()
    };
    capability_options(&mut parameters, &capabilities);
    Ok(ThreadParameters {
        capabilities,
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
        approval_reviewer: orch_engine::approval_review::session_reviewer(conn, thread)
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
        port_tools: match descriptor {
            Some(descriptor) => {
                let row = orch_db::queries::threads::thread(conn, thread)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                Some(
                    orch_engine::tools::port_tools_for_session(
                        conn,
                        &row,
                        descriptor.capabilities.orch_tools
                            == orch_agent_contract::descriptor::OrchToolsSupport::Mcp,
                        ctx.app.clock.time(),
                    )
                    .await
                    .map_err(ApiError::internal)?,
                )
            }
            None => None,
        },
    })
}

#[cfg(any(test, feature = "test-support"))]
#[allow(
    clippy::unwrap_used,
    reason = "malformed parameter fixtures fail the test"
)]
pub mod tests {

    use super::*;

    use serde_json::json;
    #[cfg(any(test, feature = "test-support"))]
    fn capability_choices_follow_current_resolver_modes_without_rewriting_stale_choices() {
        let mut parameters: Vec<ParameterView> = serde_json::from_value(json!([{
            "key":"web_search","value":"service","apply":"new_session",
            "source":{"scope":"thread","node":null},"locked_by":null,"writable":true,
            "def":{"capability":"web_search","resolved_options":[
                {"value":"disabled"},{"value":"live"},{"value":"service"}
            ]}
        }]))
        .unwrap();
        let available = orch_provider_contract::capabilities::Resolution {
            capability: "web_search".into(),
            setting: "agent.params.web_search".into(),
            modes: vec!["disabled".into(), "live".into()],
            ..Default::default()
        };
        capability_options(&mut parameters, &[available]);
        assert_eq!(
            parameters[0].def["resolved_options"],
            json!([{"value":"disabled"},{"value":"live"}])
        );
        assert_eq!(parameters[0].value, Some(json!("service")));
        assert!(parameters[0].writable);
        capability_options(&mut parameters, &[]);
        assert_eq!(parameters[0].def["resolved_options"], json!([]));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_capability_choices_follow_current_resolver_modes_without_rewriting_stale_choices()
     {
        self::capability_choices_follow_current_resolver_modes_without_rewriting_stale_choices()
    }
}
