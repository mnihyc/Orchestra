//! The scripted adapter as a harness: how it starts and what it declares, the failures a profile injects, the sign-in and the credentials it runs on, and the servers and models it lists.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{Fake, names, of, said};
use orch_agent_contract::adapter::{AdapterError, AgentAdapter};
use orch_agent_contract::credentials::GivenCredential;
use orch_agent_contract::descriptor::{ArrangementType, MemoryKind};
use orch_agent_contract::discovery::{McpImportRequest, McpScope, McpSearch};
use orch_agent_contract::login::{LoginRequest, LoginValues};
use orch_agent_contract::session::SessionMode;
use orch_plugin_contract::Secret;
use orch_types::{Clock as _, Json, new_id};
use serde_json::json;

/// The input of a script: the marker, then the lines.
macro_rules! script {
    ($($line:expr),+ $(,)?) => {
        format!("#!fake\n{}", [$($line.to_string()),+].join("\n"))
    };
}

fn login(method: &str, values: Json) -> LoginRequest {
    LoginRequest {
        method: method.to_owned(),
        values: serde_json::from_value(values).unwrap(),
        config: json!({ "tenant": "acme" }),
    }
}

fn given(form: &str, secret: &str, expires_in_secs: Option<i64>, fake: &Fake) -> GivenCredential {
    GivenCredential {
        form: form.to_owned(),
        credential: Secret::new(secret),
        expires_at: expires_in_secs
            .map(|secs| orch_types::Time::new(fake.clock.now() + chrono::Duration::seconds(secs))),
        upstream: None,
    }
}

// ---------------------------------------------------------------------------------------------------------
// Starting

#[tokio::test(start_paused = true)]
async fn starting_answers_the_version_and_the_descriptor_and_a_start_with_the_same_profile_changes_nothing()
 {
    let mut fake = Fake::start().await;
    assert_eq!(fake.started.harness_version, "1.0.0");
    assert_eq!(fake.started.descriptor.kind, "fake");
    assert_eq!(fake.started.descriptor.harness.version, "1.0.0");
    assert_eq!(
        fake.started.descriptor.harness.supported_range,
        ">=1.0.0, <2.0.0"
    );
    assert_eq!(fake.adapter.probe().await.unwrap(), fake.started.descriptor);
    assert_eq!(fake.adapter.kind(), "fake");
    assert!(
        !fake.started.descriptor.capabilities.mcp.reload,
        "the default unsupported reload method is never advertised; server changes use resume"
    );

    let session = fake.session().await;
    let again = Fake::agent_start(fake.provider, fake.agent, &fake.home_path(), json!({}));
    let started = fake.adapter.start(again, fake.sink()).await.unwrap();
    assert_eq!(started, fake.started);
    let events = fake.say(&session, "still open").await;
    assert_eq!(
        said(&events),
        ["You said: still open"],
        "the session survived the second start"
    );

    // Another profile is another harness: it starts anew, and what the first held is gone.
    let other = Fake::agent_start(
        fake.provider,
        fake.agent,
        &fake.home_path(),
        json!({ "memory": "home" }),
    );
    let started = fake.adapter.start(other, fake.sink()).await.unwrap();
    assert_eq!(started.descriptor.capabilities.memory, MemoryKind::Home);
    let turn = fake.turn_of(&session, "hi");
    assert!(matches!(
        fake.adapter.start_turn(turn).await.unwrap_err(),
        AdapterError::SessionNotFound(_)
    ));
}

fn mcp_states(events: &[common::Seen]) -> Json {
    of(events, "item.completed")
        .into_iter()
        .filter(|event| event.body["payload"]["tool"]["kind"] == "mcp")
        .map(|event| {
            json!({
                "server": event.body["payload"]["tool"]["server"],
                "status": event.body["status"]
            })
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn mcp_changes_are_applied_on_resume_without_replacing_the_conversation_or_history() {
    let mut fake = Fake::start().await;
    let mut spec = fake.spec(SessionMode::New);
    spec.mcp = vec!["before".to_owned()];
    let (session, _) = fake.open_full(spec.clone()).await;
    let events = fake
        .script(&session, &["say retained-history", "mcp before ping {}"])
        .await;
    assert_eq!(
        mcp_states(&events),
        json!([{"server":"before","status":"completed"}])
    );
    assert_eq!(said(&events), ["retained-history"]);

    let error = fake
        .adapter
        .reload_mcp(session.thread, vec!["after".to_owned()])
        .await
        .unwrap_err();
    assert!(matches!(error, AdapterError::Unsupported(ref operation) if operation == "mcp.reload"));
    let events = fake
        .script(&session, &["mcp before ping {}", "mcp after ping {}"])
        .await;
    assert_eq!(
        mcp_states(&events),
        json!([
            {"server":"before","status":"completed"},
            {"server":"after","status":"failed"}
        ]),
        "a refused live refresh has not changed the actual accepted set"
    );
    let before = fake.file(&session.native_ref);
    spec.settings.mode = SessionMode::Resume;
    spec.settings.native_session_ref = Some(session.native_ref.clone());
    spec.mcp = vec!["after".to_owned()];
    let (resumed, opened) = fake.open_full(spec).await;
    assert!(opened.resumed);
    assert_eq!(resumed.native_ref, session.native_ref);
    assert_eq!(resumed.thread, session.thread);
    assert_eq!(resumed.segment, session.segment);
    let accepted = fake.file(&session.native_ref);
    assert_eq!(
        accepted["turns"], before["turns"],
        "reconfiguration adds no hidden turn"
    );
    assert_eq!(
        accepted["items"], before["items"],
        "reconfiguration rewrites no history"
    );

    let events = fake
        .script(&resumed, &["mcp before ping {}", "mcp after ping {}"])
        .await;
    assert_eq!(
        mcp_states(&events),
        json!([
            {"server":"before","status":"failed"},
            {"server":"after","status":"completed"}
        ]),
        "resume really replaces the server set without claiming a live reload"
    );
    let after = fake.file(&session.native_ref);
    let kept = before["turns"].as_array().unwrap();
    assert_eq!(&after["turns"].as_array().unwrap()[..kept.len()], kept);
    let kept = before["items"].as_array().unwrap();
    assert_eq!(&after["items"].as_array().unwrap()[..kept.len()], kept);
    fake.adapter.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn nothing_works_before_the_harness_is_started_or_after_it_is_stopped() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.adapter.stop().await.unwrap();
    for error in [
        fake.adapter.probe().await.unwrap_err(),
        fake.adapter
            .open_session(fake.spec(SessionMode::New))
            .await
            .unwrap_err(),
        fake.adapter
            .close_session(session.thread)
            .await
            .unwrap_err(),
        fake.adapter.discover_sessions().await.unwrap_err(),
    ] {
        assert_eq!(error.code(), "harness_unavailable");
    }
    // Stopped, it can be started again.
    let restarted = Fake::agent_start(fake.provider, fake.agent, &fake.home_path(), json!({}));
    fake.adapter.start(restarted, fake.sink()).await.unwrap();
    fake.adapter.probe().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_harness_version_below_the_range_is_refused_and_one_above_it_is_run_untested() {
    let mut fake = Fake::start().await;
    let mut old = Fake::agent_start(fake.provider, fake.agent, &fake.home_path(), json!({}));
    old.profile.harness_version = Some("0.9.0".to_owned());
    let error = fake.adapter.start(old, fake.sink()).await.unwrap_err();
    assert_eq!(error.code(), "harness_unavailable");
    assert!(
        error.detail().contains(">=1.0.0, <2.0.0"),
        "{}",
        error.detail()
    );

    let mut new = Fake::agent_start(fake.provider, fake.agent, &fake.home_path(), json!({}));
    new.profile.harness_version = Some("2.4.0".to_owned());
    let started = fake.adapter.start(new, fake.sink()).await.unwrap();
    assert_eq!(started.harness_version, "2.4.0");
    assert_eq!(
        serde_json::to_value(started.descriptor.harness.status).unwrap(),
        "untested"
    );
}

#[tokio::test(start_paused = true)]
async fn a_profile_that_asks_for_what_does_not_hold_is_refused_before_anything_starts() {
    let mut fake = Fake::start().await;
    for config in [
        json!({ "memory": "everywhere" }),
        json!({ "unknown_member": true }),
        json!({ "faults": { "no_such_call": { "code": "x" } } }),
    ] {
        let start = Fake::agent_start(fake.provider, fake.agent, &fake.home_path(), config.clone());
        let error = fake.adapter.start(start, fake.sink()).await.unwrap_err();
        assert!(!error.detail().is_empty(), "{config}: {error}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_capabilities_and_the_arrangements_follow_the_profile() {
    let fake = Fake::start_with(json!({
        "separable": false,
        "capabilities": { "steer": false, "memory": "directory" },
        "forwarded_protocols": ["responses"],
        "approval_modes": ["ask", "bypass"],
        "sandbox_modes": ["workspace"]
    }))
    .await;
    let descriptor = &fake.started.descriptor;
    assert!(!descriptor.capabilities.steer);
    assert_eq!(descriptor.forwarded_protocols, ["responses"]);
    assert_eq!(descriptor.approval_modes.len(), 2);
    assert_eq!(descriptor.sandbox_modes.len(), 1);
    let kinds: Vec<ArrangementType> = descriptor.arrangements.iter().map(|a| a.kind).collect();
    assert!(!kinds.contains(&ArrangementType::Separated));
    assert!(!kinds.contains(&ArrangementType::KeySeparated));
    assert!(kinds.contains(&ArrangementType::Together));

    let all = Fake::start().await;
    let kinds: Vec<ArrangementType> = all
        .started
        .descriptor
        .arrangements
        .iter()
        .map(|a| a.kind)
        .collect();
    assert!(
        kinds.contains(&ArrangementType::Separated)
            && kinds.contains(&ArrangementType::KeySeparated)
    );
}

// ---------------------------------------------------------------------------------------------------------
// Failures a profile injects

#[tokio::test(start_paused = true)]
async fn a_fault_fails_the_call_it_names_as_often_as_it_says_after_the_calls_it_lets_pass() {
    let mut fake = Fake::start_with(json!({
        "faults": {
            "start_turn": { "code": "harness_error", "message": "the harness stumbled", "times": 2, "after": 1 },
            "discover_sessions": { "code": "timeout", "times": 0 },
            "close_session": { "code": "limited", "message": "slow down", "retryable": true }
        }
    }))
    .await;
    let session = fake.session().await;
    // The first call passes, the next two fail, and the fourth passes again.
    let outcomes = [
        fake.adapter.start_turn(fake.turn_of(&session, "a")).await,
        fake.adapter.start_turn(fake.turn_of(&session, "b")).await,
        fake.adapter.start_turn(fake.turn_of(&session, "c")).await,
    ];
    assert!(outcomes[0].is_ok());
    for failed in &outcomes[1..] {
        let error = failed.as_ref().unwrap_err();
        assert_eq!(error.code(), "harness_error");
        assert_eq!(error.detail(), "the harness stumbled");
        assert!(!error.retryable());
    }
    fake.until(|e| e.is("turn.completed")).await;
    fake.say(&session, "d").await;

    // `times: 0` fails every call.
    for _ in 0..3 {
        assert_eq!(
            fake.adapter.discover_sessions().await.unwrap_err().code(),
            "timeout"
        );
    }
    // The error says what the profile asked for, retry included.
    let error = fake
        .adapter
        .close_session(session.thread)
        .await
        .unwrap_err();
    assert_eq!((error.code(), error.retryable()), ("limited", true));
    fake.adapter.close_session(session.thread).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_resume_has_a_fault_of_its_own_beside_the_open() {
    let mut fake = Fake::start_with(json!({
        "faults": { "resume": { "code": "session_not_found", "message": "gone", "times": 1 } }
    }))
    .await;
    let session = fake.session().await;
    fake.adapter.close_session(session.thread).await.unwrap();
    let mut spec = fake.spec(SessionMode::Resume);
    spec.settings.native_session_ref = Some(session.native_ref.clone());
    let error = fake.adapter.open_session(spec.clone()).await.unwrap_err();
    assert_eq!(error.code(), "session_not_found");
    let resumed = fake.adapter.open_session(spec).await.unwrap();
    assert!(resumed.resumed);
}

// ---------------------------------------------------------------------------------------------------------
// Signing in

#[tokio::test(start_paused = true)]
async fn a_sign_in_by_a_code_shows_an_address_and_a_code_waits_and_completes_when_it_is_answered() {
    let mut fake = Fake::start().await;
    fake.adapter
        .login(login("fake_code", json!({})))
        .await
        .unwrap();
    let waiting = fake.login_state("awaiting_user").await;
    let state = &waiting.last().unwrap().body;
    assert_eq!(state["state"], "awaiting_user");
    assert_eq!(state["method"], "fake_code");
    let shows = state["shows"].as_array().unwrap();
    assert!(shows.iter().any(
        |show| show["kind"] == "uri" && show["value"].as_str().unwrap().starts_with("https://")
    ));
    assert!(
        shows
            .iter()
            .any(|show| show["kind"] == "code" && !show["value"].as_str().unwrap().is_empty())
    );
    assert!(state["expires_at"].is_string(), "the code lapses");
    assert!(
        fake.adapter.provider_facts().await.is_err(),
        "not signed in yet"
    );

    fake.adapter
        .login_input(LoginValues::default())
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("provider.facts")).await;
    assert_eq!(
        of(&rest, "provider.login").last().unwrap().body["state"],
        "completed"
    );
    let facts = fake.adapter.provider_facts().await.unwrap();
    assert_eq!(facts.identity.as_deref(), Some("account@acme"));
    assert_eq!(facts.plan.as_deref(), Some("pro"));
}

#[tokio::test(start_paused = true)]
async fn a_code_nobody_answers_ends_by_itself_after_the_time_it_was_good_for() {
    let mut fake = Fake::start_with(json!({ "code_lifetime_secs": 90 })).await;
    fake.adapter
        .login(login("fake_code", json!({})))
        .await
        .unwrap();
    let waiting = fake.login_state("awaiting_user").await;
    let begun = waiting.last().unwrap().at;
    let ended = fake.login_state("expired").await;
    assert_eq!(ended.last().unwrap().body["state"], "expired");
    assert_eq!(ended.last().unwrap().at - begun, Duration::from_secs(90));
    assert!(
        fake.adapter.provider_facts().await.is_err(),
        "an expired code signed nobody in"
    );
}

#[tokio::test(start_paused = true)]
async fn a_sign_in_on_a_page_asks_for_a_text_and_is_cancelled_or_signed_out_without_trace() {
    let mut fake = Fake::start().await;
    fake.adapter
        .login(login("fake_ask", json!({})))
        .await
        .unwrap();
    let asking = fake.login_state("awaiting_user").await;
    assert_eq!(asking.last().unwrap().body["asks"][0]["name"], "answer");
    fake.adapter.cancel_login().await.unwrap();
    let cancelled = fake.login_state("idle").await;
    assert_eq!(cancelled.last().unwrap().body["state"], "idle");
    assert!(fake.adapter.provider_facts().await.is_err());

    fake.adapter
        .login(login("fake_ask", json!({})))
        .await
        .unwrap();
    fake.login_state("awaiting_user").await;
    fake.adapter
        .login_input(serde_json::from_value(json!({ "answer": "yes" })).unwrap())
        .await
        .unwrap();
    fake.until(|e| e.is("provider.facts")).await;
    assert!(fake.adapter.provider_facts().await.is_ok());
    fake.adapter.logout().await.unwrap();
    assert!(matches!(
        fake.adapter.provider_facts().await.unwrap_err(),
        AdapterError::LoginRequired
    ));
}

#[tokio::test(start_paused = true)]
async fn a_credential_the_person_brings_is_taken_from_its_text_or_its_file_and_ends_when_it_says() {
    let mut fake = Fake::start().await;
    let path = fake.home_path().join("token.txt");
    std::fs::write(&path, "tok-from-a-file").unwrap();
    let values = json!({
        "credential_path": path.display().to_string(),
        "expires_at": "2026-12-24"
    });
    fake.adapter
        .login(login("fake_import", values))
        .await
        .unwrap();
    let events = fake.until(|e| e.is("provider.facts")).await;
    assert_eq!(
        of(&events, "provider.login").last().unwrap().body["state"],
        "completed"
    );
    let facts = fake.adapter.provider_facts().await.unwrap();
    assert_eq!(
        facts.credential.unwrap().expires_at.unwrap().to_string(),
        "2026-12-24T23:59:59.999Z",
        "the day the person gave is the token's end"
    );
    // The text of the credential is told to nobody.
    assert!(!format!("{events:?}").contains("tok-from-a-file"));
}

// ---------------------------------------------------------------------------------------------------------
// The credential a harness runs on

fn shared_spec(fake: &Fake) -> orch_agent_contract::session::SessionSpec {
    let mut spec = fake.spec(SessionMode::New);
    spec.settings.arrangement = ArrangementType::CredentialShared;
    spec
}

#[tokio::test(start_paused = true)]
async fn a_turn_on_a_shared_credential_waits_for_one_asks_for_it_and_goes_on_when_it_is_set() {
    let mut fake = Fake::start().await;
    let spec = shared_spec(&fake);
    let session = fake.open(spec).await;
    fake.begin(&session, &script!("say with a key")).await;
    let events = fake.until(|e| e.is("credential.needed")).await;
    assert_eq!(events.last().unwrap().body["reason"], "missing");
    assert!(
        fake.wait(3_600_000).await.is_empty(),
        "the turn waits for as long as it takes"
    );
    let key = given("api_key", "k-1", None, &fake);
    fake.adapter.set_credential(key).await.unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(said(&rest), ["with a key"]);
    assert_eq!(rest.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn a_credential_of_a_form_the_harness_does_not_run_on_or_that_has_lapsed_is_refused() {
    let mut fake = Fake::start().await;
    let strange = given("bearer_of_nothing", "x", None, &fake);
    assert_eq!(
        fake.adapter
            .set_credential(strange)
            .await
            .unwrap_err()
            .code(),
        "credential_unusable"
    );
    let lapsed = given("fake_token", "x", Some(-1), &fake);
    assert_eq!(
        fake.adapter
            .set_credential(lapsed)
            .await
            .unwrap_err()
            .code(),
        "credential_unusable"
    );
}

#[tokio::test(start_paused = true)]
async fn an_expire_has_the_credential_rejected_and_the_turn_waits_until_another_is_set() {
    let mut fake = Fake::start().await;
    let spec = shared_spec(&fake);
    let session = fake.open(spec).await;
    let key = given("api_key", "k-1", None, &fake);
    fake.adapter.set_credential(key).await.unwrap();
    fake.begin(&session, &script!("say before", "expire", "say after"))
        .await;
    let events = fake.until(|e| e.is("credential.needed")).await;
    assert_eq!(events.last().unwrap().body["reason"], "rejected");
    assert_eq!(said(&events), ["before"]);
    assert!(fake.wait(60_000).await.is_empty());
    let key = given("api_key", "k-2", None, &fake);
    fake.adapter.set_credential(key).await.unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(said(&rest), ["after"]);
}

#[tokio::test(start_paused = true)]
async fn a_copy_that_lapses_is_asked_for_again_before_it_does_and_a_new_turn_refuses_to_start_on_a_lapsed_one()
 {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let issued = fake.adapter.issue_credential("fake_token").await.unwrap();
    assert_eq!(issued.form, "fake_token");
    let lapses = issued.expires_at.unwrap();
    assert_eq!(
        lapses.unix_millis() - fake.clock.time().unix_millis(),
        120_000
    );
    fake.adapter
        .set_credential(issued.into_given(None))
        .await
        .unwrap();

    let spec = shared_spec(&fake);
    let session = fake.open(spec).await;
    let events = fake.say(&session, "fresh").await;
    assert!(of(&events, "credential.needed").is_empty());

    // Within the margin of its end the harness says it will need another, and goes on.
    fake.clock.advance(chrono::Duration::seconds(100));
    let events = fake.say(&session, "late").await;
    let needed = of(&events, "credential.needed");
    assert_eq!(needed.len(), 1);
    assert_eq!(needed[0].body["reason"], "expiring");
    assert_eq!(names(&events).last().copied(), Some("turn.completed"));

    // Past its end, it is a rejection, and the turn waits.
    fake.clock.advance(chrono::Duration::seconds(30));
    fake.begin(&session, "too late").await;
    let events = fake.until(|e| e.is("credential.needed")).await;
    assert_eq!(events.last().unwrap().body["reason"], "rejected");
}

#[tokio::test(start_paused = true)]
async fn a_second_copy_within_thirty_seconds_is_refused_as_too_soon_and_a_login_that_moves_leaves_none_to_issue_from()
 {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    fake.adapter.issue_credential("fake_token").await.unwrap();
    let error = fake
        .adapter
        .issue_credential("fake_token")
        .await
        .unwrap_err();
    assert_eq!((error.code(), error.retryable()), ("too_soon", true));
    fake.clock.advance(chrono::Duration::seconds(31));
    fake.adapter.issue_credential("fake_token").await.unwrap();
    assert_eq!(
        fake.adapter
            .issue_credential("no_such_form")
            .await
            .unwrap_err()
            .code(),
        "unsupported"
    );

    // The login itself moves: here it is unusable until it comes back with what it became.
    let moved = fake.adapter.issue_credential("fake_login").await.unwrap();
    assert!(moved.expires_at.is_none());
    assert_eq!(
        fake.adapter
            .issue_credential("api_key")
            .await
            .unwrap_err()
            .code(),
        "credential_unusable"
    );
    let mut renewed = moved.into_given(None);
    renewed.credential = Secret::new(format!("{}~r1", renewed.credential.expose()));
    fake.adapter.accept_credential(renewed).await.unwrap();
    fake.clock.advance(chrono::Duration::seconds(31));
    fake.adapter.issue_credential("api_key").await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_login_that_moved_renews_with_each_turn_on_it_and_comes_back_as_it_became() {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let moved = fake.adapter.issue_credential("fake_login").await.unwrap();
    let original = moved.credential.expose().to_owned();
    // The side that runs the harness is given it.
    let mut runner = Fake::start().await;
    let spec = {
        let mut spec = runner.spec(SessionMode::New);
        spec.settings.arrangement = ArrangementType::HandedOver;
        spec
    };
    let session = runner.open(spec).await;
    runner
        .adapter
        .set_credential(moved.into_given(None))
        .await
        .unwrap();
    runner.say(&session, "one").await;
    runner.say(&session, "two").await;
    let back = runner.adapter.return_credential().await.unwrap();
    assert_eq!(
        back.credential.expose(),
        format!("{original}~r2"),
        "two turns, two renewals"
    );
    assert!(
        runner.adapter.return_credential().await.is_err(),
        "once given back it is not held"
    );
    fake.adapter.accept_credential(back).await.unwrap();
    fake.clock.advance(chrono::Duration::seconds(31));
    fake.adapter.issue_credential("fake_token").await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_providers_facts_fill_with_turns() {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let session = fake.session().await;
    let used = |facts: &orch_agent_contract::facts::ProviderFacts| facts.windows[0].used_percent;
    let before = used(&fake.adapter.provider_facts().await.unwrap());
    fake.say(&session, "one").await;
    fake.say(&session, "two").await;
    let after = used(&fake.adapter.provider_facts().await.unwrap());
    assert!(after > before, "{before} -> {after}");
    assert!(after < 100.0, "a few turns do not use the plan up");
}

// ---------------------------------------------------------------------------------------------------------
// What it lists

#[tokio::test(start_paused = true)]
async fn the_servers_the_harness_has_configured_are_listed_and_read_with_their_secrets() {
    let fake_config = json!({
        "mcp_servers": [
            { "name": "docs", "command": ["docs-server", "--stdio"], "env": { "MODE": "ro" }, "secrets": { "DOCS_TOKEN": "s3cret" } },
            { "name": "web", "url": "https://tools.invalid/mcp" }
        ]
    });
    let mut fake = Fake::start_with(fake_config).await;
    let found = fake
        .adapter
        .discover_mcp(McpSearch::default())
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["docs", "web"]
    );
    assert!(
        found
            .iter()
            .all(|f| f.source == "fake" && f.scope == McpScope::User)
    );
    assert_ne!(found[0].digest, found[1].digest);
    let read = fake
        .adapter
        .read_mcp(McpImportRequest {
            server_id: new_id(),
            source: "fake".to_owned(),
            scope: McpScope::User,
            name: "docs".to_owned(),
            directory: None,
        })
        .await
        .unwrap();
    assert_eq!(
        read.spec.command,
        Some(vec!["docs-server".to_owned(), "--stdio".to_owned()])
    );
    assert_eq!(read.spec.env["MODE"], "ro");
    assert_eq!(read.spec.secret_env, ["DOCS_TOKEN"]);
    assert_eq!(read.secrets["DOCS_TOKEN"].expose(), "s3cret");
    let missing = fake
        .adapter
        .read_mcp(McpImportRequest {
            server_id: new_id(),
            source: "fake".to_owned(),
            scope: McpScope::User,
            name: "nope".to_owned(),
            directory: None,
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");
}

#[tokio::test(start_paused = true)]
async fn the_models_it_offers_are_two() {
    let mut fake = Fake::start().await;
    let models = fake.adapter.discover_models().await.unwrap();
    let names: Vec<&str> = models.iter().map(|m| m.model_name.as_str()).collect();
    assert_eq!(names, ["fake-small", "fake-large"]);
}
