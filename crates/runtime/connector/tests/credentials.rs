//! A credential that leaves the machine that keeps it, through the commands of the catalogue: a copy issued under a lease, sealed to the machine where the harness runs, installed there and renewed; a login handed over, given back and taken home. The subscription is the scripted adapter's (its forms are `fake_token`, `api_key` and `fake_login`: no real subscription is needed to check what the connector enforces), the API provider is the scripted model server's key.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::model::{ProviderRig, RESPONSES};
use orch_connector_core::credentials::{Direction, exchange_info};
use orch_link_contract::commands::SerialKey;
use orch_pki::sealing::{RecipientKey, open};
use orch_types::{Id, Json, new_id};
use serde_json::json;

/// A machine that plays both roles, and lets its credential be shared and handed over.
async fn rig() -> ProviderRig {
    ProviderRig::with(|rig, _| {
        rig.config.provider.arrangements = [
            "separated",
            "key_separated",
            "credential_shared",
            "handed_over",
        ]
        .map(String::from)
        .to_vec();
    })
    .await
}

/// This machine as the system names a recipient or a custodian: its identity and the key values are sealed to.
fn machine(p: &ProviderRig) -> Json {
    json!({
        "machine_id": p.rig.machine_id,
        "public_key": RecipientKey::from_certificate(&p.rig.identity.certificate).to_text()
    })
}

fn harness_key(p: &ProviderRig, agent: Id) -> SerialKey {
    SerialKey::Harness {
        provider_id: p.rig.provider,
        agent_id: agent,
    }
}

/// Starts a harness process for the provider and an agent profile.
async fn start_harness(p: &ProviderRig, agent: Id) {
    let body: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": agent, "harness_kind": "fake",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null
    }))
    .unwrap();
    p.rig.succeed(&harness_key(p, agent), &body).await;
}

/// Starts the harness that holds the provider's login (the rig's own agent profile) and signs it in by the
/// method that takes a credential the person brings.
async fn signed_in(p: &ProviderRig) {
    start_harness(p, p.rig.agent).await;
    let body: orch_link_contract::commands::ProviderLoginStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "method": "fake_import",
        "values": { "credential_text": "the-login-material-of-the-subscription" }, "config": {}
    }))
    .unwrap();
    p.rig.succeed(&harness_key(p, p.rig.agent), &body).await;
}

async fn sign_in_profile(p: &ProviderRig, agent: Id, material: &str) {
    start_harness(p, agent).await;
    let body: orch_link_contract::commands::ProviderLoginStart = serde_json::from_value(json!({
        "provider_id":p.rig.provider, "agent_id":agent, "method":"fake_import",
        "values":{"credential_text":material}, "config":{}
    }))
    .unwrap();
    p.rig.succeed(&harness_key(p, agent), &body).await;
}

#[tokio::test]
async fn a_returned_login_reaches_its_issuing_profile_with_another_signed_in_profile_even_after_restart()
 {
    use orch_link_contract::commands::{
        AgentCredentialReturn, ProviderCredentialAccept, ProviderLogout,
    };
    let mut p = rig().await;
    let earlier = Id::from_u128(1);
    let recipient = Id::from_u128(u128::MAX);
    start_harness(&p, earlier).await;
    signed_in(&p).await;
    start_harness(&p, recipient).await;
    let lease = open_lease(&p, "handover", "fake_login").await;
    let issued = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_login", &machine(&p)),
        )
        .await;
    p.rig
        .succeed(
            &harness_key(&p, recipient),
            &set_body(&p, recipient, &issued, "fake_login"),
        )
        .await;
    let returned = p
        .rig
        .succeed(
            &harness_key(&p, recipient),
            &AgentCredentialReturn {
                provider_id: p.rig.provider,
                agent_id: recipient,
                custodian: serde_json::from_value(machine(&p)).unwrap(),
            },
        )
        .await;
    p.rig.kill().await;
    p.rig.start().await;
    sign_in_profile(&p, earlier, "the-unrelated-profiles-login").await;
    sign_in_profile(&p, p.rig.agent, "the-issuers-login-before-its-return").await;
    let accept: ProviderCredentialAccept = serde_json::from_value(json!({
        "provider_id":p.rig.provider, "form":"fake_login", "credential":returned["credential"],
        "expires_at":returned["expires_at"]
    }))
    .unwrap();
    p.rig.succeed(&p.provider_key(), &accept).await;
    // Removing the unrelated sign-in makes the profile that received the return observable:
    // the issuer can lend the original login, not its temporary post-restart one.
    p.rig
        .succeed(
            &harness_key(&p, earlier),
            &ProviderLogout {
                provider_id: p.rig.provider,
                agent_id: earlier,
            },
        )
        .await;
    let lease = open_lease(&p, "handover", "fake_login").await;
    let lent = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_login", &machine(&p)),
        )
        .await;
    assert_eq!(
        opened(&p, &lent["credential"], Direction::Issued, "fake_login"),
        "the-login-material-of-the-subscription"
    );
}

fn lease_body(p: &ProviderRig, mode: &str, form: &str, recipient: &Json) -> (Id, Json) {
    let lease = new_id();
    (
        lease,
        json!({
            "lease_id": lease, "provider_id": p.rig.provider, "thread_id": p.thread, "mode": mode,
            "form": form, "recipient": recipient, "allowed_models": []
        }),
    )
}

async fn open_lease(p: &ProviderRig, mode: &str, form: &str) -> Id {
    let (lease, body) = lease_body(p, mode, form, &machine(p));
    let body: orch_link_contract::commands::LeaseOpen = serde_json::from_value(body).unwrap();
    p.rig.succeed(&p.provider_key(), &body).await;
    lease
}

fn issue_body(
    p: &ProviderRig,
    lease: Id,
    form: &str,
    recipient: &Json,
) -> orch_link_contract::commands::ProviderCredentialIssue {
    serde_json::from_value(json!({
        "lease_id": lease, "provider_id": p.rig.provider, "form": form, "recipient": recipient
    }))
    .unwrap()
}

fn set_body(
    p: &ProviderRig,
    agent: Id,
    issued: &Json,
    form: &str,
) -> orch_link_contract::commands::AgentCredentialSet {
    serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": agent, "form": form,
        "credential": issued["credential"], "expires_at": issued["expires_at"],
        "upstream": null, "custodian": machine(p)
    }))
    .unwrap()
}

/// What a sealed credential says once the machine it is sealed to opens it.
fn opened(p: &ProviderRig, sealed: &Json, direction: Direction, form: &str) -> String {
    let bytes = open(
        &p.rig.identity.key,
        &exchange_info(direction, p.rig.provider, form),
        sealed["$sealed"].as_str().unwrap(),
    )
    .unwrap();
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn a_copy_is_issued_under_a_shared_lease_sealed_to_the_machine_it_goes_to_installed_there_and_renewed()
 {
    let p = rig().await;
    signed_in(&p).await;
    let second = new_id();
    start_harness(&p, second).await;
    let lease = open_lease(&p, "shared", "fake_token").await;

    let issued = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_token", &machine(&p)),
        )
        .await;
    assert_eq!(issued["form"], "fake_token");
    assert!(
        issued["expires_at"].is_string(),
        "a copy that lapses says when"
    );
    let copy = opened(&p, &issued["credential"], Direction::Issued, "fake_token");
    assert!(copy.starts_with("fake-token-"), "{copy}");
    assert!(
        !copy.contains("the-login-material-of-the-subscription"),
        "a copy that lapses never carries the secret it is renewed from"
    );
    p.rig
        .succeed(
            &harness_key(&p, second),
            &set_body(&p, second, &issued, "fake_token"),
        )
        .await;

    // Not again at once, whatever the engine asks.
    let too_soon = p
        .rig
        .send(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_token", &machine(&p)),
        )
        .await;
    assert!(
        matches!(&too_soon, orch_link::commands::Progress::Failed(error) if error.code == "conflict" && error.retryable),
        "{too_soon:?}"
    );
    p.rig.manual.advance(chrono::Duration::seconds(31));
    let renewed = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_token", &machine(&p)),
        )
        .await;
    let again = opened(&p, &renewed["credential"], Direction::Issued, "fake_token");
    assert_ne!(again, copy, "a renewal is a fresh copy");
    p.rig
        .succeed(
            &harness_key(&p, second),
            &set_body(&p, second, &renewed, "fake_token"),
        )
        .await;

    // A copy bound to one form does not open as another's.
    let swapped = p
        .rig
        .fail(
            &harness_key(&p, second),
            &set_body(&p, second, &renewed, "api_key"),
        )
        .await;
    assert_eq!(swapped, "internal");

    // Nothing is issued after the lease closes.
    p.close_lease(lease).await;
    p.rig.manual.advance(chrono::Duration::seconds(31));
    let closed = p
        .rig
        .fail(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_token", &machine(&p)),
        )
        .await;
    assert_eq!(closed, "lease_closed");
    // The value is in no event the system was told.
    let told: String = p
        .provider_events()
        .iter()
        .map(|event| event.body.to_string())
        .collect();
    assert!(!told.contains(&copy) && !told.contains(&again));
}

#[tokio::test]
async fn a_copy_is_issued_only_in_the_lease_form_to_its_recipient_and_by_a_form_the_harness_has() {
    let p = rig().await;
    signed_in(&p).await;
    let lease = open_lease(&p, "shared", "fake_token").await;

    let other_form = p
        .rig
        .fail(
            &p.provider_key(),
            &issue_body(&p, lease, "api_key", &machine(&p)),
        )
        .await;
    assert_eq!(other_form, "conflict", "the lease names fake_token");
    let a_stranger = json!({
        "machine_id": new_id(),
        "public_key": RecipientKey::from_certificate(&p.rig.identity.certificate).to_text()
    });
    let other_machine = p
        .rig
        .fail(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_token", &a_stranger),
        )
        .await;
    assert_eq!(
        other_machine, "conflict",
        "the lease names the machine the copy may go to"
    );
    let unknown = p
        .rig
        .fail(
            &p.provider_key(),
            &issue_body(&p, lease, "no_such_form", &machine(&p)),
        )
        .await;
    assert_eq!(unknown, "unsupported");
    // A lease is for the provider that it names.
    let mut elsewhere = issue_body(&p, lease, "fake_token", &machine(&p));
    elsewhere.provider_id = new_id();
    assert_eq!(p.rig.fail(&p.provider_key(), &elsewhere).await, "conflict");
    // No lease at all.
    assert_eq!(
        p.rig
            .fail(
                &p.provider_key(),
                &issue_body(&p, new_id(), "fake_token", &machine(&p))
            )
            .await,
        "lease_closed"
    );
}

#[tokio::test]
async fn the_copy_comes_from_the_process_that_is_signed_in_whatever_other_processes_the_provider_has()
 {
    // Two processes for one provider: one that sorts before the signed-in one, and has no login; one that
    // sorts after it. The copy is the signed-in one's however the machine happens to list them.
    let p = rig().await;
    start_harness(&p, Id::from_u128(1)).await;
    signed_in(&p).await;
    start_harness(&p, Id::from_u128(u128::MAX)).await;
    let lease = open_lease(&p, "shared", "fake_token").await;
    let issued = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_token", &machine(&p)),
        )
        .await;
    let copy = opened(&p, &issued["credential"], Direction::Issued, "fake_token");
    assert!(copy.starts_with("fake-token-"), "{copy}");

    // Models and state are asked of the same one.
    let refreshed = p
        .rig
        .succeed(
            &p.provider_key(),
            &serde_json::from_value::<orch_link_contract::commands::ProviderRefresh>(
                json!({ "provider_id": p.rig.provider }),
            )
            .unwrap(),
        )
        .await;
    assert_eq!(refreshed["status"], "ready", "{refreshed}");
}

#[tokio::test]
async fn nothing_is_issued_when_none_of_the_processes_of_the_provider_is_signed_in() {
    let p = rig().await;
    start_harness(&p, p.rig.agent).await;
    start_harness(&p, new_id()).await;
    let lease = open_lease(&p, "shared", "fake_token").await;
    let refused = p
        .rig
        .send(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_token", &machine(&p)),
        )
        .await;
    assert!(
        matches!(&refused, orch_link::commands::Progress::Failed(error) if error.code == "login_required"),
        "{refused:?}"
    );
    let state = p
        .rig
        .succeed(
            &p.provider_key(),
            &serde_json::from_value::<orch_link_contract::commands::ProviderRefresh>(
                json!({ "provider_id": p.rig.provider }),
            )
            .unwrap(),
        )
        .await;
    assert_eq!(state["status"], "needs_setup", "{state}");
}

#[tokio::test]
async fn a_login_moves_only_under_a_handover_lease_and_a_copy_only_under_a_shared_one() {
    let p = rig().await;
    signed_in(&p).await;
    let shared = open_lease(&p, "shared", "fake_login").await;
    assert_eq!(
        p.rig
            .fail(
                &p.provider_key(),
                &issue_body(&p, shared, "fake_login", &machine(&p))
            )
            .await,
        "conflict",
        "a login that moves is not a copy"
    );
    let handover = open_lease(&p, "handover", "fake_token").await;
    assert_eq!(
        p.rig
            .fail(
                &p.provider_key(),
                &issue_body(&p, handover, "fake_token", &machine(&p))
            )
            .await,
        "conflict",
        "a copy needs no hand-over"
    );
}

#[tokio::test]
async fn a_login_is_handed_over_given_back_and_taken_home() {
    let p = rig().await;
    signed_in(&p).await;
    let second = new_id();
    start_harness(&p, second).await;
    let lease = open_lease(&p, "handover", "fake_login").await;

    let handed = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "fake_login", &machine(&p)),
        )
        .await;
    let login = opened(&p, &handed["credential"], Direction::Issued, "fake_login");
    assert_eq!(login, "the-login-material-of-the-subscription");
    p.rig
        .succeed(
            &harness_key(&p, second),
            &set_body(&p, second, &handed, "fake_login"),
        )
        .await;

    // While it is away the harness at home cannot use it, and nothing more is issued.
    let second_lease = open_lease(&p, "handover", "fake_login").await;
    assert_eq!(
        p.rig
            .fail(
                &p.provider_key(),
                &issue_body(&p, second_lease, "fake_login", &machine(&p))
            )
            .await,
        "credential_unusable"
    );

    // It comes back, sealed to the machine that keeps it, and is taken home.
    let returned = p
        .rig
        .succeed(
            &harness_key(&p, second),
            &serde_json::from_value::<orch_link_contract::commands::AgentCredentialReturn>(json!({
                "provider_id": p.rig.provider, "agent_id": second, "custodian": machine(&p)
            }))
            .unwrap(),
        )
        .await;
    assert_eq!(returned["form"], "fake_login");
    let back = opened(
        &p,
        &returned["credential"],
        Direction::Returned,
        "fake_login",
    );
    assert!(
        back.starts_with("the-login-material-of-the-subscription"),
        "{back}"
    );
    let accept: orch_link_contract::commands::ProviderCredentialAccept =
        serde_json::from_value(json!({
            "provider_id": p.rig.provider, "form": "fake_login",
            "credential": returned["credential"], "expires_at": returned["expires_at"]
        }))
        .unwrap();
    p.rig.succeed(&p.provider_key(), &accept).await;

    // At home again, it can be lent again.
    p.rig.manual.advance(chrono::Duration::seconds(31));
    let lent = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, second_lease, "fake_login", &machine(&p)),
        )
        .await;
    assert_eq!(lent["form"], "fake_login");
    // A credential that went out does not open as one that came back.
    let mismatched: orch_link_contract::commands::ProviderCredentialAccept =
        serde_json::from_value(json!({
            "provider_id": p.rig.provider, "form": "fake_login",
            "credential": lent["credential"], "expires_at": null
        }))
        .unwrap();
    assert_eq!(p.rig.fail(&p.provider_key(), &mismatched).await, "internal");
}

#[cfg(unix)]
#[tokio::test]
async fn a_claude_subscription_login_passes_through_the_connector_and_returns_its_renewal() {
    use common::model::Options;
    use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    // This stand-in only answers the adapter's version question; it never runs a vendor program.
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("claude");
    std::fs::write(&program, "#!/bin/sh\nprintf '2.1.312 (Claude Code)\\n'\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let configured = program.display().to_string();
    let options = Options {
        adapters: Some(Box::new(|_, _| {
            let mut registry = AdapterRegistry::new();
            registry
                .register(AdapterRegistration {
                    kind: "claude-code",
                    adapter_version: orch_adapter_claude_code::config::ADAPTER_VERSION,
                    builtin: true,
                    needs_binary: true,
                    home_rule: HomeRule::Connector,
                    factory: Arc::new(|clock| {
                        Box::new(orch_adapter_claude_code::ClaudeCodeAdapter::new(clock))
                    }),
                })
                .unwrap();
            registry.program_names("claude-code", &["claude"]);
            registry
        })),
        ..Options::default()
    };
    let p = ProviderRig::build(options, move |rig, _| {
        rig.config.agent.harnesses = vec![orch_config::connector::AgentHarness {
            kind: "claude-code".into(),
            path: configured,
        }];
        rig.config.provider.arrangements = vec![
            "separated".into(),
            "credential_shared".into(),
            "handed_over".into(),
        ];
    })
    .await;
    let start = |agent: Id, home: &str| {
        serde_json::from_value::<orch_link_contract::commands::AgentStart>(json!({
            "provider_id":p.rig.provider,"agent_id":agent,"harness_kind":"claude-code",
            "profile":{"harness_version":null,"config":{},"env":{}},"home":home
        }))
        .unwrap()
    };
    p.rig
        .succeed(&harness_key(&p, p.rig.agent), &start(p.rig.agent, "owner"))
        .await;
    let owner = p.rig.data.path().join("owner").join(".credentials.json");
    let login = json!({"claudeAiOauth":{"accessToken":"synthetic-access","refreshToken":"synthetic-refresh","expiresAt":1}});
    std::fs::write(&owner, login.to_string()).unwrap();
    let lease = open_lease(&p, "handover", "login").await;
    let issued = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "login", &machine(&p)),
        )
        .await;
    assert!(!owner.exists());
    assert!(
        opened(&p, &issued["credential"], Direction::Issued, "login").contains("synthetic-refresh")
    );

    let away_id = new_id();
    p.rig
        .succeed(&harness_key(&p, away_id), &start(away_id, "away"))
        .await;
    p.rig
        .succeed(
            &harness_key(&p, away_id),
            &set_body(&p, away_id, &issued, "login"),
        )
        .await;
    let away = p.rig.data.path().join("away").join(".credentials.json");
    let renewed = json!({"claudeAiOauth":{"accessToken":"renewed-access","refreshToken":"renewed-refresh","expiresAt":2}});
    assert_eq!(
        serde_json::from_slice::<Json>(&std::fs::read(&away).unwrap()).unwrap(),
        login
    );
    std::fs::write(&away, renewed.to_string()).unwrap();
    let returned = p
        .rig
        .succeed(
            &harness_key(&p, away_id),
            &serde_json::from_value::<orch_link_contract::commands::AgentCredentialReturn>(json!({
                "provider_id":p.rig.provider,"agent_id":away_id,"custodian":machine(&p)
            }))
            .unwrap(),
        )
        .await;
    assert!(!away.exists());
    let accept = serde_json::from_value::<orch_link_contract::commands::ProviderCredentialAccept>(json!({
        "provider_id":p.rig.provider,"form":"login","credential":returned["credential"],"expires_at":returned["expires_at"]
    })).unwrap();
    p.rig.succeed(&p.provider_key(), &accept).await;
    assert_eq!(
        serde_json::from_slice::<Json>(&std::fs::read(owner).unwrap()).unwrap(),
        renewed
    );
}

#[tokio::test]
async fn the_machine_of_an_owner_who_allows_neither_refuses_to_open_the_lease() {
    let p = ProviderRig::new().await;
    signed_in(&p).await;
    for (mode, form) in [("shared", "fake_token"), ("handover", "fake_login")] {
        let (_, body) = lease_body(&p, mode, form, &machine(&p));
        let body: orch_link_contract::commands::LeaseOpen = serde_json::from_value(body).unwrap();
        assert_eq!(
            p.rig.fail(&p.provider_key(), &body).await,
            "policy.arrangement_not_allowed",
            "{mode}"
        );
    }
}

#[tokio::test]
async fn the_key_of_an_api_provider_is_issued_as_it_is_sealed_to_the_machine_that_asks() {
    let p = rig().await;
    p.start_responses_provider().await;
    let lease = open_lease(&p, "shared", "api_key").await;
    let issued = p
        .rig
        .succeed(
            &p.provider_key(),
            &issue_body(&p, lease, "api_key", &machine(&p)),
        )
        .await;
    assert_eq!(issued["form"], "api_key");
    assert_eq!(
        issued["expires_at"],
        json!(null),
        "a key lasts until its owner rotates it"
    );
    assert_eq!(
        opened(&p, &issued["credential"], Direction::Issued, "api_key"),
        p.key
    );
    assert!(
        !issued.to_string().contains(&p.key),
        "the key travels sealed and no other way"
    );
    // A form the protocol does not issue.
    let lease = open_lease(&p, "shared", "fake_token").await;
    assert_eq!(
        p.rig
            .fail(
                &p.provider_key(),
                &issue_body(&p, lease, "fake_token", &machine(&p))
            )
            .await,
        "unsupported"
    );
    let _ = RESPONSES;
}
