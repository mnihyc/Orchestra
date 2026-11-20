//! Signing in by the sign-in a person already has, and what the harness lists and reports, on the real Codex
//! in Docker. No real account is involved: the sign-ins are made up, and the containers that hold one have no
//! network, so that nothing of them reaches the vendor.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, fake_auth_json};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::facts::{CredentialYield, HarnessState, ProviderFacts};
use orch_agent_contract::login::{LoginRequest, LoginStateKind, LoginValues};
use orch_types::Json;
use serde_json::json;

fn request(method: &str, values: Json) -> LoginRequest {
    LoginRequest {
        method: method.to_owned(),
        values: serde_json::from_value::<LoginValues>(values).unwrap(),
        config: json!({}),
    }
}

fn login_states(events: &[AdapterEvent]) -> Vec<LoginStateKind> {
    events
        .iter()
        .filter_map(|event| match event {
            AdapterEvent::Login(state) => Some(state.state),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_sign_in_given_as_text_is_kept_in_the_home_and_the_harness_reports_itself_signed_in() {
    let Some(mut rig) = Rig::start_with("signin", |launcher| launcher.without_network()).await
    else {
        return;
    };
    rig.boot_subscription().await;
    assert_eq!(
        rig.adapter.provider_facts().await.unwrap(),
        ProviderFacts::default(),
        "nobody is signed in"
    );
    rig.drain();

    let auth = fake_auth_json("dana@example.com", "team", "acct-1");
    rig.adapter
        .login(request("auth_file", json!({ "auth_json": auth })))
        .await
        .unwrap();
    let events = rig.drain();
    assert_eq!(
        login_states(&events),
        [LoginStateKind::Starting, LoginStateKind::Completed]
    );
    assert!(
        events.iter().any(|event| matches!(event, AdapterEvent::Status(status) if status.status == HarnessState::Online)),
        "the harness was started again, since a running one does not see a sign-in written behind its back"
    );
    let kept = rig.home_path().join("auth.json");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), auth);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let facts = rig.adapter.provider_facts().await.unwrap();
    assert_eq!(facts.identity.as_deref(), Some("dana@example.com"));
    assert_eq!(facts.plan.as_deref(), Some("team"));
    let credential = facts.credential.expect("how the provider was signed in");
    assert_eq!(
        (credential.method.as_deref(), credential.yields),
        (Some("auth_file"), CredentialYield::Login)
    );

    // The sign-in outlives the process and the adapter.
    rig.adapter.stop().await.unwrap();
    rig.boot_subscription().await;
    let again = rig.adapter.provider_facts().await.unwrap();
    assert_eq!(again.identity.as_deref(), Some("dana@example.com"));
    assert_eq!(
        again.credential.unwrap().method.as_deref(),
        Some("auth_file")
    );

    rig.adapter.logout().await.unwrap();
    assert!(
        !kept.exists(),
        "the harness took its sign-in out of the home"
    );
    assert_eq!(
        rig.adapter.provider_facts().await.unwrap(),
        ProviderFacts::default()
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_sign_in_by_the_path_of_a_file_in_the_runtime_is_taken_too() {
    let Some(mut rig) = Rig::start_with("signin-path", |launcher| launcher.without_network()).await
    else {
        return;
    };
    rig.boot_subscription().await;
    let given = rig.workspace_path().join("given-auth.json");
    std::fs::write(&given, fake_auth_json("lee@example.com", "pro", "acct-2")).unwrap();
    rig.adapter
        .login(request(
            "auth_file",
            json!({ "auth_path": given.to_string_lossy() }),
        ))
        .await
        .unwrap();
    assert_eq!(
        rig.adapter
            .provider_facts()
            .await
            .unwrap()
            .identity
            .as_deref(),
        Some("lee@example.com")
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn what_is_not_a_sign_in_is_refused_without_touching_the_home_or_the_running_harness() {
    let Some(mut rig) = Rig::start_with("signin-bad", |launcher| launcher.without_network()).await
    else {
        return;
    };
    rig.boot_subscription().await;
    rig.drain();
    for (values, why) in [
        (
            json!({ "auth_json": "not json with a-secret-in-it" }),
            "not an auth.json",
        ),
        (json!({}), "neither"),
        (
            json!({ "auth_json": fake_auth_json("a@b", "plus", "x"), "auth_path": "/somewhere" }),
            "both",
        ),
    ] {
        let error = rig
            .adapter
            .login(request("auth_file", values))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "invalid_login_values", "{why}");
        assert!(!error.to_string().contains("a-secret"), "{error}");
    }
    assert!(!rig.home_path().join("auth.json").exists());
    let events = rig.drain();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AdapterEvent::Status(_))),
        "the harness was not started again for a sign-in that was refused"
    );
    assert_eq!(login_states(&events).last(), Some(&LoginStateKind::Failed));
    let error = rig
        .adapter
        .login(request("telepathy", json!({})))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "unknown_login_method");
    // A token the harness does not accept is refused by the harness's own command, and nothing is kept.
    let error = rig
        .adapter
        .login(request(
            "access_token",
            json!({ "access_token": "this-is-not-a-token" }),
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "login_refused", "{error}");
    assert!(!error.to_string().contains("this-is-not-a-token"));
    assert!(!rig.home_path().join("auth.json").exists());
    assert!(rig.adapter.probe().await.is_ok());
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_sign_in_of_another_workspace_than_the_providers_is_signed_out_by_the_harness_itself() {
    let Some(mut rig) =
        Rig::start_with("signin-workspace", |launcher| launcher.without_network()).await
    else {
        return;
    };
    let mut start = rig.agent_start();
    start.provider_kind = Some(orch_agent_contract::descriptor::ProviderKind::Subscription);
    start.provider_config = json!({ "workspace_id": "acct-team" });
    rig.adapter.start(start, rig.sink.clone()).await.unwrap();
    let config = std::fs::read_to_string(rig.home_path().join("config.toml")).unwrap();
    assert!(
        config.contains("forced_chatgpt_workspace_id = \"acct-team\""),
        "{config}"
    );

    let error = rig
        .adapter
        .login(request(
            "auth_file",
            json!({ "auth_json": fake_auth_json("dana@example.com", "team", "acct-personal") }),
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "login_refused", "{error}");
    assert!(rig.drain().iter().any(
        |event| matches!(event, AdapterEvent::Login(state) if state.state == LoginStateKind::Failed)
    ));
    assert_eq!(
        rig.adapter.provider_facts().await.unwrap(),
        ProviderFacts::default()
    );

    // One of the right workspace stays.
    rig.adapter
        .login(request(
            "auth_file",
            json!({ "auth_json": fake_auth_json("dana@example.com", "team", "acct-team") }),
        ))
        .await
        .unwrap();
    assert_eq!(
        rig.adapter
            .provider_facts()
            .await
            .unwrap()
            .identity
            .as_deref(),
        Some("dana@example.com")
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn the_models_the_harness_lists_are_suggestions_with_what_each_offers_and_nothing_enabled() {
    let Some(mut rig) = Rig::start("models").await else {
        return;
    };
    rig.boot_subscription().await;
    let models = rig.adapter.discover_models().await.unwrap();
    let sol = models
        .iter()
        .find(|model| model.model_name == "gpt-6-sol")
        .expect("a model of the catalogue");
    assert!(sol.listed && !sol.display_name.is_empty());
    assert!(
        sol.parameters
            .efforts()
            .iter()
            .any(|effort| effort == "high")
    );
    assert!(sol.parameters.default_effort.is_some());
    assert!(sol.parameters.takes_images());
    assert!(
        sol.parameters.service_tiers().is_empty(),
        "a speed is offered and never enabled by being listed"
    );
    let offered = sol.parameters.offered.as_ref().expect("what it offers");
    assert!(
        offered
            .service_tiers
            .as_ref()
            .unwrap()
            .iter()
            .any(|tier| tier.id == "priority")
    );
    // The harness's list carries no window; its catalogue does, and discovery reads it: the window the model
    // starts with, and the larger one an override may set as what the model offers and not as what is open.
    assert_eq!(sol.parameters.context_window, Some(272_000));
    assert_eq!(offered.context_max, Some(872_000));
    assert_eq!(sol.parameters.context_max, None);
    let same = models
        .iter()
        .find(|model| model.model_name == "gpt-5.5")
        .expect("a model whose largest window is its window");
    assert_eq!(same.parameters.context_window, Some(272_000));
    assert!(
        same.parameters
            .offered
            .as_ref()
            .is_none_or(|offered| offered.context_max.is_none()),
        "nothing larger to offer"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_real_native_session_keeps_working_when_its_access_copy_is_replaced_during_output() {
    use orch_agent_contract::descriptor::ArrangementType;
    use orch_agent_contract::events::{SessionEvent, TurnOutcome};
    use orch_agent_contract::session::{ProviderAccess, SessionMode};
    use orch_testkit::doubles::model::Verdict;
    let Some(mut rig) = Rig::start("copied-signin").await else {
        return;
    };
    let original = common::fake::copied_chatgpt_token(1);
    let replacement = common::fake::copied_chatgpt_token(2);
    rig.mock.set_credential(Some(original.credential.expose()));
    let base = rig.mock.responses_base_url();
    // Both native account traffic and model traffic stay on the disposable scripted server.
    std::fs::write(rig.home_path().join("config.toml"),format!("model_provider = \"copy_test\"\nmodel = \"mock-large\"\nchatgpt_base_url = {base:?}\n[model_providers.copy_test]\nname = \"Scripted native subscription\"\nbase_url = {base:?}\nwire_api = \"responses\"\nrequires_openai_auth = true\nsupports_websockets = false\n")).unwrap();
    rig.boot_subscription().await;
    rig.adapter.set_credential(original).await.unwrap();
    assert!(!rig.home_path().join("auth.json").exists());
    let thread = orch_types::new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.provider = ProviderAccess::Shared {
        provider_id: rig.provider_id,
        form: "codex_access_token".into(),
    };
    spec.settings.arrangement = ArrangementType::CredentialShared;
    rig.adapter.open_session(spec).await.unwrap();
    rig.mock
        .queue("say Before renewal.\nhold renewing-copy\nsay After renewal.")
        .unwrap();
    rig.adapter
        .start_turn(rig.turn(thread, "Write the scripted response."))
        .await
        .unwrap();
    rig.until(|event| {
        matches!(
            event,
            AdapterEvent::Session {
                event: SessionEvent::ItemDelta(_),
                ..
            }
        )
    })
    .await;
    rig.mock
        .set_credential(Some(replacement.credential.expose()));
    rig.adapter.set_credential(replacement).await.unwrap();
    rig.mock.release("renewing-copy");
    let ended = rig.until_turn_ends().await;
    assert!(ended.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    rig.mock.queue("say The new copy works.").unwrap();
    let next = rig
        .run_turn(thread, "Write the next scripted response.")
        .await;
    assert!(next.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let requests = rig.model_requests();
    assert!(requests.len() >= 2);
    assert!(
        requests
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted)
    );
    assert!(!rig.home_path().join("auth.json").exists());
    rig.adapter.close_session(thread).await.unwrap();
    let count = rig.model_requests().len();
    assert!(
        rig.adapter
            .start_turn(rig.turn(thread, "This closed session cannot run."))
            .await
            .is_err()
    );
    assert_eq!(rig.model_requests().len(), count);
    rig.adapter.stop().await.unwrap();
}
