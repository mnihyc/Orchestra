//! A sign-in on the provider, driven by the system through the connector: the method started, what it shows and asks reported as events of the provider's stream in the order they happen, what the person gives back, a sign-in given up, and a secret that is sealed to the machine on its way and is in no event after.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::model::ProviderRig;
use orch_link_contract::commands::{
    ProviderLoginCancel, ProviderLoginInput, ProviderLoginStart, ProviderLogout, SerialKey,
};
use orch_pki::sealing::{RecipientKey, info_for_command, seal};
use orch_types::{Id, new_id};
use serde_json::json;

fn key(p: &ProviderRig) -> SerialKey {
    SerialKey::Harness {
        provider_id: p.rig.provider,
        agent_id: p.rig.agent,
    }
}

fn login_states(p: &ProviderRig) -> Vec<String> {
    p.provider_events_of("provider.login")
        .iter()
        .map(|event| event.body["state"].as_str().unwrap().to_owned())
        .collect()
}

async fn start(p: &ProviderRig, method: &str, values: serde_json::Value) {
    let body: ProviderLoginStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "method": method,
        "values": values, "config": { "tenant": "acme" }
    }))
    .unwrap();
    p.rig.succeed(&key(p), &body).await;
}

#[tokio::test]
async fn a_sign_in_by_a_code_shows_what_the_person_does_and_completes_when_they_have_done_it() {
    let p = ProviderRig::new().await;
    p.start_harness().await;
    start(&p, "fake_code", json!(null)).await;
    let waiting = p.wait_provider_event("provider.login", 2).await;
    assert_eq!(waiting[0].body["state"], "starting");
    assert_eq!(waiting[1].body["state"], "awaiting_user");
    assert_eq!(waiting[1].body["method"], "fake_code");
    let shown = waiting[1].body["shows"].to_string();
    assert!(
        shown.contains("fake.invalid"),
        "an address to open: {shown}"
    );
    assert!(
        waiting[1].body["expires_at"].is_string(),
        "a code that runs out says when"
    );

    let answer: ProviderLoginInput = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "values": {}
    }))
    .unwrap();
    p.rig.succeed(&key(&p), &answer).await;
    let done = p.wait_provider_event("provider.login", 3).await;
    assert_eq!(login_states(&p), ["starting", "awaiting_user", "completed"]);
    assert_eq!(done[2].body["method"], "fake_code");
}

#[tokio::test]
async fn a_sign_in_on_a_page_asks_for_a_text_and_is_answered() {
    let p = ProviderRig::new().await;
    p.start_harness().await;
    start(&p, "fake_ask", json!(null)).await;
    let asked = p.wait_provider_event("provider.login", 2).await;
    assert_eq!(asked[1].body["state"], "awaiting_user");
    assert_eq!(asked[1].body["asks"][0]["name"], "answer");
    let answer: ProviderLoginInput = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "values": { "answer": "the text the page showed" }
    }))
    .unwrap();
    p.rig.succeed(&key(&p), &answer).await;
    p.wait_provider_event("provider.login", 3).await;
    assert_eq!(
        login_states(&p).last().map(String::as_str),
        Some("completed")
    );
}

#[tokio::test]
async fn a_sign_in_the_person_gives_up_is_cancelled_and_the_harness_signed_out_says_so() {
    let p = ProviderRig::new().await;
    p.start_harness().await;
    start(&p, "fake_code", json!(null)).await;
    p.wait_provider_event("provider.login", 2).await;
    p.rig
        .succeed(
            &key(&p),
            &ProviderLoginCancel {
                provider_id: p.rig.provider,
                agent_id: p.rig.agent,
            },
        )
        .await;
    let states = {
        p.wait_provider_event("provider.login", 3).await;
        login_states(&p)
    };
    assert_eq!(states.len(), 3, "{states:?}");
    assert!(
        matches!(states[2].as_str(), "idle" | "failed"),
        "a sign-in that was given up is no longer waiting: {states:?}"
    );

    // Signed in another way, then signed out.
    start(
        &p,
        "fake_import",
        json!({ "credential_text": "the-credential-file" }),
    )
    .await;
    p.rig
        .succeed(
            &key(&p),
            &ProviderLogout {
                provider_id: p.rig.provider,
                agent_id: p.rig.agent,
            },
        )
        .await;
    let before = login_states(&p).len();
    assert!(before >= 4);
    let refresh: orch_link_contract::commands::ProviderRefresh =
        serde_json::from_value(json!({ "provider_id": p.rig.provider })).unwrap();
    let state = p
        .rig
        .succeed(&SerialKey::Provider(p.rig.provider), &refresh)
        .await;
    assert_eq!(
        state["status"], "needs_setup",
        "a harness signed out needs a sign-in"
    );
}

#[tokio::test]
async fn a_secret_given_to_a_sign_in_is_sealed_on_its_way_and_in_no_event_after() {
    let p = ProviderRig::new().await;
    p.start_harness().await;
    let secret = "sk-signin-secret-never-to-be-shown";
    let id: Id = new_id();
    let recipient = RecipientKey::from_certificate(&p.rig.identity.certificate);
    let sealed = seal(&recipient, &info_for_command(id), secret.as_bytes()).unwrap();
    let body: ProviderLoginStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "method": "fake_import",
        "values": { "credential_text": { "$sealed": sealed } }, "config": {}
    }))
    .unwrap();
    let progress = p.command_with_id(id, &key(&p), &body).await;
    assert!(
        matches!(progress, orch_link::commands::Progress::Succeeded(_)),
        "{progress:?}"
    );
    p.wait_provider_event("provider.login", 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let told: String = p
        .provider_events()
        .iter()
        .map(|event| event.body.to_string())
        .collect();
    assert!(!told.contains(secret), "the secret is in an event: {told}");
    assert!(
        common::model::files_holding(p.rig.data.path(), secret).is_empty(),
        "the secret is in a file in clear"
    );
}

#[tokio::test]
async fn a_sign_in_that_asks_for_every_type_of_field_is_given_each_value_in_the_type_of_its_field()
{
    let p = ProviderRig::new().await;
    p.start_harness().await;
    // A command carries every value as text, and a secret sealed: the connector gives the adapter a
    // boolean as a boolean and a map as a map, or the adapter refuses the sign-in for what it is given.
    let id: Id = new_id();
    let recipient = RecipientKey::from_certificate(&p.rig.identity.certificate);
    let sealed = |text: &str| seal(&recipient, &info_for_command(id), text.as_bytes()).unwrap();
    let body: ProviderLoginStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "method": "fake_import",
        "values": {
            "account": "ada",
            "token": { "$sealed": sealed("a-token") },
            "credential_text": { "$sealed": sealed("the\ntext of a file") },
            "tier": "team",
            "verified": "true",
            "expires_at": "2027-10-03",
            "labels": "{\"team\": \"web\"}",
            "headers": { "$sealed": sealed("{\"x-key\": \"abc\"}") }
        },
        "config": {}
    }))
    .unwrap();
    let progress = p.command_with_id(id, &key(&p), &body).await;
    assert!(
        matches!(progress, orch_link::commands::Progress::Succeeded(_)),
        "{progress:?}"
    );
    p.wait_provider_event("provider.login", 2).await;
    let told = p.provider_events_of("provider.login");
    let last = told.last().unwrap();
    assert_eq!(
        last.body["state"], "completed",
        "the adapter took every value: {}",
        last.body
    );
    // A value that does not read as the type of its field is refused, naming the field and not the value.
    let id: Id = new_id();
    let sealed = |text: &str| seal(&recipient, &info_for_command(id), text.as_bytes()).unwrap();
    let wrong: ProviderLoginStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "method": "fake_import",
        "values": { "credential_text": { "$sealed": sealed("a file") }, "verified": "perhaps" },
        "config": {}
    }))
    .unwrap();
    let refused = p.command_with_id(id, &key(&p), &wrong).await;
    match refused {
        orch_link::commands::Progress::Failed(error) => {
            assert!(error.detail.contains("verified"), "{error:?}");
            assert!(!error.detail.contains("perhaps"), "{error:?}");
        }
        other => panic!("a value that does not fit its field is refused: {other:?}"),
    }
}
