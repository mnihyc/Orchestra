//! Sign-in changes follow actual home ownership and the live machine policy, independently of placement.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::Rig;
use orch_connector_core::credentials::{Direction, exchange_info};
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    AgentCredentialSet, AgentStart, AgentStop, ProviderLoginStart, ProviderLogout, ProviderRefresh,
};
use orch_link_contract::policy::{LocalPolicy, PolicyDefaults, PolicyKey};
use orch_pki::sealing::{RecipientKey, seal};
use serde_json::json;

fn login(rig: &Rig) -> ProviderLoginStart {
    serde_json::from_value(
        json!({"provider_id":rig.provider,"agent_id":rig.agent,"method":"fake_import",
        "values":{"credential_text":"only-the-scripted-login"},"config":{}}),
    )
    .unwrap()
}

#[test]
fn signing_in_to_a_persons_home_is_one_delegatable_provider_policy_with_explicit_shipped_defaults()
{
    let personal = LocalPolicy::installed(PolicyDefaults::Cautious);
    let host = LocalPolicy::installed(PolicyDefaults::Managed);
    assert_eq!(personal.get(PolicyKey::ProviderSignIn), json!(false));
    assert_eq!(host.get(PolicyKey::ProviderSignIn), json!(true));
    assert_eq!(PolicyKey::ProviderSignIn.as_str(), "provider.signin");
    assert_eq!(
        PolicyKey::ProviderSignIn.bounds(),
        orch_link_contract::policy::Bounds::Machine
    );
    let mut changed = personal;
    changed
        .set(PolicyKey::ProviderSignIn, &json!(true))
        .unwrap();
    assert!(changed.provider_signin);
    assert!(
        changed
            .set(PolicyKey::ProviderSignIn, &json!("true"))
            .is_err()
    );
    assert!(
        changed.provider_signin,
        "an invalid change leaves the actual policy alone"
    );
}

#[tokio::test]
async fn a_connector_owned_home_uses_native_methods_and_a_persons_home_needs_the_machine_switch() {
    for (borrowed, allowed) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut rig = Rig::new();
        rig.config.provider.signin = allowed;
        let home = tempfile::tempdir().unwrap();
        rig.config.environment.roots = vec![home.path().display().to_string()];
        let attach = borrowed.then(
            || json!({"mode":"spawn","binary":null,"home":home.path().display().to_string()}),
        );
        rig.start().await;
        let start: AgentStart = serde_json::from_value(
            json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
            "profile":{"config":{},"env":{},"attach":attach},"home":null}),
        )
        .unwrap();
        rig.succeed(&rig.harness_key(), &start).await;
        let hosting = rig.connector.as_ref().unwrap().hosting().unwrap();
        assert_eq!(
            hosting
                .process(rig.provider, rig.agent)
                .unwrap()
                .home_owned(),
            !borrowed
        );
        let result = rig.send(&rig.harness_key(), &login(&rig)).await;
        if borrowed && !allowed {
            let Progress::Failed(error) = result else {
                panic!("the person's home must be left alone: {result:?}");
            };
            assert_eq!(error.code, "policy.signin_not_allowed");
            assert!(error.detail.contains("provider.signin"));
            let recipient = RecipientKey::from_certificate(&rig.identity.certificate);
            let copy:AgentCredentialSet=serde_json::from_value(json!({"provider_id":rig.provider,"agent_id":rig.agent,"form":"fake_token",
                "credential":{"$sealed":seal(&recipient,&exchange_info(Direction::Issued,rig.provider,"fake_token"),b"native-copy").unwrap()},
                "expires_at":null,"upstream":null,"custodian":{"machine_id":rig.machine_id,"public_key":recipient.to_text()}})).unwrap();
            assert_eq!(
                rig.fail(&rig.harness_key(), &copy).await,
                "policy.signin_not_allowed",
                "a credential copy also changes the person's native sign-in"
            );
            let renewed = rig
                .succeed(
                    &rig.harness_key(),
                    &ProviderRefresh {
                        provider_id: rig.provider,
                        retry: None,
                        renew: Some(true),
                        facts: None,
                    },
                )
                .await;
            assert_eq!(renewed["status"], "not_told");
            assert!(
                renewed["source"]["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("provider.signin")
            );
        } else {
            assert!(matches!(result, Progress::Succeeded(_)), "{result:?}");
            rig.succeed(
                &rig.harness_key(),
                &ProviderLogout {
                    provider_id: rig.provider,
                    agent_id: rig.agent,
                },
            )
            .await;
        }
        rig.kill().await;
    }
}

#[tokio::test]
async fn deletion_stops_the_harness_even_when_its_borrowed_home_refuses_native_sign_out() {
    for borrowed in [false, true] {
        let mut rig = Rig::new();
        rig.config.provider.signin = true;
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("personal-file"), "keep").unwrap();
        rig.config.environment.roots = vec![home.path().display().to_string()];
        rig.start().await;
        let attach = borrowed.then(
            || json!({"mode":"spawn","binary":null,"home":home.path().display().to_string()}),
        );
        let start: AgentStart = serde_json::from_value(
            json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
            "profile":{"config":{},"env":{},"attach":attach},"home":null}),
        )
        .unwrap();
        rig.succeed(&rig.harness_key(), &start).await;
        rig.succeed(&rig.harness_key(), &login(&rig)).await;
        let hosting = rig.connector.as_ref().unwrap().hosting().unwrap();
        hosting
            .services()
            .policy
            .set_here(PolicyKey::ProviderSignIn, &json!(false))
            .unwrap();
        let stopped = rig
            .send(
                &rig.harness_key(),
                &AgentStop {
                    provider_id: rig.provider,
                    agent_id: rig.agent,
                    logout: Some(true),
                },
            )
            .await;
        if borrowed {
            let Progress::Failed(error) = stopped else {
                panic!("the local ceiling must still refuse sign-out: {stopped:?}");
            };
            assert_eq!(error.code, "policy.signin_not_allowed");
        } else {
            assert!(matches!(stopped, Progress::Succeeded(_)), "{stopped:?}");
        }
        assert!(
            hosting.process(rig.provider, rig.agent).is_err(),
            "refusing sign-out never keeps a process running"
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("personal-file")).unwrap(),
            "keep"
        );
        rig.kill().await;
    }
}

#[tokio::test]
async fn taking_back_the_machine_switch_prevents_logout_and_renewal_without_hiding_read_only_facts()
{
    let mut rig = Rig::new();
    rig.config.provider.signin = true;
    let home = tempfile::tempdir().unwrap();
    rig.config.environment.roots = vec![home.path().display().to_string()];
    rig.start().await;
    let start:AgentStart=serde_json::from_value(json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
        "profile":{"config":{},"env":{},"attach":{"mode":"spawn","binary":null,"home":home.path().display().to_string()}},"home":null})).unwrap();
    rig.succeed(&rig.harness_key(), &start).await;
    rig.succeed(&rig.harness_key(), &login(&rig)).await;
    rig.connector
        .as_ref()
        .unwrap()
        .hosting()
        .unwrap()
        .services()
        .policy
        .set_here(PolicyKey::ProviderSignIn, &json!(false))
        .unwrap();
    let code = rig
        .fail(
            &rig.harness_key(),
            &ProviderLogout {
                provider_id: rig.provider,
                agent_id: rig.agent,
            },
        )
        .await;
    assert_eq!(code, "policy.signin_not_allowed");
    let read = rig
        .succeed(
            &rig.harness_key(),
            &ProviderRefresh {
                provider_id: rig.provider,
                retry: None,
                renew: None,
                facts: None,
            },
        )
        .await;
    assert_eq!(
        read["status"], "ready",
        "reading a sign-in never grants permission to replace it"
    );
    let renewed = rig
        .succeed(
            &rig.harness_key(),
            &ProviderRefresh {
                provider_id: rig.provider,
                retry: None,
                renew: Some(true),
                facts: None,
            },
        )
        .await;
    assert_eq!(renewed["status"], "not_told");
    rig.kill().await;
}
