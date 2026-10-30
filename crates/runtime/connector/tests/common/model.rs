//! The model side of a test: the scripted model server with a key of its own, the connector's protocols, the commands of the provider role as the system sends them, a harness that calls the listener, and a search for a key on disk.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::adapter::{
    AdapterError, AgentAdapter, AgentStart, AgentStarted, EventSink,
};
use orch_agent_contract::credentials::{GivenCredential, IssuedCredential};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::facts::ProviderFacts;
use orch_agent_contract::login::{LoginRequest, LoginValues};
use orch_agent_contract::loopback::Loopback as HarnessLoopback;
use orch_agent_contract::session::{
    ProviderAccess, SessionOpened, SessionSpec, ThreadId, TurnStart,
};
use orch_connector_core::forwarder::{ProtocolEntry, Protocols};
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_connector_core::loopback::Tickets;
use orch_connector_core::roles::provider::{ProviderParts, plug_in};
use orch_link::commands::Progress;
use orch_link_contract::commands::{CommandBody, SerialKey};
use orch_link_contract::session::Role;
use orch_pki::sealing::{RecipientKey, info_for_command, seal};
use orch_protocol_anthropic_messages::AnthropicMessages;
use orch_protocol_openai_responses::{OpenAiChat, OpenAiEmbeddings, OpenAiResponses};
use orch_testkit::doubles::controller::Applied;
use orch_testkit::doubles::model::{
    ClientReply, ClientRequest, MockConfig, MockLlm, ModelClient, RecordedRequest,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;

use super::{PATIENCE, Rig};

/// The name of the Responses format.
pub const RESPONSES: &str = "openai-responses";
/// The name of the Chat Completions format.
pub const CHAT: &str = "openai-chat";
/// The name of the Messages format.
pub const MESSAGES: &str = "anthropic-messages";
/// The model the scripted server answers to unless it is told another.
pub const MODEL: &str = "mock-large";

/// The protocols a connector forwards, as the program composes them.
pub fn protocols() -> Protocols {
    Protocols::new()
        .with(ProtocolEntry::new(OpenAiResponses, "/v1", |names| {
            OpenAiResponses.models_response(names)
        }))
        .with(ProtocolEntry::new(OpenAiChat, "/v1", |names| {
            OpenAiChat.models_response(names)
        }))
        .with(ProtocolEntry::new(OpenAiEmbeddings, "/v1", |names| {
            OpenAiEmbeddings.models_response(names)
        }))
        .with(ProtocolEntry::new(AnthropicMessages, "", |names| {
            AnthropicMessages.models_response(names)
        }))
}

/// A key that belongs to nobody, different in every test.
pub fn fresh_key() -> String {
    format!("sk-orch-test-{}", new_id().simple())
}

/// Every file under `root` whose bytes hold `needle`.
pub fn files_holding(root: &Path, needle: &str) -> Vec<PathBuf> {
    fn walk(directory: &Path, needle: &[u8], found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                walk(&path, needle, found);
            } else if kind.is_file()
                && std::fs::read(&path)
                    .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, needle.as_bytes(), &mut found);
    found
}

/// What a harness got back from the model.
#[derive(Debug, Clone)]
pub struct ModelCall {
    /// The status of the answer.
    pub status: u16,
    /// The text the model said, where it said one.
    pub text: String,
    /// The unchanged reply bytes as UTF-8, including SSE frames when streamed.
    pub body: String,
}

/// The scripted adapter, made to call the model the way a harness does: with the token of its process, at
/// the address of its session, in the protocol its session was opened in. The turn it then runs says what the
/// model said. It is given, and records, everything the connector gives an adapter, so that a test can look for
/// a key in it.
pub struct CallingFake {
    inner: FakeAdapter,
    loopback: Option<HarnessLoopback>,
    sessions: std::collections::HashMap<ThreadId, (String, String)>,
    calls: Arc<Mutex<Vec<ModelCall>>>,
    given: Arc<Mutex<Vec<String>>>,
    chat_stream: bool,
}

impl CallingFake {
    fn note(&self, what: &impl serde::Serialize) {
        self.given
            .lock()
            .unwrap()
            .push(serde_json::to_string(what).unwrap());
    }
}

fn text_of(body: &Json) -> String {
    if let Some(choices) = body["choices"].as_array() {
        return choices
            .iter()
            .filter_map(|choice| choice.pointer("/message/content").and_then(Json::as_str))
            .collect();
    }
    if let Some(content) = body["content"].as_array() {
        return content
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
    }
    body["output"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}

#[async_trait]
impl AgentAdapter for CallingFake {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }

    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        self.note(&start);
        self.loopback = Some(start.loopback.clone());
        self.inner.start(start, sink).await
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.inner.stop().await
    }

    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        self.inner.probe().await
    }

    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        self.note(&spec);
        if let ProviderAccess::Forwarded { protocol, .. } = &spec.provider {
            self.sessions.insert(
                spec.settings.thread_id,
                (spec.session_key.clone(), protocol.clone()),
            );
        }
        self.inner.open_session(spec).await
    }

    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        self.inner.close_session(thread).await
    }

    async fn start_turn(&mut self, mut turn: TurnStart) -> Result<(), AdapterError> {
        if let (Some(loopback), Some((key, protocol))) =
            (&self.loopback, self.sessions.get(&turn.thread_id))
        {
            let client = ModelClient::new(&loopback.base_url);
            let (path, body, messages) = if protocol == RESPONSES {
                (
                    format!("/llm/{key}/v1/responses"),
                    json!({ "model": turn.model, "input": turn.text }),
                    false,
                )
            } else if protocol == CHAT {
                (
                    format!("/llm/{key}/v1/chat/completions"),
                    json!({
                        "model": turn.model, "messages":[{"role":"user","content":turn.text}],
                        "stream":self.chat_stream,"stream_options":{"include_usage":true},
                        "tools":[{"type":"function","function":{"name":"local_tool","parameters":{"type":"object","properties":{}}}}],
                        "vendor_option":{"keep":"unchanged"},
                    }),
                    false,
                )
            } else {
                (
                    format!("/llm/{key}/v1/messages"),
                    json!({ "model": turn.model, "max_tokens": 64,
                            "messages": [ { "role": "user", "content": turn.text } ] }),
                    true,
                )
            };
            let mut request =
                ClientRequest::post_json(&path, &body).bearer(loopback.token.expose());
            if messages {
                request = request.messages_version();
            }
            let reply = client.send(request).await.expect("the listener answers");
            let text = if protocol == CHAT && self.chat_stream {
                reply
                    .events()
                    .iter()
                    .filter_map(|event| serde_json::from_str::<Json>(&event.data).ok())
                    .filter_map(|chunk| {
                        chunk
                            .pointer("/choices/0/delta/content")
                            .and_then(Json::as_str)
                            .map(str::to_owned)
                    })
                    .collect()
            } else {
                text_of(&reply.json())
            };
            self.calls.lock().unwrap().push(ModelCall {
                status: reply.status.as_u16(),
                text: text.clone(),
                body: reply.text(),
            });
            turn.text = if reply.status.is_success() {
                format!("#!fake\nsay {}", text.replace('\n', "\\n"))
            } else {
                format!(
                    "#!fake\nfail model_refused {}",
                    reply.text().replace('\n', " ")
                )
            };
        }
        self.note(&turn);
        self.inner.start_turn(turn).await
    }

    async fn interrupt(
        &mut self,
        thread: ThreadId,
        turn: orch_agent_contract::session::TurnId,
    ) -> Result<(), AdapterError> {
        self.inner.interrupt(thread, turn).await
    }

    async fn login(&mut self, request: LoginRequest) -> Result<(), AdapterError> {
        self.inner.login(request).await
    }

    async fn login_input(&mut self, values: LoginValues) -> Result<(), AdapterError> {
        self.inner.login_input(values).await
    }

    async fn cancel_login(&mut self) -> Result<(), AdapterError> {
        self.inner.cancel_login().await
    }

    async fn logout(&mut self) -> Result<(), AdapterError> {
        self.inner.logout().await
    }

    async fn provider_facts(&mut self) -> Result<ProviderFacts, AdapterError> {
        self.inner.provider_facts().await
    }

    async fn discover_models(&mut self) -> Result<Vec<ModelSuggestion>, AdapterError> {
        self.inner.discover_models().await
    }

    async fn issue_credential(&mut self, form: &str) -> Result<IssuedCredential, AdapterError> {
        self.inner.issue_credential(form).await
    }

    async fn set_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        self.inner.set_credential(credential).await
    }

    async fn return_credential(&mut self) -> Result<GivenCredential, AdapterError> {
        self.inner.return_credential().await
    }

    async fn accept_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        self.inner.accept_credential(credential).await
    }
}

/// Makes the adapters a connector hosts from its data directory and the working directory of the test.
pub type MakeAdapters = Box<dyn FnOnce(&Path, &Path) -> AdapterRegistry>;

/// How a [`ProviderRig`] is made, beyond its configuration.
#[derive(Default)]
pub struct Options {
    /// The adapters the connector hosts; the scripted adapter that calls the model where none are given.
    pub adapters: Option<MakeAdapters>,
    /// Whether the system made the machine.
    pub managed: bool,
    /// The tools directory of a managed runtime.
    pub tools_dir: Option<PathBuf>,
    /// How long the test waits for what has to happen, where it is not the usual.
    pub patience: Option<std::time::Duration>,
    /// Whether the scripted adapter is registered as one that needs the harness's program.
    pub fake_needs_binary: bool,
    /// The Chat Completions test adapter requests a streamed reply rather than one JSON answer.
    pub chat_stream: bool,
}

/// A connector that plays the agent and the provider roles, hosting the scripted adapter that calls the model,
/// on a system in memory, with a scripted model server that wants a key of its own.
pub struct ProviderRig {
    /// The system and its connector.
    pub rig: Rig,
    /// The scripted model server.
    pub mock: MockLlm,
    /// The key the model server wants.
    pub key: String,
    /// What the scripted adapter got from the model.
    pub calls: Arc<Mutex<Vec<ModelCall>>>,
    /// Everything the connector gave the scripted adapter, written as JSON.
    pub given: Arc<Mutex<Vec<String>>>,
    /// The thread of the tests that need one.
    pub thread: Id,
    /// Its segment.
    pub segment: Id,
    /// The directory the sessions of the test work in.
    pub work: tempfile::TempDir,
}

impl ProviderRig {
    /// A rig whose connector is started. The model server listens on a port the system chose.
    pub async fn new() -> Self {
        Self::with(|_, _| ()).await
    }

    /// A rig whose configuration (and the model server's) the test shapes before the connector starts.
    pub async fn with(shape: impl FnOnce(&mut Rig, &MockLlm)) -> Self {
        Self::build(Options::default(), shape).await
    }

    /// A rig made as `options` say.
    pub async fn build(options: Options, shape: impl FnOnce(&mut Rig, &MockLlm)) -> Self {
        let key = fresh_key();
        let mock = MockLlm::start(MockConfig::default().with_credential(&key))
            .await
            .unwrap();
        let mut rig = Rig::with_roles(vec![Role::Agent, Role::Provider]);
        rig.config.provider.upstreams = vec![mock.responses_base_url(), mock.messages_base_url()];
        let calls = Arc::new(Mutex::new(Vec::new()));
        let given = Arc::new(Mutex::new(Vec::new()));
        let work = tempfile::tempdir().unwrap();
        rig.managed = options.managed;
        rig.tools_dir = options.tools_dir;
        rig.patience = options.patience.unwrap_or(rig.patience);
        rig.adapters = match options.adapters {
            Some(adapters) => adapters(rig.data.path(), work.path()),
            None => {
                let mut adapters = AdapterRegistry::new();
                let (shared_calls, shared_given) = (Arc::clone(&calls), Arc::clone(&given));
                let chat_stream = options.chat_stream;
                adapters
                    .register(AdapterRegistration {
                        kind: "fake",
                        adapter_version: 1,
                        builtin: true,
                        needs_binary: options.fake_needs_binary,
                        home_rule: HomeRule::Connector,
                        factory: Arc::new(move |clock| {
                            Box::new(CallingFake {
                                inner: FakeAdapter::new(clock),
                                loopback: None,
                                sessions: std::collections::HashMap::new(),
                                calls: Arc::clone(&shared_calls),
                                given: Arc::clone(&shared_given),
                                chat_stream,
                            })
                        }),
                    })
                    .unwrap();
                adapters
            }
        };
        rig.plug = Some(Arc::new(|builder, roles, data, key, clock| {
            let parts = ProviderParts::new(protocols(), data, key, clock).unwrap();
            plug_in(builder, roles, parts)
        }));
        shape(&mut rig, &mock);
        rig.start().await;
        Self {
            rig,
            mock,
            key,
            calls,
            given,
            thread: new_id(),
            segment: new_id(),
            work,
        }
    }

    /// `agent.start` for the scripted adapter of the rig's provider and agent profile.
    pub async fn start_harness(&self) {
        self.rig.agent_start(json!({})).await;
    }

    /// `session.open` of the rig's thread on the forwarded path, in `protocol`, for `model`.
    pub async fn open_forwarded_session(&self, protocol: &str, model: &str) -> Json {
        self.open_forwarded_session_in(protocol, model, "on_request", "workspace")
            .await
    }

    /// The same, with the approval mode and the sandbox mode of the session.
    pub async fn open_forwarded_session_in(
        &self,
        protocol: &str,
        model: &str,
        approval_mode: &str,
        sandbox_mode: &str,
    ) -> Json {
        let mut body = self.rig.open_body(
            self.thread,
            self.segment,
            &self.work.path().display().to_string(),
        );
        body["model"] = json!(model);
        body["approval_mode"] = json!(approval_mode);
        body["sandbox_mode"] = json!(sandbox_mode);
        body["arrangement"] = json!("key_separated");
        body["provider"] = json!({
            "path": "forwarded", "provider_id": self.rig.provider, "protocol": protocol,
            "ticket": { "$ticket": { "kind": "llm" } }
        });
        self.rig.open_session(body).await
    }

    /// `turn.start` of the rig's thread with `text` for `model`; returns the turn.
    pub async fn start_turn_for(&self, text: &str, model: &str) -> Id {
        self.start_turn_in(text, model, "on_request", "workspace")
            .await
    }

    /// The same, with the approval mode and the sandbox mode of the turn.
    pub async fn start_turn_in(
        &self,
        text: &str,
        model: &str,
        approval_mode: &str,
        sandbox_mode: &str,
    ) -> Id {
        let turn = new_id();
        let body: orch_link_contract::commands::TurnStart = serde_json::from_value(json!({
            "thread_id": self.thread, "turn_id": turn, "text": text, "images": [],
            "model": model, "effort": null, "params": {},
            "approval_mode": approval_mode, "sandbox_mode": sandbox_mode
        }))
        .unwrap();
        self.rig
            .succeed(&SerialKey::Thread(self.thread), &body)
            .await;
        turn
    }

    /// The serial key of the provider's commands.
    pub fn provider_key(&self) -> SerialKey {
        SerialKey::Provider(self.rig.provider)
    }

    /// The key of the provider, sealed to the connector's machine for the command `command`.
    pub fn sealed_key_for(&self, command: Id, key: &str) -> Json {
        let recipient = RecipientKey::from_certificate(&self.rig.identity.certificate);
        let sealed = seal(&recipient, &info_for_command(command), key.as_bytes()).unwrap();
        json!({ "$sealed": sealed })
    }

    /// Sends `provider.start` for the rig's provider with these endpoints (protocol, base URL, options) and the
    /// key the model server wants; returns how the command ended.
    pub async fn start_provider(&self, upstreams: &[(&str, String, Json)]) -> Progress {
        self.start_provider_with_key(upstreams, Some(&self.key.clone()))
            .await
    }

    /// Sends `provider.start` with the key given, or none.
    pub async fn start_provider_with_key(
        &self,
        upstreams: &[(&str, String, Json)],
        key: Option<&str>,
    ) -> Progress {
        let id = new_id();
        let body = json!({
            "provider_id": self.rig.provider,
            "protocols": upstreams.iter().map(|(protocol, _, _)| *protocol).collect::<Vec<_>>(),
            "upstreams": upstreams.iter().map(|(protocol, url, options)| json!({
                "protocol": protocol, "base_url": url, "options": options })).collect::<Vec<_>>(),
            "credential": key.map(|key| self.sealed_key_for(id, key)),
            "secrets": {}
        });
        let body: orch_link_contract::commands::ProviderStart =
            serde_json::from_value(body).unwrap();
        self.command_with_id(id, &self.provider_key(), &body).await
    }

    /// Sends a command under an identifier chosen beforehand and waits until it ended.
    pub async fn command_with_id<B: CommandBody>(
        &self,
        id: Id,
        key: &SerialKey,
        body: &B,
    ) -> Progress {
        self.rig
            .controller
            .issue_with_id(id, key, self.rig.deadline(), body);
        tokio::time::timeout(PATIENCE, self.rig.controller.wait_command(id))
            .await
            .expect("the command never ended")
    }

    /// `provider.start` that has to succeed, for the Responses endpoint of the scripted server.
    pub async fn start_responses_provider(&self) {
        let progress = self
            .start_provider(&[(RESPONSES, self.mock.responses_base_url(), json!({}))])
            .await;
        assert!(
            matches!(progress, Progress::Succeeded(_)),
            "provider.start: {progress:?}"
        );
    }

    /// `provider.start` that has to succeed, for both endpoints of the scripted server.
    pub async fn start_both_providers(&self) {
        let progress = self
            .start_provider(&[
                (RESPONSES, self.mock.responses_base_url(), json!({})),
                (MESSAGES, self.mock.messages_base_url(), json!({})),
            ])
            .await;
        assert!(
            matches!(progress, Progress::Succeeded(_)),
            "provider.start: {progress:?}"
        );
    }

    /// `lease.open` in mode `forwarded`; returns the lease and how the command ended.
    pub async fn try_open_lease(
        &self,
        thread: Id,
        models: &[&str],
        protocol: Option<&str>,
        allowance: Option<Json>,
    ) -> (Id, Progress) {
        let lease = new_id();
        let mut body = json!({
            "lease_id": lease, "provider_id": self.rig.provider, "thread_id": thread,
            "mode": "forwarded", "allowed_models": models
        });
        if let Some(protocol) = protocol {
            body["protocol"] = json!(protocol);
        }
        if let Some(allowance) = allowance {
            body["allowance"] = allowance;
        }
        let body: orch_link_contract::commands::LeaseOpen = serde_json::from_value(body).unwrap();
        let progress = self.rig.send(&self.provider_key(), &body).await;
        (lease, progress)
    }

    /// `lease.open` that has to succeed.
    pub async fn open_lease(
        &self,
        thread: Id,
        models: &[&str],
        protocol: Option<&str>,
        allowance: Option<Json>,
    ) -> Id {
        let (lease, progress) = self
            .try_open_lease(thread, models, protocol, allowance)
            .await;
        assert!(
            matches!(progress, Progress::Succeeded(_)),
            "lease.open: {progress:?}"
        );
        lease
    }

    /// `lease.close`; returns the answer.
    pub async fn close_lease(&self, lease: Id) -> Json {
        let body = orch_link_contract::commands::LeaseClose { lease_id: lease };
        self.rig.succeed(&self.provider_key(), &body).await
    }

    /// A harness of the connector's listener: a token for its process and the key of a session of `thread`.
    pub fn harness(&self, thread: Id) -> Harness {
        self.harness_with(thread, Tickets::default())
    }

    /// The same, with tickets the session was given.
    pub fn harness_with(&self, thread: Id, tickets: Tickets) -> Harness {
        let loopback = self.rig.connector.as_ref().unwrap().loopback();
        let grant = loopback.grant();
        let binding = loopback.bind(
            grant.id,
            thread,
            self.segment,
            self.rig.provider,
            self.rig.agent,
            tickets,
        );
        Harness {
            client: ModelClient::new(&format!("http://127.0.0.1:{}", loopback.port())),
            token: grant.token.expose().to_owned(),
            session_key: binding.session_key.clone(),
        }
    }

    /// The stream of the provider, as the system applied it.
    pub fn provider_events(&self) -> Vec<Applied> {
        self.rig.controller.applied(
            &orch_link_contract::streams::Stream::Provider {
                provider_id: self.rig.provider,
            }
            .to_string(),
        )
    }

    /// The events of one kind on the stream of the provider.
    pub fn provider_events_of(&self, kind: &str) -> Vec<Applied> {
        self.provider_events()
            .into_iter()
            .filter(|event| event.kind == kind)
            .collect()
    }

    /// Waits until `count` events of `kind` were applied on the stream of the provider.
    pub async fn wait_provider_event(&self, kind: &str, count: usize) -> Vec<Applied> {
        let stream = orch_link_contract::streams::Stream::Provider {
            provider_id: self.rig.provider,
        }
        .to_string();
        let wanted = kind.to_owned();
        let arrived = self
            .rig
            .controller
            .wait_until(PATIENCE, move |controller| {
                controller
                    .applied(&stream)
                    .iter()
                    .filter(|event| event.kind == wanted)
                    .count()
                    >= count
            })
            .await;
        assert!(
            arrived,
            "no {count}th {kind} arrived on the provider's stream: {:?}",
            self.provider_events()
                .iter()
                .map(|event| event.kind.clone())
                .collect::<Vec<_>>()
        );
        self.provider_events_of(kind)
    }

    /// What the scripted model server was sent that is a model call.
    pub fn model_requests(&self) -> Vec<RecordedRequest> {
        self.mock
            .requests()
            .into_iter()
            .filter(|request| !request.path.starts_with("/_mock"))
            .collect()
    }
}

/// A harness as the listener sees it.
pub struct Harness {
    client: ModelClient,
    /// The token of its process.
    pub token: String,
    /// The key of its session.
    pub session_key: String,
}

impl Harness {
    /// A request to the model address of the session, with the token of the process.
    pub fn request(&self, method: &str, rest: &str, body: Option<&Json>) -> ClientRequest {
        let path = format!("/llm/{}{rest}", self.session_key);
        let request = match body {
            Some(body) => ClientRequest::post_json(&path, body),
            None => ClientRequest::get(&path),
        };
        let mut request = request.bearer(&self.token);
        request.method = method.parse().unwrap();
        request
    }

    /// `POST` a JSON document to `rest` below the session.
    pub async fn post(&self, rest: &str, body: &Json) -> ClientReply {
        self.client
            .send(self.request("POST", rest, Some(body)))
            .await
            .unwrap()
    }

    /// `GET` `rest` below the session.
    pub async fn get(&self, rest: &str) -> ClientReply {
        self.client
            .send(self.request("GET", rest, None))
            .await
            .unwrap()
    }

    /// The client, for a request a test makes up.
    pub fn client(&self) -> &ModelClient {
        &self.client
    }
}

/// A request for a response of the Responses format, as Codex makes it.
pub fn responses_request(model: &str, text: &str) -> Json {
    json!({ "model": model, "input": text })
}

/// A request for a message of the Messages format.
pub fn messages_request(model: &str, text: &str) -> Json {
    json!({ "model": model, "max_tokens": 64, "messages": [ { "role": "user", "content": text } ] })
}

/// The log of the process, captured at its finest level, so that a test can look in it for a key.
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl LogBuffer {
    /// Everything logged so far.
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Writes into the buffer.
pub struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> LogWriter {
        LogWriter(Arc::clone(&self.0))
    }
}

/// Captures the log of the whole process, once (the callsites of a process are decided once, so the one
/// subscriber is installed before any test logs), and returns what it holds.
pub fn capture_log() -> LogBuffer {
    static LOG: std::sync::OnceLock<LogBuffer> = std::sync::OnceLock::new();
    LOG.get_or_init(|| {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
        buffer
    })
    .clone()
}

/// A second machine on the same system, which plays the agent role alone: its harnesses call a provider that
/// another machine holds.
pub struct AgentMachine {
    /// Its connector.
    pub connector: orch_connector_core::connector::Connector,
    /// The system's end of its link.
    pub controller: orch_testkit::doubles::controller::ScriptedController,
    /// Its identity in the system.
    pub machine_id: Id,
    /// Its data directory.
    pub data: tempfile::TempDir,
}

impl ProviderRig {
    /// Adds a machine that plays the agent role alone to the system of the rig, and starts its connector.
    pub async fn add_agent_machine(&self) -> AgentMachine {
        use orch_connector_core::connector::{ConnectorBuilder, ConnectorParams};
        use orch_link_contract::session::{Arch, Os, Platform};
        use orch_pki::enrollment::MachineIdentity;
        use orch_testkit::doubles::controller::ScriptedController;
        use orch_testkit::doubles::link::{Hub, ShapedBlobs};

        let rig = &self.rig;
        let machine_id = new_id();
        let issued = rig.pki.machine(machine_id);
        let roles = vec![Role::Agent];
        let controller =
            ScriptedController::attach(&rig.hub, Arc::clone(&rig.clock), machine_id, roles.clone());
        let data = tempfile::tempdir().unwrap();
        let params = ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: data.path().to_owned(),
            config: orch_config::ConnectorConfig::default(),
            identity: MachineIdentity {
                machine_id,
                key: issued.key.duplicate().unwrap(),
                certificate: issued.certificate.clone(),
                authority: rig.pki.certificate(),
            },
            endpoints: vec![Hub::endpoint()],
            roles: roles.clone(),
            managed: true,
            dialer: rig.hub.dialer(Some(issued.certificate.clone())),
            clock: Arc::clone(&rig.clock),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            adapters: AdapterRegistry::new(),
            tools_dir: None,
            blobs: Some(Arc::new(ShapedBlobs::new(
                Arc::clone(&rig.blobs),
                None,
                i64::MAX,
            ))),
        };
        let parts = ProviderParts::new(
            protocols(),
            data.path(),
            &issued.key,
            Arc::clone(&rig.clock),
        )
        .unwrap();
        let builder = plug_in(ConnectorBuilder::new(params), &roles, parts);
        let connector = builder.start().await.unwrap();
        tokio::time::timeout(PATIENCE, connector.link().connected())
            .await
            .expect("the second connector never got a session");
        AgentMachine {
            connector,
            controller,
            machine_id,
            data,
        }
    }
}
