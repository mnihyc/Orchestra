//! Whole sign-ins through the kind against the signed local provider: what a provider signs is read, every algorithm that is taken verifies, a rotated key is followed, and every fault a provider can make gives no identity.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_identity_contract::provider::{
    CompleteRequest, IdentityProviderKind, PendingSignIn, ProviderSettings, StartRequest,
};
use orch_identity_oidc::OidcKind;
use orch_identity_oidc::authorization::challenge;
use orch_plugin_contract::Secret;
use orch_testkit::clock::{self, Clock, ManualClock};
use orch_testkit::oidc::{
    Alg, CLIENT_ID, CLIENT_SECRET, Faults, OidcProvider, Person, ProviderConfig,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

const CALLBACK: &str = "https://orch.example.com/api/v1/auth/oidc/acme/callback";

struct Rig {
    provider: OidcProvider,
    kind: OidcKind,
    clock: ManualClock,
}

async fn rig(alg: Alg) -> Rig {
    let clock = clock::manual();
    let mut config = ProviderConfig::new(CALLBACK, Arc::new(clock.clone()));
    config.alg = alg;
    let provider = OidcProvider::start(config).await.unwrap();
    provider.sign_in_as(
        Person::new("248289761001")
            .with_email("dana@acme.example", true)
            .named("Dana Whit")
            .with_username("dana.whit"),
    );
    Rig {
        provider,
        kind: OidcKind::new().unwrap(),
        clock,
    }
}

impl Rig {
    fn settings(&self) -> ProviderSettings {
        serde_json::from_value(json!({
            "key": "acme", "name": "Acme", "issuer": self.provider.issuer(), "client_id": CLIENT_ID
        }))
        .unwrap()
    }

    /// Starts a sign-in, visits the provider as the browser would, and comes back with what the
    /// callback would be given: the pending sign-in and the code.
    async fn visit(&self) -> (PendingSignIn, String) {
        let started = self
            .kind
            .start(&StartRequest {
                settings: self.settings(),
                callback_url: CALLBACK.to_owned(),
                fresh: false,
            })
            .await
            .unwrap();
        let back = self
            .provider
            .authorize(&started.redirect_url)
            .await
            .unwrap();
        let query: HashMap<String, String> = url::Url::parse(&back)
            .unwrap()
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        assert!(back.starts_with(CALLBACK), "{back}");
        assert_eq!(query["state"], started.pending.state);
        (started.pending, query["code"].clone())
    }

    fn completion(&self, pending: PendingSignIn, code: String) -> CompleteRequest {
        CompleteRequest {
            settings: self.settings(),
            client_secret: Some(Secret::new(CLIENT_SECRET)),
            callback_url: CALLBACK.to_owned(),
            pending,
            code,
            now: self.clock.time(),
        }
    }
}

#[tokio::test]
async fn a_whole_sign_in_gives_the_identity_the_provider_signed_and_pkce_was_asked_with_s256() {
    let rig = rig(Alg::Rs256).await;
    let (pending, code) = rig.visit().await;
    let asked = rig.provider.asked();
    assert_eq!(asked[0]["code_challenge_method"], "S256");
    assert_eq!(
        asked[0]["code_challenge"],
        challenge(&pending.pkce_verifier)
    );
    assert_eq!(asked[0]["nonce"], pending.nonce);
    let identity = rig
        .kind
        .complete(&rig.completion(pending, code))
        .await
        .unwrap();
    assert_eq!(identity.subject, "248289761001");
    assert_eq!(identity.trusted_email(), Some("dana@acme.example"));
    assert_eq!(identity.name.as_deref(), Some("Dana Whit"));
    assert_eq!(identity.username.as_deref(), Some("dana.whit"));
    assert_eq!(
        identity.auth_time.map(|at| at.get().timestamp()),
        Some(rig.clock.now().timestamp())
    );
}

#[tokio::test]
async fn every_algorithm_a_provider_signs_with_and_that_is_taken_verifies() {
    for alg in [Alg::Ps256, Alg::Es256, Alg::Es384, Alg::EdDsa] {
        let rig = rig(alg).await;
        let (pending, code) = rig.visit().await;
        let identity = rig.kind.complete(&rig.completion(pending, code)).await;
        assert_eq!(identity.unwrap().subject, "248289761001", "{alg:?}");
    }
}

#[tokio::test]
async fn a_code_serves_once() {
    let rig = rig(Alg::Rs256).await;
    let (pending, code) = rig.visit().await;
    let request = rig.completion(pending, code);
    assert!(rig.kind.complete(&request).await.is_ok());
    assert!(rig.kind.complete(&request).await.is_err());
}

#[tokio::test]
async fn a_rotated_key_is_read_again_once_and_then_kept() {
    let rig = rig(Alg::Rs256).await;
    let (pending, code) = rig.visit().await;
    rig.kind
        .complete(&rig.completion(pending, code))
        .await
        .unwrap();
    assert_eq!(rig.provider.key_reads(), 1);
    let (pending, code) = rig.visit().await;
    rig.kind
        .complete(&rig.completion(pending, code))
        .await
        .unwrap();
    assert_eq!(rig.provider.key_reads(), 1, "the keys are kept");
    rig.provider.rotate_keys(false);
    let (pending, code) = rig.visit().await;
    rig.kind
        .complete(&rig.completion(pending, code))
        .await
        .unwrap();
    assert_eq!(
        rig.provider.key_reads(),
        2,
        "a key not kept is looked for again"
    );
    let (pending, code) = rig.visit().await;
    rig.kind
        .complete(&rig.completion(pending, code))
        .await
        .unwrap();
    assert_eq!(rig.provider.key_reads(), 2);
}

#[tokio::test]
async fn every_fault_of_a_provider_gives_no_identity() {
    let rig = rig(Alg::Rs256).await;
    let faults = [
        Faults {
            issuer: Some("http://127.0.0.1:1".to_owned()),
            ..Faults::default()
        },
        Faults {
            audience: Some(json!("another-client")),
            ..Faults::default()
        },
        Faults {
            audience: Some(json!([CLIENT_ID, "another-client"])),
            ..Faults::default()
        },
        Faults {
            nonce: Some("another-nonce".to_owned()),
            ..Faults::default()
        },
        Faults {
            lifetime: Some(-120),
            ..Faults::default()
        },
        Faults {
            issued_ago: 3_600,
            ..Faults::default()
        },
        Faults {
            unpublished_key: true,
            ..Faults::default()
        },
        Faults {
            unsigned: true,
            ..Faults::default()
        },
        Faults {
            refuse_codes: true,
            ..Faults::default()
        },
    ];
    for fault in faults {
        rig.provider.set_faults(fault.clone());
        let (pending, code) = rig.visit().await;
        let answer = rig.kind.complete(&rig.completion(pending, code)).await;
        assert!(answer.is_err(), "{fault:?} gave {answer:?}");
    }
    rig.provider.set_faults(Faults::default());
    let (pending, code) = rig.visit().await;
    assert!(
        rig.kind
            .complete(&rig.completion(pending, code))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_code_exchanged_with_another_verifier_or_another_secret_gives_nothing() {
    let rig = rig(Alg::Rs256).await;
    let (mut pending, code) = rig.visit().await;
    pending.pkce_verifier = "a-verifier-somebody-else-made-for-a-stolen-code".to_owned();
    assert!(
        rig.kind
            .complete(&rig.completion(pending, code))
            .await
            .is_err()
    );
    let (pending, code) = rig.visit().await;
    let mut request = rig.completion(pending, code);
    request.client_secret = Some(Secret::new("not-the-secret"));
    assert!(rig.kind.complete(&request).await.is_err());
}

#[tokio::test]
async fn the_profile_is_read_from_the_user_information_where_the_token_leaves_it_out() {
    let rig = rig(Alg::Es256).await;
    rig.provider.set_faults(Faults {
        profile_at_user_info: true,
        ..Faults::default()
    });
    let (pending, code) = rig.visit().await;
    let identity = rig
        .kind
        .complete(&rig.completion(pending, code))
        .await
        .unwrap();
    assert_eq!(identity.trusted_email(), Some("dana@acme.example"));
    assert_eq!(identity.name.as_deref(), Some("Dana Whit"));
}

#[tokio::test]
async fn a_test_of_the_provider_finds_its_issuer_and_counts_its_keys() {
    let rig = rig(Alg::Rs256).await;
    rig.provider.rotate_keys(true);
    let found = rig.kind.validate(&rig.settings()).await.unwrap();
    assert_eq!(found.issuer, rig.provider.issuer());
    assert_eq!(found.signing_keys, 2);
    assert_eq!(found.issuer_metadata()["signing_keys"], 2);
    let mut elsewhere = rig.settings();
    elsewhere.issuer = format!("{}/realms/other", rig.provider.issuer());
    let refused = rig.kind.validate(&elsewhere).await.unwrap_err();
    assert!(
        refused
            .detail
            .starts_with("Could not read the issuer's discovery document"),
        "{}",
        refused.detail
    );
}
