//! The windows of the models a harness lists: its list carries none, so discovery reads them from the catalogue the harness renders on request,
//! against a harness played in the test (the real one is asked in `provider.rs`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::fake::FakeLauncher;
use orch_adapter_codex::CodexAdapter;
use orch_adapter_codex::process::{
    HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit,
};
use orch_agent_contract::adapter::{AgentAdapter, AgentStart, EventSink, EventStream};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_plugin_contract::Secret;
use orch_types::{Json, new_id};
use serde_json::json;

/// A command that has already ended with `code`.
struct Ended(i32);

#[async_trait]
impl ProcessControl for Ended {
    async fn wait(&mut self) -> ProcessExit {
        ProcessExit {
            code: Some(self.0),
            detail: String::new(),
        }
    }

    fn kill(&mut self) {}
}

/// The harness of the fake, which renders `printed` and ends with `code` when it is asked for its catalogue.
struct WithCatalogue {
    harness: FakeLauncher,
    printed: String,
    code: i32,
    rendered: Mutex<Vec<LaunchRequest>>,
}

#[async_trait]
impl Launcher for WithCatalogue {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        if request.args == ["debug", "models"] {
            self.rendered.lock().unwrap().push(request);
            return Ok(HarnessProcess {
                stdin: Box::new(tokio::io::sink()),
                stdout: Box::new(std::io::Cursor::new(self.printed.clone().into_bytes())),
                stderr: Some(Box::new(std::io::Cursor::new(b"a warning".to_vec()))),
                control: Box::new(Ended(self.code)),
            });
        }
        self.harness.launch(request).await
    }
}

/// The catalogue as the harness renders it, with the members that matter here.
fn catalogue() -> String {
    json!({ "models": [
        { "slug": "gpt-6-sol", "context_window": 272_000, "max_context_window": 872_000, "base_instructions": "..." },
        { "slug": "gpt-5.5", "context_window": 272_000, "max_context_window": 272_000 }
    ] })
    .to_string()
}

/// A harness whose list of models is `listed`.
fn harness(listed: Json) -> FakeLauncher {
    FakeLauncher::new(move |method, _| match method {
        "model/list" => Ok(json!({ "data": listed.clone(), "nextCursor": null })),
        _ => Ok(json!({})),
    })
}

struct Booted {
    adapter: CodexAdapter,
    launcher: Arc<WithCatalogue>,
    _events: EventStream,
    _sink: EventSink,
    _home: tempfile::TempDir,
}

async fn boot(listed: Json, printed: String, code: i32) -> Booted {
    let launcher = Arc::new(WithCatalogue {
        harness: harness(listed),
        printed,
        code,
        rendered: Mutex::new(Vec::new()),
    });
    let mut adapter = CodexAdapter::with_launcher(Arc::clone(&launcher) as Arc<dyn Launcher>);
    let (sink, events) = EventSink::channel();
    let home = tempfile::tempdir().unwrap();
    adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                agent_id: new_id(),
                harness_kind: "codex".into(),
                profile: AgentProfile::default(),
                home: Some(home.path().to_string_lossy().into_owned()),
                provider_config: json!({}),
                loopback: Loopback::new(41999, Secret::new("secret-token")),
                binary: None,
            },
            sink.clone(),
        )
        .await
        .unwrap();
    Booted {
        adapter,
        launcher,
        _events: events,
        _sink: sink,
        _home: home,
    }
}

fn named<'a>(found: &'a [ModelSuggestion], name: &str) -> &'a ModelSuggestion {
    found.iter().find(|model| model.model_name == name).unwrap()
}

fn listed() -> Json {
    json!([
        { "id": "gpt-6-sol", "model": "gpt-6-sol", "displayName": "GPT-6 Sol", "hidden": false },
        { "id": "gpt-5.5", "model": "gpt-5.5", "displayName": "GPT-5.5", "hidden": false },
        { "id": "unlisted", "model": "unlisted", "displayName": "Unlisted", "hidden": false }
    ])
}

#[tokio::test]
async fn a_list_without_windows_is_completed_from_the_catalogue_the_harness_renders() {
    let mut booted = boot(listed(), catalogue(), 0).await;
    let found = booted.adapter.discover_models().await.unwrap();
    assert_eq!(found.len(), 3);
    let sol = named(&found, "gpt-6-sol");
    assert_eq!(sol.parameters.context_window, Some(272_000));
    assert_eq!(
        sol.parameters
            .offered
            .as_ref()
            .and_then(|offered| offered.context_max),
        Some(872_000),
        "the largest an override may set is on offer, and nothing is open"
    );
    assert_eq!(sol.parameters.context_max, None);
    let same = named(&found, "gpt-5.5");
    assert_eq!(same.parameters.context_window, Some(272_000));
    assert_eq!(same.parameters.offered, None, "its largest is its window");
    assert_eq!(
        named(&found, "unlisted").parameters.context_window,
        None,
        "the catalogue says nothing of it"
    );

    // The command is the harness's own, run beside it with what it was started with.
    let rendered = booted.launcher.rendered.lock().unwrap().clone();
    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].program, "codex");
    assert!(rendered[0].env.contains_key("CODEX_HOME"));
    assert!(rendered[0].env.contains_key("ORCH_LLM_TOKEN"));
}

#[tokio::test]
async fn a_catalogue_that_cannot_be_rendered_leaves_the_list_as_it_was() {
    for (printed, code) in [
        (catalogue(), 1),
        ("this is not a catalogue".to_owned(), 0),
        (String::new(), 0),
        (json!({ "models": 3 }).to_string(), 0),
    ] {
        let mut booted = boot(listed(), printed.clone(), code).await;
        let found = booted.adapter.discover_models().await.unwrap();
        assert_eq!(found.len(), 3, "{printed:?} {code}");
        assert!(
            found
                .iter()
                .all(|model| model.parameters.context_window.is_none()
                    && model.parameters.offered.is_none()),
            "{printed:?} {code}"
        );
    }
}

#[tokio::test]
async fn a_list_that_carries_the_windows_is_not_corrected_and_the_catalogue_is_not_asked_for() {
    let carried = json!([
        { "id": "gpt-6-sol", "model": "gpt-6-sol", "displayName": "GPT-6 Sol", "hidden": false,
          "contextWindow": 400_000, "maxContextWindow": 1_000_000 }
    ]);
    let mut booted = boot(carried, catalogue(), 0).await;
    let found = booted.adapter.discover_models().await.unwrap();
    let sol = named(&found, "gpt-6-sol");
    assert_eq!(sol.parameters.context_window, Some(400_000));
    assert_eq!(
        sol.parameters
            .offered
            .as_ref()
            .and_then(|offered| offered.context_max),
        Some(1_000_000)
    );
    assert!(booted.launcher.rendered.lock().unwrap().is_empty());
}
