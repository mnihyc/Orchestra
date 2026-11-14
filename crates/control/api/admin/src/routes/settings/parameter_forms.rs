//! Default forms reuse native parameter projection without making a hypothetical thread or reading a credential.
use crate::middleware::requests::ApiResult;
use crate::registry::Ctx;
use crate::routes::nodes::invalid;
use orch_agent_contract::credentials::CredentialPath;
use orch_agent_contract::descriptor::{Placement, ProviderKind};
use orch_agent_contract::parameters::{Eligibility, ParameterDef, ParameterScope};
use orch_api_contract::resources::ProviderKind as ApiProviderKind;
use orch_api_contract::settings::SettingParameterFormBody;
use orch_types::Json;
use serde_json::json;

/// Projects the supplied declarations using the same rules as an actual thread's parameters.
pub(super) async fn form(
    _ctx: Ctx,
    (): (),
    body: SettingParameterFormBody,
) -> ApiResult<Vec<Json>> {
    let scope: Option<ParameterScope> = body
        .scope
        .map(|scope| {
            serde_json::from_value(json!(scope)).map_err(|_| {
                invalid(
                    "scope",
                    "a parameter scope is an organization, project, topic or thread",
                )
            })
        })
        .transpose()?;
    let provider_kind = match body.provider_kind {
        ApiProviderKind::Api => ProviderKind::Api,
        ApiProviderKind::Subscription => ProviderKind::Subscription,
    };
    // Defaults may feed any future credential/runtime combination. The fixed enum product
    // covers those possibilities; concrete thread admission still evaluates its actual binding.
    let mut contexts = Vec::new();
    for credential_path in [
        CredentialPath::Native,
        CredentialPath::Forwarded,
        CredentialPath::Shared,
        CredentialPath::Handover,
    ] {
        for environment_placement in [Placement::Managed, Placement::Connected] {
            contexts.push(Eligibility {
                credential_path,
                provider_kind,
                environment_placement,
                model: body.model.clone(),
                harness_version: body.harness_version.clone(),
            });
        }
    }
    let features =
        orch_agent_contract::parameters::feature_values(&body.feature_defaults, &body.features);
    let mut forms = Vec::new();
    for (index, value) in body.definitions.into_iter().enumerate() {
        let def: ParameterDef = serde_json::from_value(value).map_err(|_| {
            invalid(
                &format!("definitions.{index}"),
                "the parameter declaration cannot be read",
            )
        })?;
        if scope
            .as_ref()
            .is_some_and(|scope| !def.scopes.contains(scope))
            || !contexts
                .iter()
                .any(|context| def.is_shown(context, &features))
        {
            continue;
        }
        forms.push(def.projection(&body.model));
    }
    Ok(forms)
}
