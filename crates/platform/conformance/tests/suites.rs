//! Built-in implementations and controlled faults are distinguished by the contract rule an operator reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use http::{Method, Response, StatusCode};
use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::events::TokenUsage;
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored};
use orch_conformance::report::{Outcome, Report};
use orch_conformance::{agent, blob, driver, executor, identity, notifier, protocol, translator};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::RuntimeSpec;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::descriptor::{ExecutorAvailability, ExecutorDescriptor};
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_executor_orch::server::{ExecConfig, OrchExec};
use orch_identity_contract::claims::VerifiedIdentity;
use orch_identity_contract::provider::{
    CompleteRequest, Discovered, IdentityProviderKind, PendingSignIn, ProviderError,
    ProviderSettings, SignInStart, StartRequest,
};
use orch_model_contract::protocol::WireProtocol;
use orch_model_contract::translator::{
    ProtocolTranslator, ResponseTranslator, TranslateError, TranslatedRequest,
    TranslatorDescriptor, UpstreamSide,
};
use orch_model_contract::usage::{ObservedUsage, UsageObserver};
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery};
use orch_notifier_contract::descriptor::NotifierDescriptor;
use orch_notifier_contract::notifier::Notifier;
use orch_testkit::doubles::driver::{MemoryDriver, Operation};
use orch_types::{Hash, new_id};
use serde_json::{Value, json};

fn report() -> Report {
    Report::new("controlled conformance check")
}

fn failed(report: &Report, rule: &str) {
    assert!(
        matches!(report.outcome(rule), Some(Outcome::Failed(_))),
        "{report}"
    );
    assert!(report.to_string().contains(rule), "{report}");
}

fn runtime() -> RuntimeSpec {
    serde_json::from_value(json!({
        "runtime_id": new_id(), "kind": "environment", "name": "base", "driver": "memory", "spec": {},
        "resources": {}, "roles": ["environment"], "gateway_url": "wss://127.0.0.1:41000",
        "enroll_token": "test", "ca_cert": "", "tools": [], "mounts": [], "env": {}, "labels": {},
        "volumes": {"workspace":{"name":"workspace","storage":null}, "home":{"name":"home","storage":null},
            "state":{"name":"state","storage":null}}
    })).unwrap()
}

#[tokio::test]
async fn a_driver_passes_and_a_refused_create_fails_the_named_create_rule() {
    let good = MemoryDriver::new("memory");
    let mut result = report();
    driver::run(&good, &runtime(), "conformance-test", &mut result).await;
    assert!(result.passed(), "{result}");
    let broken = MemoryDriver::new("memory");
    broken.fail_next(
        Operation::Create,
        DriverError::Internal("deliberate wrong implementation".into()),
    );
    let mut result = report();
    driver::run(&broken, &runtime(), "conformance-test", &mut result).await;
    failed(&result, "driver.create");
}

struct WrongRange(orch_blob_local::LocalBlobStore);

#[async_trait]
impl BlobStore for WrongRange {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        content: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.0.put(hash, size, content).await
    }
    async fn get(&self, hash: &Hash, _: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        self.0.get(hash, None).await
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.0.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.0.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.0.delete(hash).await
    }
}

#[tokio::test]
async fn the_local_store_passes_and_ignoring_a_range_fails_what_the_reader_sees() {
    let dir = tempfile::tempdir().unwrap();
    let store = orch_blob_local::LocalBlobStore::open(dir.path())
        .await
        .unwrap();
    let mut result = report();
    blob::run(&store, &mut result).await;
    assert!(result.passed(), "{result}");
    let mut result = report();
    blob::run(&WrongRange(store), &mut result).await;
    failed(&result, "blob.read");
    failed(&result, "blob.range");
}

fn environment(dir: &std::path::Path) -> EnvironmentContext {
    serde_json::from_value(json!({
        "environment_id": new_id(), "generation": 1, "data_dir": dir, "workspace_root": dir,
        "home": null, "tools_dir": null, "tools": [], "mounts": [], "user": "agent",
        "shell": {"name":"sh","path":"/bin/sh"}, "os":"linux", "arch":"x86_64", "path_style":"posix",
        "limits": {}, "variables": {}, "mcp_servers_enabled": false
    })).unwrap()
}

fn orch_exec() -> OrchExec {
    OrchExec::with_config(ExecConfig {
        ports: Some(41000..=41999),
        ..ExecConfig::default()
    })
}

#[tokio::test]
async fn unverified_executor_platforms_are_not_offered_and_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut environment = environment(dir.path());
    for os in [
        orch_agent_contract::facts::Os::Macos,
        orch_agent_contract::facts::Os::Windows,
    ] {
        environment.os = os;
        let mut orch = orch_exec();
        let mut codex = orch_executor_codex::server::CodexExecServer::new();
        for implementation in [
            &mut orch as &mut dyn Executor,
            &mut codex as &mut dyn Executor,
        ] {
            let available = implementation.available(&environment).await;
            assert!(!available.available);
            assert!(
                available
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("declared for Linux"))
            );
            assert!(implementation.start(&environment).await.is_err());
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "a refused platform starts nothing and creates no state"
        );
    }
}

struct WrongExecutor(OrchExec);

#[async_trait]
impl Executor for WrongExecutor {
    fn kind(&self) -> &'static str {
        self.0.kind()
    }
    fn describe(&self) -> ExecutorDescriptor {
        self.0.describe()
    }
    async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
        self.0.available(env).await
    }
    async fn start(&mut self, env: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError> {
        let mut endpoint = self.0.start(env).await?;
        endpoint.tool_sets.push("not-declared".into());
        Ok(endpoint)
    }
    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.0.stop().await
    }
}

#[tokio::test]
async fn two_orch_executors_pass_and_an_unusable_offer_names_its_start_rule() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let fixture = executor::Fixture {
        first: environment(a.path()),
        second: environment(b.path()),
    };
    let mut result = report();
    executor::run(&mut orch_exec(), &mut orch_exec(), &fixture, &mut result).await;
    assert!(result.passed(), "{result}");
    let mut result = report();
    executor::run(
        &mut WrongExecutor(orch_exec()),
        &mut orch_exec(),
        &fixture,
        &mut result,
    )
    .await;
    failed(&result, "executor.start_pair");
}

fn agent_fixture(home: &std::path::Path, work: &std::path::Path, faults: Value) -> agent::Fixture {
    let provider = new_id();
    let profile = new_id();
    let start = serde_json::from_value(json!({
        "provider_id":provider,"agent_id":profile,"harness_kind":"fake",
        "profile":{"harness_version":null,"config":{"faults":faults},"env":{}},
        "home":home,"provider_config":null,"binary":null,
        "loopback":{"base_url":"http://127.0.0.1:41000","token":"conformance"}
    }))
    .unwrap();
    let mut sessions = Vec::new();
    let mut turns = Vec::new();
    for index in 0..2 {
        let thread = new_id();
        sessions.push(serde_json::from_value(json!({
            "agent_id":profile,"thread_id":thread,"segment_id":new_id(),"mode":"new",
            "native_session_ref":null,"upto_turn_ordinal":null,"workspace_root":work,"instruction_root_marker":false,
            "directory":work,"cwd":work,"mounts":[],"model":"fake-large","effort":null,"params":{},
            "approval_mode":"on_request","sandbox_mode":"workspace","instructions":"Complete the controlled turn.",
            "memory":null,"arrangement":"together","orch_tools":[],"raw_events":false,
            "session_key":format!("{index:032x}"),"archive":null,"history":null,
            "provider":{"path":"native","provider_id":provider},
            "exec":{"executor":null,"tool_set":null,"environment_id":new_id(),"generation":1},"mcp":[]
        })).unwrap());
        turns.push(serde_json::from_value(json!({
            "thread_id":thread,"turn_id":new_id(),"text":format!("controlled turn {index}"),"images":[],
            "model":"fake-large","effort":null,"params":{},"approval_mode":"on_request","sandbox_mode":"workspace"
        })).unwrap());
    }
    agent::Fixture {
        start,
        sessions,
        turns,
    }
}

#[tokio::test]
async fn the_scripted_adapter_passes_and_a_turn_it_cannot_start_fails_the_session_rule() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut adapter = orch_adapter_fake::FakeAdapter::new(Arc::new(orch_testkit::clock::manual()));
    let mut result = report();
    agent::run(
        &mut adapter,
        Some(&agent_fixture(home.path(), work.path(), json!({}))),
        &mut result,
    )
    .await;
    assert!(result.passed(), "{result}");
    let home = tempfile::tempdir().unwrap();
    let mut adapter = orch_adapter_fake::FakeAdapter::new(Arc::new(orch_testkit::clock::manual()));
    let mut result = report();
    let fixture = agent_fixture(
        home.path(),
        work.path(),
        json!({"start_turn":{"code":"harness_error","message":"deliberate","times":1}}),
    );
    agent::run(&mut adapter, Some(&fixture), &mut result).await;
    failed(&result, "agent.sessions");
}

fn protocol_fixture() -> protocol::Fixture<'static> {
    protocol::Fixture {
        method: Method::POST, path: "/responses", body: br#"{"model":"test-model","input":"hello"}"#, model:"test-model",
        response: b"event: response.completed\ndata: {\"response\":{\"id\":\"resp_test\",\"model\":\"test-model\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n",
        usage: ObservedUsage { model:Some("test-model".into()), request_ref:Some("resp_test".into()), cost:None,
            tokens: TokenUsage { input_tokens:10, output_tokens:2, total_tokens:12, ..TokenUsage::default() } }
    }
}

struct OpenRelay;
impl WireProtocol for OpenRelay {
    fn name(&self) -> &'static str {
        "openai-responses"
    }
    fn allowed(&self, _: &Method, _: &str) -> bool {
        true
    }
    fn model_of(&self, path: &str, body: &[u8]) -> Option<String> {
        orch_protocol_openai_responses::OpenAiResponses.model_of(path, body)
    }
    fn deny(&self, status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
        orch_protocol_openai_responses::OpenAiResponses.deny(status, code, message)
    }
    fn usage_observer(&self) -> Box<dyn UsageObserver> {
        orch_protocol_openai_responses::OpenAiResponses.usage_observer()
    }
    fn models_request(&self) -> Option<(Method, &'static str)> {
        None
    }
    fn parse_models(&self, _: &[u8]) -> Vec<orch_agent_contract::discovery::ModelSuggestion> {
        Vec::new()
    }
}

#[test]
fn both_builtin_protocols_pass_and_an_open_relay_fails_the_allowlist_rule() {
    let mut result = report();
    protocol::run(
        &orch_protocol_openai_responses::OpenAiResponses,
        &protocol_fixture(),
        &mut result,
    );
    assert!(result.passed(), "{result}");
    let fixture = protocol::Fixture {
        method:Method::POST, path:"/v1/messages", body:br#"{"model":"test-model","messages":[]}"#, model:"test-model",
        response:b"event: message_start\ndata: {\"message\":{\"id\":\"msg_test\",\"model\":\"test-model\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n",
        usage:ObservedUsage { request_ref:Some("msg_test".into()), ..protocol_fixture().usage }
    };
    let mut result = report();
    protocol::run(
        &orch_protocol_anthropic_messages::AnthropicMessages,
        &fixture,
        &mut result,
    );
    assert!(result.passed(), "{result}");
    let mut result = report();
    protocol::run(&OpenRelay, &protocol_fixture(), &mut result);
    failed(&result, "protocol.allowed");
}

struct Channel {
    ignores_private: bool,
}
#[async_trait]
impl Notifier for Channel {
    fn describe(&self) -> NotifierDescriptor {
        serde_json::from_value(json!({"name":"controlled","title":"Controlled channel","summary":"A disposable far side.",
            "targets":["destination"],"fields":[],"owners":[],"single":false,"reaches":"channel","body":"text",
            "templates":{"*":{"single":{"subject":null,"body":"{{title}}"},
                "digest":{"subject":null,"body":"{{#notifications}}{{title}}\n{{/notifications}}"}}}})).unwrap()
    }
    async fn check(&self, _: &Config) -> Result<Value, DeliverError> {
        Ok(json!({"checked":true}))
    }
    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        match delivery.body.as_str() {
            "retry" => Err(DeliverError::after("busy", "try later", 3)),
            "gone" => Err(DeliverError::gone("gone", "target removed")),
            "private" if !self.ignores_private => {
                Err(DeliverError::refused("private_host", "not allowed"))
            }
            _ => Ok(()),
        }
    }
}

fn delivery(body: &str, digest: bool) -> Delivery {
    let notification = json!({"id":new_id(),"kind":"turn.completed","title":"Controlled notification",
        "url":"https://orch.example/","node":null,"thread_id":null,"chat_id":null,"recipient":null,
        "data":{},"body":"The controlled turn completed.","at":"2026-10-14T10:00:00.000Z"});
    let notifications = if digest {
        vec![notification.clone(), notification]
    } else {
        vec![notification]
    };
    serde_json::from_value(json!({"id":new_id(),"attempt":1,"config":{"values":{},"secrets":{},"allowed_private_hosts":[]},
        "to":{},"recipient":null,"language":"en","subject":null,"body":body,"notifications":notifications})).unwrap()
}

#[tokio::test]
async fn a_channel_preserves_delivery_decisions_and_private_access_cannot_pass_silently() {
    let fixture = notifier::Fixture {
        config: Config::default(),
        deliveries: vec![delivery("single", false), delivery("digest", true)],
        retry: Some(delivery("retry", false)),
        gone: Some(delivery("gone", false)),
        private: Some(delivery("private", false)),
    };
    let mut result = report();
    notifier::run(
        &Channel {
            ignores_private: false,
        },
        Some(&fixture),
        &mut result,
    )
    .await;
    assert!(result.passed(), "{result}");
    let mut result = report();
    notifier::run(
        &Channel {
            ignores_private: true,
        },
        Some(&fixture),
        &mut result,
    )
    .await;
    failed(&result, "notifier.private");
}

struct Convert {
    drops_fragments: bool,
}
impl ProtocolTranslator for Convert {
    fn from(&self) -> &'static str {
        "source"
    }
    fn to(&self) -> &'static str {
        "target"
    }
    fn describe(&self) -> TranslatorDescriptor {
        serde_json::from_value(
            json!({"from":"source","to":"target","title":"Controlled conversion",
            "losses":{"preserved":["Text"],"approximated":[],"dropped":[]}}),
        )
        .unwrap()
    }
    fn translate_request(
        &self,
        path: &str,
        body: Bytes,
        _: &UpstreamSide,
    ) -> Result<TranslatedRequest, TranslateError> {
        Ok(TranslatedRequest {
            method: "POST".into(),
            path: path.into(),
            query: BTreeMap::new(),
            headers: BTreeMap::new(),
            body,
            authorized: true,
        })
    }
    fn response_translator(&self) -> Box<dyn ResponseTranslator> {
        Box::new(Convert {
            drops_fragments: self.drops_fragments,
        })
    }
}
impl ResponseTranslator for Convert {
    fn translate_chunk(&mut self, chunk: Bytes) -> Result<Bytes, TranslateError> {
        Ok(if self.drops_fragments && chunk.len() == 1 {
            Bytes::new()
        } else {
            chunk
        })
    }
    fn finish(&mut self) -> Result<Bytes, TranslateError> {
        Ok(Bytes::new())
    }
}

#[test]
fn a_translator_that_loses_fragmented_bytes_fails_the_stream_rule() {
    let fixture = translator::Fixture {
        path: "/convert".into(),
        body: "{}".into(),
        upstream: UpstreamSide {
            base_url: "http://127.0.0.1:41000".into(),
            options: json!({}),
            credential: None,
        },
        request: TranslatedRequest {
            method: "POST".into(),
            path: "/convert".into(),
            query: BTreeMap::new(),
            headers: BTreeMap::new(),
            body: Bytes::from_static(b"{}"),
            authorized: true,
        },
        response: "answer with Unicode: café".into(),
        translated_response: "answer with Unicode: café".into(),
        implemented: vec![],
    };
    let mut result = report();
    translator::run(
        &Convert {
            drops_fragments: false,
        },
        Some(&fixture),
        &mut result,
    );
    assert!(result.passed(), "{result}");
    let mut result = report();
    translator::run(
        &Convert {
            drops_fragments: true,
        },
        Some(&fixture),
        &mut result,
    );
    failed(&result, "translator.response");
}

struct Issuer {
    accepts_wrong_nonce: bool,
}
fn identity() -> VerifiedIdentity {
    serde_json::from_value(json!({"subject":"person-1","email":"person@example.test","email_verified":true,"name":"Person","username":"person"})).unwrap()
}
#[async_trait]
impl IdentityProviderKind for Issuer {
    fn kind(&self) -> &'static str {
        "controlled"
    }
    async fn start(&self, _: &StartRequest) -> Result<SignInStart, ProviderError> {
        let state = new_id().to_string();
        Ok(SignInStart {
            redirect_url: format!("https://issuer.example/authorize?state={state}"),
            pending: PendingSignIn {
                state,
                nonce: new_id().to_string(),
                pkce_verifier: new_id().to_string(),
            },
        })
    }
    async fn complete(&self, request: &CompleteRequest) -> Result<VerifiedIdentity, ProviderError> {
        if request.pending.nonce != "valid-nonce" && !self.accepts_wrong_nonce {
            Err(ProviderError::new("wrong nonce"))
        } else {
            Ok(identity())
        }
    }
    async fn validate(&self, settings: &ProviderSettings) -> Result<Discovered, ProviderError> {
        Ok(Discovered {
            issuer: settings.issuer.clone(),
            signing_keys: 1,
            metadata: json!({}),
        })
    }
}

#[tokio::test]
async fn an_identity_for_another_signin_is_refused_by_its_named_nonce_rule() {
    let settings: ProviderSettings =
        serde_json::from_value(json!({"key":"test","name":"Test issuer",
        "issuer":"https://issuer.example","client_id":"orch"}))
        .unwrap();
    let complete = CompleteRequest {
        settings: settings.clone(),
        client_secret: None,
        callback_url: "https://orch.example/callback".into(),
        pending: PendingSignIn {
            state: "state".into(),
            nonce: "valid-nonce".into(),
            pkce_verifier: "verifier".into(),
        },
        code: "good-code".into(),
        now: "2026-10-08T17:04:05.123Z".parse().unwrap(),
    };
    let mut wrong_nonce = complete.clone();
    wrong_nonce.pending.nonce = "wrong-nonce".into();
    wrong_nonce.code = "independent-code".into();
    let fixture = identity::Fixture {
        start: StartRequest {
            settings,
            callback_url: complete.callback_url.clone(),
            fresh: false,
        },
        complete,
        wrong_nonce,
        identity: identity(),
    };
    let mut result = report();
    identity::run(
        &Issuer {
            accepts_wrong_nonce: false,
        },
        Some(&fixture),
        &mut result,
    )
    .await;
    assert!(result.passed(), "{result}");
    let mut result = report();
    identity::run(
        &Issuer {
            accepts_wrong_nonce: true,
        },
        Some(&fixture),
        &mut result,
    )
    .await;
    failed(&result, "identity.nonce");
}
