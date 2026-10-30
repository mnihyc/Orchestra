//! Signing in, what the harness says of the sign-in, the models it lists and the credentials that pass between homes, against the real harness.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;

use common::{Rig, outcome};
use orch_agent_contract::adapter::{AdapterError, AgentAdapter};
use orch_agent_contract::credentials::{CredentialNeed, GivenCredential};
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::facts::CredentialYield;
use orch_agent_contract::login::{LoginRequest, LoginStateKind, LoginValues, ShowKind};
use orch_agent_contract::session::{ProviderAccess, SessionMode};
use orch_plugin_contract::Secret;
use orch_testkit::doubles::model::Presentation;
use orch_types::new_id;
use serde_json::json;

/// A token as the vendor's command prints it, for a test.
const PASTED: &str = "sk-ant-oat01-pasted-for-a-test-0123456789";

fn token_request(token: &str) -> LoginRequest {
    LoginRequest {
        method: "setup_token".into(),
        values: LoginValues(BTreeMap::from([("token".to_owned(), json!(token))])),
        config: json!({}),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_home_nobody_signed_in_to_says_that_a_login_is_required() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    assert!(matches!(
        rig.adapter.provider_facts().await,
        Err(AdapterError::LoginRequired)
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pasted_token_is_kept_privately_and_the_facts_say_how_the_harness_is_signed_in() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.adapter.login(token_request(PASTED)).await.unwrap();
    let states = rig
        .logins_until(|state| state.state == LoginStateKind::Completed, 20)
        .await;
    assert_eq!(states[0].state, LoginStateKind::Starting);

    let file = rig.home.path().join(".orch/oauth.json");
    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(kept["token"], PASTED);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "only the user the harness runs as may read it");
    }

    let facts = rig.adapter.provider_facts().await.unwrap();
    let credential = facts.credential.expect("it says how");
    assert_eq!(credential.method.as_deref(), Some("setup_token"));
    assert_eq!(credential.yields, CredentialYield::Token);
    assert!(credential.expires_at.is_some(), "a token lapses");
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn something_that_is_not_a_token_is_refused_and_nothing_is_kept() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let refused = rig.adapter.login(token_request("two words")).await;
    assert!(matches!(refused, Err(AdapterError::CredentialUnusable(_))));
    let states = rig
        .logins_until(|state| state.state == LoginStateKind::Failed, 20)
        .await;
    assert!(
        states.last().unwrap().error.is_some(),
        "the person is told why"
    );
    assert!(!rig.home.path().join(".orch/oauth.json").exists());
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_that_was_pasted_is_what_the_turns_present_and_signing_out_takes_it_away() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    // The harness reaches the model server on its own, not through the connector, and with the token alone.
    rig.model.set_credential(Some(PASTED));
    let env = BTreeMap::from([
        ("CLAUDE_CODE_MAX_RETRIES".to_owned(), "1".to_owned()),
        ("ANTHROPIC_BASE_URL".to_owned(), rig.model.url()),
    ]);
    rig.start(json!({}), env).await;
    rig.adapter.login(token_request(PASTED)).await.unwrap();
    rig.logins_until(|state| state.state == LoginStateKind::Completed, 20)
        .await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.provider = ProviderAccess::Native {
        provider_id: new_id(),
    };
    rig.open_with(spec).await;
    let (_, events) = rig.run("hello").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let requests = rig.model.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.path.ends_with("/v1/messages")
                && request
                    .credential
                    .presented_by
                    .contains(&Presentation::Bearer)),
        "the token went to the model as a bearer"
    );

    rig.adapter.logout().await.unwrap();
    let states = rig
        .logins_until(|state| state.state == LoginStateKind::Idle, 20)
        .await;
    assert!(!states.is_empty());
    assert!(!rig.home.path().join(".orch/oauth.json").exists());
    assert!(matches!(
        rig.adapter.provider_facts().await,
        Err(AdapterError::LoginRequired)
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_browser_sign_in_shows_the_address_and_asks_for_the_code_and_can_be_given_up() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.adapter
        .login(LoginRequest {
            method: "browser_code".into(),
            values: LoginValues::default(),
            config: json!({}),
        })
        .await
        .unwrap();
    let states = rig
        .logins_until(|state| state.state == LoginStateKind::AwaitingUser, 60)
        .await;
    let waiting = states.last().unwrap();
    let address = waiting
        .shows
        .iter()
        .find(|show| show.kind == ShowKind::Uri)
        .expect("the address to open");
    assert!(address.value.starts_with("https://"), "{}", address.value);
    let asks = waiting.asks.as_ref().expect("it asks for the code");
    assert_eq!(asks[0].name, "code");
    assert!(waiting.expires_at.is_some());

    rig.adapter.cancel_login().await.unwrap();
    rig.logins_until(|state| state.state == LoginStateKind::Idle, 20)
        .await;
    assert!(matches!(
        rig.adapter
            .login_input(LoginValues(BTreeMap::from([(
                "code".to_owned(),
                json!("late")
            )])))
            .await,
        Err(AdapterError::NotFound(_))
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_models_the_harness_lists_come_with_the_efforts_each_takes() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_beside(rig.loopback()).await;
    let models = rig.adapter.discover_models().await.unwrap();
    let names: Vec<_> = models
        .iter()
        .map(|model| model.model_name.as_str())
        .collect();
    assert!(names.iter().any(|name| name.contains("haiku")), "{names:?}");
    let mut once = names.clone();
    once.sort_unstable();
    once.dedup();
    assert_eq!(once.len(), names.len(), "each model is listed once");
    assert!(
        models
            .iter()
            .all(|model| model.parameters.efforts().contains(&"high".to_owned())),
        "the harness says which efforts each takes"
    );
    assert!(
        models
            .iter()
            .all(|model| model.display_name != "Default (recommended)")
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credentials_of_each_form_are_issued_from_the_home_and_never_more_than_the_form_holds() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    // Nothing to issue yet.
    assert!(matches!(
        rig.adapter.issue_credential("oauth_token").await,
        Err(AdapterError::CredentialUnusable(_))
    ));
    assert!(matches!(
        rig.adapter.issue_credential("nonsense").await,
        Err(AdapterError::Unsupported(_))
    ));

    rig.adapter.login(token_request(PASTED)).await.unwrap();
    let long_lived = rig.adapter.issue_credential("oauth_token").await.unwrap();
    assert_eq!(long_lived.credential.expose(), PASTED);
    assert!(long_lived.expires_at.is_some());

    // A login as the harness keeps it: the short-lived token, and the one that renews it.
    let login = json!({ "claudeAiOauth": {
        "accessToken": "access-aaa", "refreshToken": "refresh-rrr",
        "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000,
        "scopes": ["user:inference"], "subscriptionType": "max"
    }});
    std::fs::write(rig.home.path().join(".credentials.json"), login.to_string()).unwrap();
    let access = rig.adapter.issue_credential("access_token").await.unwrap();
    assert_eq!(access.credential.expose(), "access-aaa");
    assert!(
        !format!("{:?}{}", access.expires_at, access.credential.expose()).contains("refresh-rrr"),
        "not the token that renews it"
    );
    assert!(
        rig.home.path().join(".credentials.json").exists(),
        "a copy leaves the original"
    );

    let whole = rig.adapter.issue_credential("login").await.unwrap();
    assert!(whole.credential.expose().contains("refresh-rrr"));
    assert!(
        !rig.home.path().join(".credentials.json").exists(),
        "the login moves: it is not here to be used"
    );
    assert!(
        rig.home.path().join(".orch/held-login.json").exists(),
        "it waits here"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_login_handed_over_is_returned_as_it_stands_and_the_owner_takes_it_home() {
    let Some(mut owner) = Rig::new().await else {
        return;
    };
    let Some(mut away) = Rig::new().await else {
        return;
    };
    owner.start_quick().await;
    away.start_quick().await;
    let login =
        json!({ "claudeAiOauth": { "accessToken": "old", "refreshToken": "r1", "expiresAt": 1 }});
    std::fs::write(
        owner.home.path().join(".credentials.json"),
        login.to_string(),
    )
    .unwrap();

    let issued = owner.adapter.issue_credential("login").await.unwrap();
    let given = issued.into_given(None);
    away.adapter.set_credential(given).await.unwrap();
    let there = away.home.path().join(".credentials.json");
    assert!(
        there.exists(),
        "the harness reads it where it keeps its login"
    );

    // The harness renewed it while it was away.
    let renewed =
        json!({ "claudeAiOauth": { "accessToken": "new", "refreshToken": "r2", "expiresAt": 2 }});
    std::fs::write(&there, renewed.to_string()).unwrap();
    let back = away.adapter.return_credential().await.unwrap();
    assert_eq!(back.form, "login");
    assert!(!there.exists(), "it is not used here after it is returned");
    assert!(back.credential.expose().contains("r2"));

    owner.adapter.accept_credential(back).await.unwrap();
    let home = std::fs::read_to_string(owner.home.path().join(".credentials.json")).unwrap();
    assert!(home.contains("r2"), "the owner has what is current");
    assert!(!owner.home.path().join(".orch/held-login.json").exists());

    // A form the harness cannot run on is refused.
    let refused = away
        .adapter
        .set_credential(GivenCredential {
            form: "api_key".into(),
            credential: Secret::new("k"),
            expires_at: None,
            upstream: None,
        })
        .await;
    assert!(matches!(refused, Err(AdapterError::CredentialUnusable(_))));
    owner.adapter.stop().await.unwrap();
    away.adapter.stop().await.unwrap();
}

/// The reasons the harness asked for another credential, in the order it asked.
fn needs(events: &[SessionEvent]) -> Vec<CredentialNeed> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::CredentialNeeded(needed) => Some(needed.reason),
            _ => None,
        })
        .collect()
}

fn copy(token: &str) -> GivenCredential {
    GivenCredential {
        form: "oauth_token".into(),
        credential: Secret::new(token),
        expires_at: None,
        upstream: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_on_a_copy_asks_for_it_first_runs_on_it_and_asks_again_when_it_is_refused() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    // The harness keeps its own number of tries: a refused copy must not be tried for minutes.
    let env = BTreeMap::from([("ANTHROPIC_BASE_URL".to_owned(), rig.model.url())]);
    rig.start(json!({}), env).await;
    rig.model.set_credential(Some("copy-one"));
    let mut spec = rig.spec(SessionMode::New, None);
    spec.provider = ProviderAccess::Shared {
        provider_id: new_id(),
        form: "oauth_token".into(),
    };
    rig.open_with(spec).await;

    // Nothing to run on yet: the connector is asked, and no process is started.
    let refused = rig.adapter.start_turn(rig.turn("hello")).await;
    assert!(matches!(refused, Err(AdapterError::CredentialUnusable(_))));
    assert_eq!(needs(&rig.drain(300).await), [CredentialNeed::Missing]);
    assert!(rig.model.requests().is_empty());

    // Given the copy, the turn runs on it.
    rig.adapter.set_credential(copy("copy-one")).await.unwrap();
    let (_, events) = rig.run("hello").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert!(needs(&events).is_empty());

    // The copy lapses: the next turn fails at the first refusal, the connector is told before the turn ends,
    // and the error says that trying again after a new copy may help.
    rig.model.set_credential(Some("copy-two"));
    let started = std::time::Instant::now();
    let (_, events) = rig.run("again").await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(40),
        "not the harness's ten tries"
    );
    assert_eq!(needs(&events), [CredentialNeed::Rejected]);
    let positions: Vec<_> = common::kinds(&events);
    let needed_at = positions
        .iter()
        .position(|kind| *kind == "credential.needed")
        .unwrap();
    let ended_at = positions
        .iter()
        .position(|kind| *kind == "turn.completed")
        .unwrap();
    assert!(needed_at < ended_at, "{positions:?}");
    let SessionEvent::TurnCompleted(end) = events.last().unwrap() else {
        panic!("the turn ended");
    };
    assert_eq!(end.status, TurnOutcome::Failed);
    let error = end.error.as_ref().unwrap();
    assert_eq!(error.code, "provider_unauthorized");
    assert!(error.retryable);

    // A new copy and the conversation goes on where it was.
    rig.adapter.set_credential(copy("copy-two")).await.unwrap();
    let (_, events) = rig.run("once more").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert!(
        rig.last_request().contains("hello"),
        "it remembers the first turn"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_on_a_handed_over_login_asks_for_it_when_the_home_holds_none() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.provider = ProviderAccess::Handover {
        provider_id: new_id(),
        form: "login".into(),
    };
    rig.open_with(spec).await;
    let refused = rig.adapter.start_turn(rig.turn("hello")).await;
    assert!(matches!(refused, Err(AdapterError::CredentialUnusable(_))));
    assert_eq!(needs(&rig.drain(300).await), [CredentialNeed::Missing]);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_handed_over_login_is_the_one_the_harness_signs_in_with_and_runs_on() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let env = BTreeMap::from([
        ("CLAUDE_CODE_MAX_RETRIES".to_owned(), "1".to_owned()),
        ("ANTHROPIC_BASE_URL".to_owned(), rig.model.url()),
    ]);
    rig.start(json!({}), env).await;
    rig.model.set_credential(Some("handed-access-token"));
    let login = json!({ "claudeAiOauth": {
        "accessToken": "handed-access-token", "refreshToken": "handed-refresh-token",
        "expiresAt": chrono::Utc::now().timestamp_millis() + 24 * 3_600_000,
        "scopes": ["user:inference", "user:profile"], "subscriptionType": "max"
    }});
    rig.adapter
        .set_credential(GivenCredential {
            form: "login".into(),
            credential: Secret::new(login.to_string()),
            expires_at: None,
            upstream: None,
        })
        .await
        .unwrap();
    let facts = rig.adapter.provider_facts().await.unwrap();
    assert_eq!(facts.plan.as_deref(), Some("max"));
    assert_eq!(facts.credential.unwrap().yields, CredentialYield::Login);

    let mut spec = rig.spec(SessionMode::New, None);
    spec.provider = ProviderAccess::Handover {
        provider_id: new_id(),
        form: "login".into(),
    };
    rig.open_with(spec).await;
    let (_, events) = rig.run("hello").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert!(
        rig.model.requests().iter().any(|request| request
            .credential
            .presented_by
            .contains(&Presentation::Bearer)),
        "the login's access token went to the model as a bearer"
    );
    rig.adapter.stop().await.unwrap();
}
