//! Concurrent forwarded model details never change the account process or another session's RPC owner.
use super::*;
use orch_agent_contract::session::NativeModelContext;
use orch_provider_contract::models::{ModelParameters, ServiceTier};
use std::sync::atomic::AtomicUsize;

fn metadata(spec: &mut orch_agent_contract::session::SessionSpec, image: bool, window: i64) {
    spec.settings.model = "openai/gpt-6-sol".into();
    spec.settings.native_model = Some(Box::new(NativeModelContext {
        model: spec.settings.model.clone(),
        parameters: ModelParameters {
            input_images: Some(image),
            context_max: Some(window),
            service_tiers: Some(vec![ServiceTier {
                id: "priority".into(),
                label: "Fast".into(),
            }]),
            ..Default::default()
        },
        capabilities: json!([
            {"capability":"generation","native_config":{"model.use_responses_lite":false}},
            {"capability":"image_input","native_config":{"features.view_image":image}}
        ]),
        extensions: Default::default(),
    }));
}

#[tokio::test]
async fn forwarded_model_processes_keep_concurrent_windows_gates_and_rpc_ownership() {
    let sequence = Arc::new(AtomicUsize::new(0));
    let mut setup = setup(FakeLauncher::new(move |method, params| {
        Ok(match method {
            "thread/start" => {
                json!({"thread":{"id":format!("model-thread-{}",sequence.fetch_add(1,Ordering::SeqCst))},"model":params["model"]})
            }
            "thread/unsubscribe" => json!({"status":"unloaded"}),
            "turn/start" => {
                json!({"turn":{"id":format!("turn-{}",params["threadId"].as_str().unwrap())}})
            }
            _ => json!({}),
        })
    }));
    let start = setup.start_request();
    setup
        .adapter
        .start(start.clone(), setup.sink.clone())
        .await
        .unwrap();
    let original_config = std::fs::read(setup.home.path().join("config.toml")).unwrap();
    let first = new_id();
    let second = new_id();
    let third = new_id();
    let mut one = plain_spec(
        setup.home.path().to_str().unwrap(),
        start.agent_id,
        start.provider_id,
        new_id(),
        first,
        SessionMode::New,
        None,
    );
    metadata(&mut one, false, 40_000);
    let mut two = one.clone();
    two.settings.thread_id = second;
    two.settings.segment_id = new_id();
    metadata(&mut two, true, 80_000);
    let first_opened = setup.adapter.open_session(one.clone()).await.unwrap();
    let second_opened = setup.adapter.open_session(two.clone()).await.unwrap();
    let mut three = one.clone();
    three.settings.thread_id = third;
    three.settings.segment_id = new_id();
    setup.adapter.open_session(three).await.unwrap();
    assert_eq!(
        setup.fake.state.launches.load(Ordering::SeqCst),
        3,
        "one control process and two reusable metadata cohorts"
    );
    assert_eq!(
        std::fs::read(setup.home.path().join("config.toml")).unwrap(),
        original_config,
        "cohort startup cannot rewrite the shared home configuration"
    );
    let requests = setup.fake.state.requests.lock().unwrap().clone();
    let mut windows = Vec::new();
    for request in requests.iter().skip(1) {
        let pin = request
            .args
            .iter()
            .find(|arg| arg.starts_with("model_catalog_json="))
            .unwrap();
        let table: toml::Table = toml::from_str(pin).unwrap();
        let path = std::path::PathBuf::from(table["model_catalog_json"].as_str().unwrap());
        assert!(path.starts_with(setup.home.path()));
        let catalogue: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let model = &catalogue["models"][0];
        assert_eq!(model["slug"], "openai/gpt-6-sol");
        assert_eq!(
            model["use_responses_lite"], false,
            "a namespaced suffix supplies no native dialect declaration"
        );
        assert_eq!(model["service_tiers"][0]["id"], "priority");
        assert!(
            model["model_messages"]["instructions_template"]
                .as_str()
                .unwrap()
                .contains("coding agent")
        );
        windows.push(model["context_window"].as_i64().unwrap());
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("secret-token")
        );
    }
    windows.sort_unstable();
    assert_eq!(windows, [40_000, 80_000]);
    setup
        .until(|event| matches!(event,AdapterEvent::Status(status) if status.sessions_open==3))
        .await;
    let turn = |thread_id| TurnStart {
        thread_id,
        turn_id: new_id(),
        model: one.settings.model.clone(),
        text: "Read this source".into(),
        images: vec![],
        effort: None,
        params: json!({"service_tier":"priority"}),
        plan: false,
        native_model: one.settings.native_model.clone(),
        native_wake: None,
        approval_mode: ApprovalMode::Bypass,
        sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Open,
    };
    let first_turn = turn(first);
    let mut second_turn = turn(second);
    second_turn.native_model = two.settings.native_model.clone();
    setup.adapter.start_turn(first_turn).await.unwrap();
    setup.adapter.start_turn(second_turn).await.unwrap();
    let mut changed = one.clone();
    changed.settings.thread_id = second;
    changed.settings.mode = SessionMode::Resume;
    changed.settings.native_session_ref = Some(second_opened.native_session_ref.clone());
    assert!(
        matches!(
            setup.adapter.open_session(changed).await,
            Err(AdapterError::Other {
                retryable: true,
                ..
            })
        ),
        "pending native work cannot be moved to another metadata process"
    );
    let received = setup.fake.state.received.lock().unwrap().clone();
    let starts: Vec<_> = received
        .iter()
        .filter(|request| request["method"] == "thread/start")
        .collect();
    assert_eq!(starts[0]["params"]["config"]["features.view_image"], false);
    assert_eq!(starts[1]["params"]["config"]["features.view_image"], true);
    let turns: Vec<_> = received
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect();
    assert_eq!(
        turns[0]["params"]["threadId"],
        first_opened.native_session_ref
    );
    assert_eq!(
        turns[1]["params"]["threadId"],
        second_opened.native_session_ref
    );
    assert!(
        received
            .iter()
            .all(|request| request["method"] != "thread/resume")
    );
    setup.adapter.stop().await.unwrap();
}
