//! The existing sealed issue/accept channel supports one audited owner-job static copy without a turn lease.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod common;
use common::{
    Rig,
    model::{ProviderRig, RESPONSES},
};
use orch_connector_core::{credentials::SecretStore, roles::Keys};
use orch_link_contract::commands::{
    MachineKey, PROVIDER_COPY_FORM, ProviderCredentialAccept, ProviderCredentialCopy,
    ProviderCredentialIssue, ProviderStart, SealedCredential, SerialKey,
};
use orch_pki::sealing::RecipientKey;
use orch_types::{Json, new_id};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn a_static_owner_copy_is_sealed_to_its_actual_new_provider_and_issues_and_accepts_once_without_a_lease()
 {
    let mut source = ProviderRig::new().await;
    let mut target = Rig::with_roles(vec![orch_link_contract::session::Role::Provider]);
    target.config.provider.upstreams = vec![source.mock.responses_base_url()];
    // Advertising a role does not install its protocol/credential keeper.
    target.plug = Some(Arc::new(|builder, roles, data, key, clock| {
        let parts = orch_connector_core::roles::provider::ProviderParts::new(
            common::model::protocols(),
            data,
            key,
            clock,
        )
        .unwrap();
        orch_connector_core::roles::provider::plug_in(builder, roles, parts)
    }));
    target.start().await;
    let command = new_id();
    let header = "synthetic-owned-header";
    let body:ProviderStart=serde_json::from_value(json!({
        "provider_id":source.rig.provider,"protocols":[RESPONSES],"upstreams":[{"protocol":RESPONSES,"base_url":source.mock.responses_base_url(),"options":{"secret_headers":["x-owned-header"]}}],
        "credential":source.sealed_key_for(command,&source.key),"secrets":{"x-owned-header":source.sealed_key_for(command,header)}
    })).unwrap();
    let started = source
        .command_with_id(command, &source.provider_key(), &body)
        .await;
    assert!(matches!(
        started,
        orch_link::commands::Progress::Succeeded(_)
    ));
    let purpose = ProviderCredentialCopy {
        job_id: new_id(),
        source_provider_id: source.rig.provider,
        target_provider_id: target.provider,
        headers: vec!["x-owned-header".into()],
    };
    let recipient = MachineKey {
        machine_id: target.machine_id,
        public_key: RecipientKey::from_certificate(&target.identity.certificate).to_text(),
    };
    let issue = ProviderCredentialIssue {
        lease_id: None,
        copy: Some(purpose.clone()),
        provider_id: source.rig.provider,
        form: PROVIDER_COPY_FORM.into(),
        recipient: recipient.clone(),
    };
    let first = source.rig.succeed(&source.provider_key(), &issue).await;
    assert!(!first.to_string().contains(&source.key));
    assert!(!first.to_string().contains(header));
    let second = source.rig.succeed(&source.provider_key(), &issue).await;
    assert_eq!(
        first, second,
        "even a different command ID cannot issue a second sealed value for this copy job"
    );
    let sealed: SealedCredential = serde_json::from_value(first).unwrap();
    let accept = ProviderCredentialAccept {
        provider_id: target.provider,
        copy: Some(purpose.clone()),
        form: sealed.form.clone(),
        credential: sealed.credential.clone(),
        expires_at: sealed.expires_at,
    };
    let key = SerialKey::Provider(target.provider);
    target.succeed(&key, &accept).await;
    let store = SecretStore::open(
        target.data.path(),
        Arc::new(Keys::new(target.identity.key.duplicate().unwrap())),
    )
    .unwrap();
    let name = format!("provider-{}.copy", target.provider);
    let before = store.get(&name).unwrap().unwrap();
    let values: Json = serde_json::from_str(&before).unwrap();
    assert_eq!(values["values"]["key"], source.key);
    assert_eq!(values["values"]["headers"]["x-owned-header"], header);
    target.succeed(&key, &accept).await;
    assert_eq!(
        store.get(&name).unwrap().unwrap(),
        before,
        "an accepted receipt never rewrites or restores local values"
    );
    let mut changed = purpose.clone();
    changed.job_id = new_id();
    let wrong = ProviderCredentialAccept {
        copy: Some(changed),
        ..accept.clone()
    };
    assert!(!target.fail(&key, &wrong).await.is_empty());
    let mut wrong_recipient = issue.clone();
    wrong_recipient.recipient.machine_id = new_id();
    assert!(
        !source
            .rig
            .fail(&source.provider_key(), &wrong_recipient)
            .await
            .is_empty()
    );
    let mut mixed = issue.clone();
    mixed.lease_id = Some(new_id());
    assert!(
        !source
            .rig
            .fail(&source.provider_key(), &mixed)
            .await
            .is_empty(),
        "an owner copy cannot be borrowed as a turn lease"
    );
    let start:ProviderStart=serde_json::from_value(json!({"provider_id":target.provider,"protocols":[RESPONSES],"upstreams":[{"protocol":RESPONSES,"base_url":source.mock.responses_base_url(),"options":{"secret_headers":["x-owned-header"]}}],"credential":null,"secrets":{}})).unwrap();
    target.succeed(&key, &start).await;
    target.kill().await;
    source.rig.kill().await;
}
