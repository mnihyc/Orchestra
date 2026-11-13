//! Explicit native profile lifetimes keep the admitted model ceiling before session or model work.
mod common;

use common::corpus::{Scripts, WAIT, barrier, from_native, handshake, to_native};
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_agent_contract::{
    adapter::{AgentAdapter, AgentStart, EventSink, EventStream},
    descriptor::{ApprovalMode, ArrangementType, SandboxMode},
    facts::AgentProfile,
    loopback::Loopback,
    session::{NativeModelContext, PromptCacheTtl, ProviderAccess, SessionMode, TurnStart},
};
use orch_plugin_contract::Secret;
use orch_provider_contract::models::ModelParameters;
use orch_testkit::harness_corpus::{Bindings, Frame, collect_through};
use orch_types::new_id;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const MAIN: &str = "CLAUDE_CODE_PROMPT_CACHE_TTL";
const CHILD: &str = "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL";
const ALIASES: [&str; 4] = [
    MAIN,
    "claude_code_prompt_cache_ttl",
    CHILD,
    "Claude_Code_Subagent_Prompt_Cache_Ttl",
];

fn facts(kind: &str) -> Proof<Option<Box<NativeModelContext>>> {
    if kind == "missing" {
        return Ok(None);
    }
    let enabled = matches!(kind, "enabled" | "other-model");
    let parameters: ModelParameters = serde_json::from_value(json!({
        "cache_lifetimes": if enabled { Some(vec!["1h"]) } else { None },
        "offered":{"cache_lifetimes":["1h"]}
    }))?;
    Ok(Some(Box::new(NativeModelContext::captured(
        if kind == "other-model" {
            "other-model"
        } else {
            "mock-large"
        }
        .into(),
        parameters,
        &[],
    )?)))
}
fn frames() -> Vec<Frame> {
    let mut frames = handshake()[..2].to_vec();
    frames.push(to_native(json!({"type":"control_request","request_id":{"$replay_capture":"settings"},"request":{"subtype":"get_settings"}})));
    frames.push(from_native(json!({"type":"control_response","response":{"subtype":"success","request_id":{"$replay_ref":"settings"},"response":{"effective":{}}}})));
    frames.extend(barrier());
    frames
}
async fn start(
    profile: BTreeMap<String, String>,
    native: bool,
) -> Proof<(
    ClaudeCodeAdapter,
    EventStream,
    Arc<Scripts>,
    tempfile::TempDir,
    tempfile::TempDir,
)> {
    let mut script = vec![vec![
        Frame::Raw {
            line: "2.1.322 (Claude Code)".into(),
            newline: true,
        },
        Frame::Exit { code: Some(0) },
    ]];
    if native {
        script.push(frames());
    }
    let scripts = Scripts::new(script, orch_testkit::clock::manual(), Bindings::new());
    let home = tempfile::tempdir()?;
    let work = tempfile::tempdir()?;
    let (sink, events) = EventSink::channel();
    let mut adapter =
        ClaudeCodeAdapter::with_launcher(Arc::new(scripts.clock.clone()), scripts.clone());
    adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                provider_kind: None,
                agent_id: new_id(),
                harness_kind: "claude-code".into(),
                profile: AgentProfile {
                    harness_version: None,
                    attach: None,
                    config: json!({}),
                    env: profile,
                },
                home: Some(
                    home.path()
                        .to_str()
                        .ok_or("fixture home is not UTF-8")?
                        .into(),
                ),
                provider_config: json!({}),
                loopback: Loopback::new(1, Secret::new("offline-cache-ceiling")),
                binary: Some("cache-ceiling-candidate".into()),
            },
            sink,
        )
        .await?;
    {
        let launches = scripts
            .launched
            .lock()
            .map_err(|_| "launch log is poisoned")?;
        assert_eq!(launches.len(), 1);
        for name in launches[0].env.keys() {
            assert!(!name.eq_ignore_ascii_case(MAIN) && !name.eq_ignore_ascii_case(CHILD));
        }
    }
    Ok((adapter, events, scripts, home, work))
}

async fn open(
    adapter: &mut ClaudeCodeAdapter,
    spec: orch_agent_contract::session::SessionSpec,
) -> Result<(), orch_agent_contract::adapter::AdapterError> {
    let thread = spec.settings.thread_id;
    adapter.open_session(spec).await?;
    adapter.warm_session(thread).await
}
async fn stop(adapter: &mut ClaudeCodeAdapter, scripts: &Scripts) -> Proof {
    adapter.stop().await?;
    scripts.finish().await;
    assert!(
        !scripts
            .transcripts()
            .iter()
            .flat_map(|trace| &trace.frames)
            .any(|frame| matches!(frame, Frame::ToHarness{frame,..} if frame["type"]=="user"))
    );
    Ok(())
}

#[tokio::test]
async fn explicit_profile_lifetimes_use_current_enablement_for_main_child_and_case_aliases() {
    for variable in ALIASES {
        for ttl in ["5m", "1h"] {
            for model in ["missing", "offered-only", "enabled", "other-model"] {
                let allowed = ttl == "5m" || model == "enabled";
                let profile = BTreeMap::from([
                    (variable.into(), ttl.into()),
                    ("OTHER_PROFILE_CHOICE".into(), "kept".into()),
                ]);
                let (mut adapter, mut events, scripts, _home, work) =
                    start(profile, allowed).await.unwrap();
                let thread = new_id();
                let mut spec = common::fixture_spec(
                    thread,
                    new_id(),
                    work.path().to_str().unwrap(),
                    "offline-cache-ceiling",
                    SessionMode::New,
                    None,
                );
                spec.provider = ProviderAccess::Native {
                    provider_id: new_id(),
                };
                spec.settings.arrangement = ArrangementType::Together;
                spec.settings.native_model = facts(model).unwrap();
                let opened = open(&mut adapter, spec).await;
                if allowed {
                    opened.unwrap();
                    let _ = collect_through(&mut events, "item.completed", WAIT)
                        .await
                        .unwrap();
                    scripts.completed().await;
                    let launches = scripts.launched.lock().unwrap();
                    let launch = launches
                        .iter()
                        .find(|launch| launch.label.starts_with("session-"))
                        .unwrap();
                    assert_eq!(
                        launch
                            .env
                            .get(&variable.to_ascii_uppercase())
                            .map(String::as_str),
                        Some(ttl)
                    );
                    assert_eq!(launch.env["OTHER_PROFILE_CHOICE"], "kept");
                    assert!(launch.env.keys().filter(|name|name.eq_ignore_ascii_case(&variable.to_ascii_uppercase())).all(|name|name==&variable.to_ascii_uppercase()));
                    let args = launch
                        .args
                        .windows(2)
                        .find(|args| args[0] == "--settings")
                        .unwrap();
                    let settings: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
                    assert!(
                        settings.get("promptCacheTtl").is_none()
                            && settings.get("subagentPromptCacheTtl").is_none(),
                        "the profile choice does not fabricate a thread setting"
                    );
                } else {
                    let error = opened.unwrap_err();
                    assert_eq!(error.code(), "invalid_input");
                    assert!(error.to_string().contains(if model == "offered-only" {
                        "has not enabled"
                    } else {
                        "facts for this model are unavailable"
                    }));
                    assert_eq!(
                        scripts.launched.lock().unwrap().len(),
                        1,
                        "only version discovery may run; the model session must not launch"
                    );
                }
                stop(&mut adapter, &scripts).await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn accepted_thread_lifetime_overrides_unused_profile_values_and_default_stays_omitted() {
    for accepted in [
        None,
        Some(PromptCacheTtl::FiveMinutes),
        Some(PromptCacheTtl::OneHour),
    ] {
        let profile = if accepted.is_none() {
            BTreeMap::new()
        } else {
            BTreeMap::from([
                (MAIN.into(), "unused-invalid-profile-value".into()),
                ("claude_code_prompt_cache_ttl".into(), "1h".into()),
                (CHILD.into(), "unused-invalid-profile-value".into()),
                ("claude_code_subagent_prompt_cache_ttl".into(), "1h".into()),
            ])
        };
        let (mut adapter, mut events, scripts, _home, work) = start(profile, true).await.unwrap();
        let thread = new_id();
        let mut spec = common::fixture_spec(
            thread,
            new_id(),
            work.path().to_str().unwrap(),
            "offline-cache-default",
            SessionMode::New,
            None,
        );
        spec.settings.native_model = facts(if accepted == Some(PromptCacheTtl::FiveMinutes) {
            "offered-only"
        } else {
            "enabled"
        })
        .unwrap();
        spec.settings.prompt_cache_ttl = accepted;
        open(&mut adapter, spec).await.unwrap();
        let _ = collect_through(&mut events, "item.completed", WAIT)
            .await
            .unwrap();
        scripts.completed().await;
        {
            let launches = scripts.launched.lock().unwrap();
            let launch = launches
                .iter()
                .find(|launch| launch.label.starts_with("session-"))
                .unwrap();
            let args = launch
                .args
                .windows(2)
                .find(|args| args[0] == "--settings")
                .unwrap();
            let settings: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
            for (variable, member) in [(MAIN, "promptCacheTtl"), (CHILD, "subagentPromptCacheTtl")]
            {
                assert_eq!(
                    launch.env.get(variable).map(String::as_str),
                    accepted.map(PromptCacheTtl::as_str)
                );
                assert_eq!(
                    settings.get(member),
                    accepted.map(|ttl| json!(ttl)).as_ref()
                );
                assert_eq!(
                    launch
                        .env
                        .keys()
                        .filter(|name| name.eq_ignore_ascii_case(variable))
                        .count(),
                    usize::from(accepted.is_some())
                );
            }
        }
        stop(&mut adapter, &scripts).await.unwrap();
    }
}

#[tokio::test]
async fn invalid_or_conflicting_explicit_profile_values_stop_with_no_value_disclosure() {
    for profile in [
        BTreeMap::from([(MAIN.into(), "private-invalid-cache-value".into())]),
        BTreeMap::from([
            (MAIN.into(), "5m".into()),
            ("claude_code_prompt_cache_ttl".into(), "1h".into()),
        ]),
    ] {
        let (mut adapter, _events, scripts, _home, work) = start(profile, false).await.unwrap();
        let mut spec = common::fixture_spec(
            new_id(),
            new_id(),
            work.path().to_str().unwrap(),
            "offline-cache-invalid",
            SessionMode::New,
            None,
        );
        spec.settings.native_model = facts("enabled").unwrap();
        let error = open(&mut adapter, spec).await.unwrap_err();
        assert_eq!(error.code(), "invalid_input");
        assert!(!error.to_string().contains("private-invalid-cache-value"));
        assert_eq!(scripts.launched.lock().unwrap().len(), 1);
        stop(&mut adapter, &scripts).await.unwrap();
    }
}

#[tokio::test]
async fn a_reused_session_checks_the_turns_current_model_facts_before_sending_another_input() {
    for model in ["offered-only", "missing", "other-model"] {
        let (mut adapter, mut events, scripts, _home, work) =
            start(BTreeMap::from([(MAIN.into(), "1h".into())]), true)
                .await
                .unwrap();
        let thread = new_id();
        let mut spec = common::fixture_spec(
            thread,
            new_id(),
            work.path().to_str().unwrap(),
            "offline-current-cache",
            SessionMode::New,
            None,
        );
        spec.settings.native_model = facts("enabled").unwrap();
        adapter.open_session(spec).await.unwrap();
        adapter.warm_session(thread).await.unwrap();
        let _ = collect_through(&mut events, "item.completed", WAIT)
            .await
            .unwrap();
        scripts.completed().await;
        let result = adapter
            .start_turn(TurnStart {
                native_model: facts(model).unwrap(),
                thread_id: thread,
                turn_id: new_id(),
                text: "No model work".into(),
                images: vec![],
                native_wake: None,
                plan: false,
                model: "mock-large".into(),
                effort: None,
                params: json!({}),
                approval_mode: ApprovalMode::Ask,
                sandbox_mode: SandboxMode::Open,
            })
            .await;
        assert_eq!(result.unwrap_err().code(), "invalid_input");
        assert_eq!(
            scripts.launched.lock().unwrap().len(),
            2,
            "the previous session cannot authorize the next turn's explicit one-hour choice"
        );
        stop(&mut adapter, &scripts).await.unwrap();
    }
}

#[tokio::test]
async fn every_explicit_main_and_child_choice_is_checked_without_forcing_them_to_match() {
    for enabled in [false, true] {
        let (mut adapter, mut events, scripts, _home, work) = start(
            BTreeMap::from([
                (MAIN.into(), "5m".into()),
                (CHILD.into(), "1h".into()),
                ("claude_code_subagent_prompt_cache_ttl".into(), "1h".into()),
            ]),
            enabled,
        )
        .await
        .unwrap();
        let mut spec = common::fixture_spec(
            new_id(),
            new_id(),
            work.path().to_str().unwrap(),
            "offline-paired-cache",
            SessionMode::New,
            None,
        );
        spec.settings.native_model =
            facts(if enabled { "enabled" } else { "offered-only" }).unwrap();
        let result = open(&mut adapter, spec).await;
        if enabled {
            result.unwrap();
            let _ = collect_through(&mut events, "item.completed", WAIT)
                .await
                .unwrap();
            scripts.completed().await;
            let launches = scripts.launched.lock().unwrap();
            let launch = launches
                .iter()
                .find(|launch| launch.label.starts_with("session-"))
                .unwrap();
            assert_eq!(launch.env[MAIN], "5m");
            assert_eq!(launch.env[CHILD], "1h");
            assert_eq!(
                launch
                    .env
                    .keys()
                    .filter(|name| name.eq_ignore_ascii_case(CHILD))
                    .count(),
                1
            );
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code(), "invalid_input");
            assert!(error.to_string().contains(CHILD));
            assert_eq!(scripts.launched.lock().unwrap().len(), 1);
        }
        stop(&mut adapter, &scripts).await.unwrap();
    }
}
