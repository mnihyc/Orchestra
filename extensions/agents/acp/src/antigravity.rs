//! Google owns the sign-in and the available models. Orch asks the official CLI over ACP.
use super::{Shared, launch, probe_variables, program_fault, rpc::RpcError, wire};
use orch_agent_contract::{
    adapter::AdapterError,
    descriptor::NativeProvider,
    discovery::ModelSuggestion,
    login::{LoginKind, LoginMethodDef, LoginYields},
};
use serde_json::json;

pub(super) const KIND: &str = "antigravity";
pub(super) const METHOD: &str = "oauth-personal";

pub(super) fn offered(kind: &str, hello: &wire::Initialized) -> bool {
    kind == KIND
        && hello
            .auth_methods
            .iter()
            .any(|method| method["id"] == METHOD)
}

pub(super) fn provider() -> NativeProvider {
    NativeProvider {
        harness_kind: KIND.into(),
        id: None,
        title: "Google Antigravity".into(),
        plans: Default::default(),
        login_methods: vec![LoginMethodDef {
            id: METHOD.into(),
            title: "Sign in with Google".into(),
            summary: "Sign in to the official Antigravity CLI on this provider.".into(),
            kind: LoginKind::Interactive,
            fields: vec![],
            yields: LoginYields::Login,
            lifetime_secs: None,
            requires: None,
            recommended: true,
        }],
        options_schema: json!({}),
        arrangements: vec![],
    }
}

pub(super) fn plan(shared: &Shared) -> Result<crate::program::LaunchPlan, AdapterError> {
    if !offered(&shared.env.program.kind, &shared.learned.initialized) {
        return Err(AdapterError::Unsupported("Google account sign-in".into()));
    }
    let vars = probe_variables(&shared.home_root, &shared.start, &shared.env.program, None);
    shared.env.program.plan(&vars).map_err(program_fault)
}

fn rpc_error(error: RpcError) -> AdapterError {
    match error {
        RpcError::Remote { code: -32000, .. } => AdapterError::LoginRequired,
        RpcError::Timeout { .. } => AdapterError::Timeout,
        _ => AdapterError::HarnessUnavailable(
            "Antigravity could not complete the account request.".into(),
        ),
    }
}

async fn connection(
    shared: &Shared,
) -> Result<(launch::Launched, launch::KillGuard), AdapterError> {
    let plan = plan(shared)?;
    let cwd = shared.home_root.join("probe/neutral");
    let mut agent = launch::start(&*shared.launcher, &shared.binary, &plan, cwd, None).await?;
    let guard = launch::KillGuard::over(&agent);
    let hello = agent.initialize(&shared.env.program, false, false).await?;
    if !offered(KIND, &hello.initialized) {
        return Err(AdapterError::Unsupported(
            "the installed CLI did not offer personal Google sign-in".into(),
        ));
    }
    Ok((agent, guard))
}

/// Session configuration is account-specific. Never enumerate from a static list or start a model turn.
async fn models(shared: &Shared) -> Result<Vec<ModelSuggestion>, AdapterError> {
    let (agent, _guard) = connection(shared).await?;
    let setup = agent
        .rpc
        .call(
            wire::agent::SESSION_NEW,
            json!({"cwd":shared.home_root.join("probe/neutral"),"mcpServers":[]}),
        )
        .await
        .map_err(rpc_error)?;
    let id = setup["sessionId"].as_str().ok_or_else(|| {
        AdapterError::HarnessUnavailable(
            "Antigravity did not open a model-discovery session".into(),
        )
    })?;
    let options = wire::ConfigOption::read_all(setup.get("configOptions"));
    let model = options
        .iter()
        .find(|o| o.id == "model" || o.category.as_deref() == Some("model"))
        .ok_or_else(|| {
            AdapterError::HarnessUnavailable(
                "Antigravity did not report a selectable model list.".into(),
            )
        })?;
    let mut seen = std::collections::BTreeSet::new();
    let rows: Vec<_> = model
        .choices
        .iter()
        .filter(|c| !c.value.is_empty() && seen.insert(c.value.as_str()))
        .map(|c| ModelSuggestion::named(&c.value, Some(&c.name)))
        .collect();
    if shared
        .learned
        .initialized
        .agent_capabilities
        .session_capabilities
        .close
        .is_some()
    {
        agent
            .rpc
            .call(wire::agent::SESSION_CLOSE, json!({"sessionId":id}))
            .await
            .map_err(rpc_error)?;
    }
    if rows.is_empty() {
        return Err(AdapterError::HarnessUnavailable(
            "Antigravity has no selectable model for this account.".into(),
        ));
    }
    Ok(rows)
}

pub(super) async fn logout(shared: &Shared) -> Result<(), AdapterError> {
    let (agent, _guard) = connection(shared).await?;
    agent
        .rpc
        .call("logout", json!({}))
        .await
        .map_err(rpc_error)?;
    Ok(())
}

pub(super) async fn verified_models(shared: &Shared) -> Result<Vec<ModelSuggestion>, AdapterError> {
    let models = models(shared).await;
    let verified = match &models {
        Ok(_) => Some(true),
        Err(AdapterError::LoginRequired) => Some(false),
        Err(_) => None,
    };
    if let Some(verified) = verified {
        *super::lock(&shared.google_verified) = Some(verified);
        let _ = shared
            .env
            .sink
            .emit(orch_agent_contract::adapter::AdapterEvent::Facts(
                orch_agent_contract::facts::ProviderFacts {
                    credential_verified: Some(verified),
                    ..Default::default()
                },
            ))
            .await;
    }
    models
}

pub(super) fn words(
    mut words: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
) -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
    let zh = words.entry("zh-CN".into()).or_default();
    zh.insert("native_provider.title".into(), "Google Antigravity".into());
    zh.insert(
        "native_provider.login_methods.oauth-personal.title".into(),
        "使用 Google 登录".into(),
    );
    zh.insert(
        "native_provider.login_methods.oauth-personal.summary".into(),
        "在此提供方的官方 Antigravity CLI 中登录。".into(),
    );
    words
}
