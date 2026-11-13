//! What the adapter does with a thread when the program that made it and the program that runs now are not the
//! same build: listing and adopting a thread a person began under another build, and continuing an Orch thread
//! under a program that updated itself in between. Codex keeps its threads in its own store and reads them
//! itself; the adapter asks it through the app-server and checks no version of the thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]
mod common;
use common::fake::FakeLauncher;
use orch_adapter_codex::CodexAdapter;
use orch_agent_contract::adapter::{AgentAdapter, AgentStart, EventSink, EventStream};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::SessionMode;
use orch_plugin_contract::Secret;
use orch_types::new_id;
use serde_json::json;
use std::sync::Arc;

/// The build the adapter is pinned to, one before it, and one after it.
const PINNED: &str = "0.167.2";
const OLDER: &str = "0.160.0";
const NEWER: &str = "0.167.3";

/// An app-server that holds three threads, each made by another build, and answers for them as the real one does.
fn store() -> FakeLauncher {
    let made_by = |native: &str| match native {
        "thr_older" => OLDER,
        "thr_newer" => NEWER,
        _ => PINNED,
    };
    FakeLauncher::new(move |method, params| match method {
        "thread/list" => Ok(json!({"data":[
            {"id":"thr_older","cliVersion":OLDER,"preview":"made long ago","cwd":"/work/web","updatedAt":1_700_000_000},
            {"id":"thr_pinned","cliVersion":PINNED,"preview":"made under the pin","cwd":"/work/web","updatedAt":1_700_000_100},
            {"id":"thr_newer","cliVersion":NEWER,"preview":"made lately","cwd":"/work/web","updatedAt":1_700_000_200},
        ],"nextCursor":null})),
        "thread/read" => {
            let native = params["threadId"].as_str().unwrap();
            Ok(
                json!({"thread":{"id":native,"cliVersion":made_by(native),"model":"gpt-5","reasoningEffort":"high"}}),
            )
        }
        "thread/turns/list" => Ok(json!({"data":[
            {"id":"t1","status":"completed","startedAt":1_700_000_000,"completedAt":1_700_000_001,"error":null,"items":[],"itemsView":"notLoaded"},
            {"id":"t2","status":"completed","startedAt":1_700_000_002,"completedAt":1_700_000_003,"error":null,"items":[],"itemsView":"notLoaded"},
        ],"nextCursor":null})),
        "thread/resume" => {
            let native = params["threadId"].as_str().unwrap();
            Ok(json!({"thread":{"id":native,"cliVersion":made_by(native),"path":null}}))
        }
        _ => Ok(json!({})),
    })
}

struct Host {
    adapter: CodexAdapter,
    fake: Arc<FakeLauncher>,
    _home: tempfile::TempDir,
    _events: EventStream,
}

/// An adapter started on a program of the build `running`.
async fn host(running: &str) -> Host {
    let fake = Arc::new(store());
    fake.state.set_version(running);
    let mut adapter = CodexAdapter::with_launcher(
        Arc::clone(&fake) as Arc<dyn orch_adapter_codex::process::Launcher>
    );
    let home = tempfile::tempdir().unwrap();
    let (sink, events) = EventSink::channel();
    adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                provider_kind: None,
                agent_id: new_id(),
                harness_kind: "codex".into(),
                profile: AgentProfile::default(),
                home: Some(home.path().to_string_lossy().into_owned()),
                provider_config: json!({}),
                loopback: Loopback::new(41999, Secret::new("test-token")),
                binary: None,
            },
            sink,
        )
        .await
        .unwrap();
    Host {
        adapter,
        fake,
        _home: home,
        _events: events,
    }
}

// (d, a) Threads begun under a build older than the pin, and (d, b) under a newer one.

#[tokio::test]
async fn threads_made_by_older_and_newer_builds_are_all_listed_for_adoption() {
    let mut host = host(PINNED).await;
    let found = host.adapter.discover_sessions().await.unwrap();
    let mut refs: Vec<_> = found
        .iter()
        .map(|session| session.native_session_ref.as_str())
        .collect();
    refs.sort_unstable();
    assert_eq!(refs, ["thr_newer", "thr_older", "thr_pinned"]);
}

#[tokio::test]
async fn a_thread_made_by_any_build_is_read_for_its_adoption_by_the_program_that_runs_now() {
    let mut host = host(PINNED).await;
    for native in ["thr_older", "thr_pinned", "thr_newer"] {
        let record = host
            .adapter
            .read_session(native, false)
            .await
            .unwrap_or_else(|error| panic!("{native}: {error}"));
        assert_eq!(record.turns, 2, "{native}");
        assert_eq!(record.model.as_deref(), Some("gpt-5"));
    }
}

// (d, c) An Orch thread begun under the pin, continued by a program that updated itself.

#[tokio::test]
async fn a_thread_begun_under_the_pin_is_resumed_by_a_newer_program() {
    let mut host = host(NEWER).await;
    for (native, mode) in [
        ("thr_pinned", SessionMode::Resume),
        ("thr_older", SessionMode::Resume),
        ("thr_newer", SessionMode::Adopt),
    ] {
        let spec = common::plain_spec(
            "/work/web",
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            mode,
            Some(native),
        );
        let opened = host
            .adapter
            .open_session(spec)
            .await
            .unwrap_or_else(|error| panic!("{native}: {error}"));
        assert!(opened.resumed, "{native}");
        assert_eq!(opened.native_session_ref, native);
    }
    assert_eq!(host.fake.state.calls("thread/resume").len(), 3);
}
