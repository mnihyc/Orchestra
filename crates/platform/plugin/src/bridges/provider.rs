//! External provider facts, models, resets and key presentation through one supervised program.
//!
//! Each element is optional. The methods are `facts.read`, `models.list`, `resets.spend` and
//! `credential.present`; `describe` is read from the negotiated declaration. Facts stay a presence-
//! preserving report: an absent member, null and an empty list must not collapse into one another.
//! Resets send the ordinary source ask with `reset_id` and `idempotency_key` beside it.

use async_trait::async_trait;
use orch_plugin_contract::PluginKind;
use orch_provider_contract::limits::REPORT_MAX_BYTES;
use orch_provider_contract::source::{
    CredentialPresenter, FactsReport, FactsSource, ModelsReport, ModelsSource, PresentAsk,
    Presented, ResetReport, ResetRequest, SourceAsk, SourceDescriptor, SourceError,
    SourceErrorCode,
};
use orch_provider_contract::wire::{
    CREDENTIAL_PRESENT as PRESENT, FACTS_READ as READ_FACTS, MODELS_LIST as LIST_MODELS,
    RESETS_SPEND as SPEND_RESET,
};
use orch_types::Json;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{BridgeError, Described};
use crate::requests::CallError;
use crate::supervisor::Plugin;

/// One program supplying any declared subset of a provider's independent elements.
pub struct ProviderBridge {
    plugin: Plugin,
    descriptor: Described<SourceDescriptor>,
}

impl ProviderBridge {
    /// Takes a provider program which has negotiated its declaration.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        let descriptor = Described::new(plugin.clone(), PluginKind::Provider)?;
        Ok(Self { plugin, descriptor })
    }

    /// The latest accepted declaration, retained while the program is being restarted.
    #[must_use]
    pub fn declared(&self) -> SourceDescriptor {
        (*self.descriptor.get()).clone()
    }

    async fn ask<T: DeserializeOwned>(
        &self,
        method: &str,
        mut sent: Json,
    ) -> Result<T, SourceError> {
        let secrets = secrets_of(&sent);
        let descriptor = self.descriptor.get();
        if !self.plugin.supports(method) || !descriptor.implements(method) {
            return Err(error(
                SourceErrorCode::Unsupported,
                format!("{} does not implement {method}", self.plugin.name()),
                &secrets,
            ));
        }
        restrict_secrets(&mut sent, &descriptor);
        // Credential presentation always takes a key; other reads receive it only when declared.
        let wants_key = descriptor.wants_key();
        if !wants_key && let Some(object) = sent.as_object_mut() {
            object.remove("credential");
        }
        let reply = self
            .plugin
            .call(method, sent)
            .await
            .map_err(|failure| call_error(failure, &secrets))?;
        let bytes = serde_json::to_vec(&reply).map_err(|_| {
            error(
                SourceErrorCode::Malformed,
                "the source reply cannot be written",
                &secrets,
            )
        })?;
        if bytes.len() > REPORT_MAX_BYTES {
            return Err(error(
                SourceErrorCode::TooLarge,
                "the source reply exceeds one MiB",
                &secrets,
            ));
        }
        if !reply.is_object()
            || (method == LIST_MODELS
                && !reply
                    .get("models")
                    .is_some_and(|models| models.is_null() || models.is_array()))
        {
            return Err(error(
                SourceErrorCode::Malformed,
                format!("the reply to {method} has another shape"),
                &secrets,
            ));
        }
        // A decoder's diagnostic may include a supplied value. Name the method, never that value.
        serde_json::from_value(reply).map_err(|_| {
            error(
                SourceErrorCode::Malformed,
                format!("the reply to {method} has another shape"),
                &secrets,
            )
        })
    }
}

fn sent<T: Serialize>(ask: &T) -> Result<Json, SourceError> {
    serde_json::to_value(ask).map_err(|_| {
        error(
            SourceErrorCode::Malformed,
            "the source parameters cannot be written",
            &[],
        )
    })
}

fn secrets_of(sent: &Json) -> Vec<String> {
    let mut secrets = sent
        .get("secrets")
        .and_then(Json::as_object)
        .into_iter()
        .flat_map(|fields| fields.values())
        .filter_map(Json::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(key) = sent.pointer("/credential/key").and_then(Json::as_str) {
        secrets.push(key.to_owned());
    }
    secrets.retain(|secret| !secret.is_empty());
    secrets
        .sort_unstable_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    secrets.dedup();
    secrets
}

fn restrict_secrets(sent: &mut Json, descriptor: &SourceDescriptor) {
    if let Some(secrets) = sent.get_mut("secrets").and_then(Json::as_object_mut) {
        secrets.retain(|name, _| descriptor.secret_fields().any(|field| field.name == *name));
    }
}

fn error(code: SourceErrorCode, message: impl Into<String>, secrets: &[String]) -> SourceError {
    SourceError::new(code, message)
        .scrubbed(&secrets.iter().map(String::as_str).collect::<Vec<_>>())
}

fn call_error(failure: CallError, secrets: &[String]) -> SourceError {
    match failure {
        CallError::Failed(remote) => {
            let code = remote.code.parse().unwrap_or(SourceErrorCode::Malformed);
            error(code, remote.message, secrets)
        }
        CallError::Timeout { .. } => error(
            SourceErrorCode::Timeout,
            "the provider program did not answer in time",
            secrets,
        ),
        CallError::Malformed { .. } => error(
            SourceErrorCode::Malformed,
            "the provider program answered in another shape",
            secrets,
        ),
        CallError::Unavailable { .. } | CallError::Lost { .. } | CallError::Cancelled => error(
            SourceErrorCode::Unavailable,
            "the provider program is unavailable",
            secrets,
        ),
    }
}

#[async_trait]
impl FactsSource for ProviderBridge {
    fn describe(&self) -> SourceDescriptor {
        self.declared()
    }

    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        self.ask(READ_FACTS, sent(&ask)?).await
    }

    async fn spend_reset(
        &self,
        ask: SourceAsk,
        reset: ResetRequest,
    ) -> Result<ResetReport, SourceError> {
        let mut params = sent(&ask)?;
        let reset = sent(&reset)?;
        match (params.as_object_mut(), reset.as_object()) {
            (Some(params), Some(reset)) => params.extend(reset.clone()),
            _ => {
                return Err(error(
                    SourceErrorCode::Malformed,
                    "reset parameters must be objects",
                    &[],
                ));
            }
        }
        self.ask(SPEND_RESET, params).await
    }
}

#[async_trait]
impl ModelsSource for ProviderBridge {
    fn describe(&self) -> SourceDescriptor {
        self.declared()
    }

    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        self.ask(LIST_MODELS, sent(&ask)?).await
    }
}

#[async_trait]
impl CredentialPresenter for ProviderBridge {
    fn describe(&self) -> SourceDescriptor {
        self.declared()
    }

    async fn present(&self, ask: PresentAsk) -> Result<Presented, SourceError> {
        self.ask(PRESENT, sent(&ask)?).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_plugin_contract::{PluginRegistration, PluginState};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    /// A process-starting regression has time to run on a busy machine, but never waits indefinitely.
    const PATIENCE: Duration = Duration::from_secs(30);

    fn declaration() -> Json {
        json!({"name":"test-source","title":"A test plan","summary":"Reports a disposable plan",
            "contract":1,"facts":{"reports":["windows","balances","available_resets"],
                "credential":"key","spends_resets":true},"models":{},"presents":{},
            "fields":[{"name":"token","label":"Source token","type":"secret","required":false,
                "help":null,"choices":null,"default":null,"max_bytes":null}]})
    }

    fn ask() -> SourceAsk {
        serde_json::from_value(
            json!({"provider":{"id":"0199abcd-1234-7000-8000-0000000000a1","name":"Test plan"},
            "endpoints":[{"protocol":"openai-responses","base_url":"https://test.invalid/v1"}],
            "config":{},"secrets":{"token":"source-token-long","undeclared":"do-not-send"},
            "credential":{"key":"provider-key-long"}}),
        )
        .unwrap()
    }

    async fn program(descriptor: Json, responses: Json) -> (tempfile::TempDir, Plugin) {
        program_with(descriptor, responses, crate::Options::new("1.0.0"), &[]).await
    }

    async fn program_with(
        descriptor: Json,
        responses: Json,
        options: crate::Options,
        args: &[&str],
    ) -> (tempfile::TempDir, Plugin) {
        let dir = tempfile::tempdir().unwrap();
        let script = json!({"kind":"provider","name":"test-source","version":"1.0.0",
            "methods":["describe","facts.read","models.list","resets.spend","credential.present"],
            "descriptor":descriptor,"responses":responses});
        let file = dir.path().join("script.json");
        std::fs::write(&file, script.to_string()).unwrap();
        let mut command = vec![
            "python3".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/support/scripted_plugin.py")
                .display()
                .to_string(),
            "--script".into(),
            file.display().to_string(),
            "--log".into(),
            dir.path().join("requests.jsonl").display().to_string(),
        ];
        command.extend(args.iter().map(|arg| (*arg).to_owned()));
        let registration = PluginRegistration {
            kind: PluginKind::Provider,
            name: "test-source".into(),
            command: Some(command),
            connect: None,
            env: BTreeMap::new(),
            restart: Some(orch_plugin_contract::registration::Restart::Never),
        };
        let plugin = Plugin::start(&registration, options).await.unwrap();
        tokio::time::timeout(PATIENCE, plugin.settled())
            .await
            .unwrap();
        assert_eq!(
            plugin.report().state,
            PluginState::Running,
            "{:?}",
            plugin.report()
        );
        (dir, plugin)
    }

    fn received(dir: &tempfile::TempDir, method: &str) -> Vec<Json> {
        std::fs::read_to_string(dir.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Json>(line).unwrap())
            .filter_map(|record| record.get("received").cloned())
            .filter(|request| request["method"] == method)
            .collect()
    }

    #[tokio::test]
    async fn an_absent_window_an_explicit_null_and_an_empty_list_stay_different() {
        let (dir, plugin) = program(
            declaration(),
            json!({"facts.read":[
                {"result":{}},{"result":{"windows":null}},{"result":{"windows":[]}},
                {"result":{"balances":[{"id":"account","unit":"USD","remaining":"12.5000"}]}}
            ]}),
        )
        .await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        assert_eq!(bridge.read_facts(ask()).await.unwrap().0, json!({}));
        assert_eq!(
            bridge.read_facts(ask()).await.unwrap().0,
            json!({"windows":null})
        );
        assert_eq!(
            bridge.read_facts(ask()).await.unwrap().0,
            json!({"windows":[]})
        );
        assert_eq!(
            bridge.read_facts(ask()).await.unwrap().0["balances"][0]["remaining"],
            "12.5000"
        );
        let given = &received(&dir, READ_FACTS)[0]["params"];
        assert_eq!(given["secrets"], json!({"token":"source-token-long"}));
        assert_eq!(given["credential"]["key"], "provider-key-long");
        plugin.shutdown().await;
    }

    #[tokio::test]
    async fn unsupported_elements_are_not_called_and_a_source_which_wants_no_key_receives_none() {
        let mut descriptor = declaration();
        descriptor["facts"]["credential"] = json!("none");
        descriptor["facts"]["spends_resets"] = json!(false);
        descriptor["models"] = Json::Null;
        descriptor["presents"] = Json::Null;
        let (dir, plugin) =
            program(descriptor, json!({"facts.read":{"result":{"windows":[]}}})).await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        bridge.read_facts(ask()).await.unwrap();
        assert!(
            received(&dir, READ_FACTS)[0]["params"]
                .get("credential")
                .is_none()
        );
        assert_eq!(
            bridge.list_models(ask()).await.unwrap_err().code,
            SourceErrorCode::Unsupported
        );
        assert!(received(&dir, LIST_MODELS).is_empty());
        assert_eq!(
            bridge
                .spend_reset(
                    ask(),
                    ResetRequest {
                        reset_id: "test".into(),
                        idempotency_key: "one".into()
                    }
                )
                .await
                .unwrap_err()
                .code,
            SourceErrorCode::Unsupported
        );
        assert!(received(&dir, SPEND_RESET).is_empty());
        plugin.shutdown().await;
    }

    #[tokio::test]
    async fn a_refusal_masks_given_secrets_and_unknown_error_codes_cannot_echo_a_key() {
        let (_dir,plugin) = program(declaration(),json!({"facts.read":[
            {"error":{"code":"refused","message":"source-token-long and provider-key-long refused","retryable":false}},
            {"error":{"code":"provider-key-long","message":"provider-key-long unknown","retryable":false}}
        ]})).await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        let refused = bridge.read_facts(ask()).await.unwrap_err();
        assert_eq!(refused.code, SourceErrorCode::Refused);
        assert_eq!(refused.message, "[secret] and [secret] refused");
        let unknown = bridge.read_facts(ask()).await.unwrap_err();
        assert_eq!(unknown.code, SourceErrorCode::Malformed);
        assert!(!unknown.to_string().contains("provider-key-long"));
        plugin.shutdown().await;
    }

    #[tokio::test]
    async fn models_none_and_models_empty_are_kept_apart_and_a_shapeless_reply_is_refused() {
        let (_dir, plugin) = program(
            declaration(),
            json!({"models.list":[
                {"result":{"models":null}},{"result":{"models":[]}},{"result":{}}
            ]}),
        )
        .await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        assert!(bridge.list_models(ask()).await.unwrap().models.is_none());
        assert_eq!(
            bridge.list_models(ask()).await.unwrap().models,
            Some(Vec::new())
        );
        assert_eq!(
            bridge.list_models(ask()).await.unwrap_err().code,
            SourceErrorCode::Malformed
        );
        plugin.shutdown().await;
    }

    #[tokio::test]
    async fn a_reset_carries_the_exact_credit_and_replay_key_and_returns_the_sources_outcome() {
        let (dir, plugin) = program(
            declaration(),
            json!({"resets.spend":{"result":{
            "outcome":"The test window was reset","facts":{"windows":[]}}}}),
        )
        .await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        let result = bridge
            .spend_reset(
                ask(),
                ResetRequest {
                    reset_id: "credit-1".into(),
                    idempotency_key: "one-logical-spend".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(result.outcome, "The test window was reset");
        assert_eq!(result.facts.unwrap().0, json!({"windows":[]}));
        let request = &received(&dir, SPEND_RESET)[0]["params"];
        assert_eq!(request["reset_id"], "credit-1");
        assert_eq!(request["idempotency_key"], "one-logical-spend");
        assert_eq!(request["provider"]["name"], "Test plan");
        plugin.shutdown().await;
    }

    #[tokio::test]
    async fn a_presenter_returns_secret_wrapped_headers_with_their_expiry() {
        let (_dir,plugin) = program(declaration(),json!({"credential.present":{"result":{
            "headers":{"authorization":"Bearer test-presented"},"expires_at":"2026-10-15T12:00:00.000Z"}}})).await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        let request: PresentAsk = serde_json::from_value(json!({
            "provider":{"id":"0199abcd-1234-7000-8000-0000000000a1","name":"Test plan"},
            "endpoint":{"protocol":"openai-responses","base_url":"https://test.invalid/v1"},
            "config":{},"secrets":{"token":"source-token-long"},"credential":{"key":"provider-key-long"}})).unwrap();
        let presented = bridge.present(request).await.unwrap();
        assert_eq!(
            presented.headers["authorization"].expose(),
            "Bearer test-presented"
        );
        assert!(presented.expires_at.is_some());
        assert!(!format!("{presented:?}").contains("test-presented"));
        plugin.shutdown().await;
    }

    #[tokio::test]
    async fn an_excess_reply_is_refused_and_a_down_program_is_retryable_without_touching_a_provider()
     {
        let (_dir, plugin) = program(
            declaration(),
            json!({"facts.read":{"result":{
            "x_excess":"a".repeat(REPORT_MAX_BYTES)}}}),
        )
        .await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        assert_eq!(
            bridge.read_facts(ask()).await.unwrap_err().code,
            SourceErrorCode::TooLarge
        );
        plugin.shutdown().await;
        let unavailable = bridge.read_facts(ask()).await.unwrap_err();
        assert_eq!(unavailable.code, SourceErrorCode::Unavailable);
        assert!(unavailable.retryable);
    }

    #[tokio::test]
    async fn a_silent_facts_read_times_out_under_the_shared_read_deadline() {
        /// The clock-driven test sees a changed deadline promptly, without an eight-second sleep.
        const TICK: Duration = Duration::from_millis(5);
        let clock = orch_types::ManualClock::new("2026-10-15T10:00:00Z".parse().unwrap());
        let options = crate::Options::new("1.0.0")
            .with_clock(std::sync::Arc::new(clock.clone()))
            .with_timing(crate::Timing { tick: TICK });
        let (dir, plugin) = program_with(
            declaration(),
            json!({}),
            options,
            &["--hang-on", READ_FACTS],
        )
        .await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        let pending = tokio::spawn(async move { bridge.read_facts(ask()).await });
        tokio::time::timeout(PATIENCE, async {
            while received(&dir, READ_FACTS).is_empty() {
                tokio::time::sleep(TICK).await;
            }
        })
        .await
        .unwrap();
        clock.advance(
            chrono::Duration::from_std(orch_provider_contract::wire::READ_DEADLINE).unwrap(),
        );
        let timed_out = tokio::time::timeout(PATIENCE, pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(timed_out.code, SourceErrorCode::Timeout);
        assert!(timed_out.retryable);
        plugin.shutdown().await;
    }
}
