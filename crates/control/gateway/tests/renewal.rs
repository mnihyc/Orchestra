//! A certificate is renewed over a new key, by either port, and both the old and the new are the machine's until a session shows the new.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration as Days, Utc};
use common::{Enrolled, PATIENCE, World, post_json, try_request};
use orch_db::queries::resources;
use orch_link::machine::LinkStatus;
use orch_link_contract::enrollment::{RENEW_PATH, RenewReply, RenewRequest, Trust};
use orch_link_contract::version::RejectCode;
use orch_pki::ca::Holder;
use orch_pki::certificates::{Certificate, ServiceName};
use orch_pki::enrollment::MachineIdentity;
use orch_pki::renewal;
use orch_pki::verification::{
    ServerTrust, TlsIdentity, pinned_client_config, public_client_config, root_store,
};
use orch_types::{Id, ManualClock};
use std::net::SocketAddr;
use std::sync::Arc;

async fn renew_pinned(
    world: &World,
    enrolled: &Enrolled,
) -> (u16, Vec<u8>, orch_pki::certificates::LeafKey) {
    let (key, csr_pem) = renewal::start().unwrap();
    let (status, body) = post_json(
        world.gateway.pinned,
        world.pinned_config(enrolled),
        RENEW_PATH,
        &serde_json::to_string(&RenewRequest { csr_pem }).unwrap(),
        &[],
    )
    .await;
    (status, body, key)
}

fn renewed(
    world: &World,
    enrolled: &Enrolled,
    key: orch_pki::certificates::LeafKey,
    body: &[u8],
) -> Enrolled {
    let reply: RenewReply = serde_json::from_slice(body).unwrap();
    Enrolled {
        identity: MachineIdentity {
            machine_id: enrolled.identity.machine_id,
            key,
            certificate: Certificate::from_pem(&reply.certificate_pem).unwrap(),
            authority: world.secrets.ca_certificate(),
        },
        reply: enrolled.reply.clone(),
    }
}

async fn refused_with(machine: &common::Machine) -> RejectCode {
    let mut status = machine.link.status();
    tokio::time::timeout(PATIENCE, async {
        loop {
            if let LinkStatus::Refused { code, .. } = &*status.borrow_and_update() {
                return *code;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .expect("the machine to be refused")
}

async fn row(world: &World, id: Id) -> resources::MachineRow {
    let mut conn = world.db.acquire().await.unwrap();
    resources::machine(&mut conn, id).await.unwrap().unwrap()
}

#[tokio::test]
async fn a_renewed_certificate_is_known_beside_the_old_one_until_a_session_shows_it() {
    let mut world = World::start().await;
    let old = world.enroll("alice-laptop").await;
    let id = old.identity.machine_id;
    let (status, body, key) = renew_pinned(&world, &old).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let new = renewed(&world, &old, key, &body);
    assert_ne!(
        new.identity.certificate.fingerprint(),
        old.identity.certificate.fingerprint()
    );

    // Nothing has changed for the machine yet: it is known by the old one, and has the new one waiting.
    let before = row(&world, id).await;
    assert_eq!(
        hex::encode(&before.cert_fingerprint),
        old.identity.certificate.fingerprint()
    );
    assert_eq!(
        before.link().next.map(|next| next.fingerprint),
        Some(new.identity.certificate.fingerprint())
    );

    // A reply that never arrived costs the machine nothing: the old certificate still holds a session.
    let on_old = world.start_machine(&old, old.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, on_old.link.connected())
        .await
        .unwrap();
    on_old.link.shutdown().await;

    // The new one is shown, and from then on it is the machine's own.
    let on_new = world.start_machine(&new, new.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, on_new.link.connected())
        .await
        .unwrap();
    let after = row(&world, id).await;
    assert_eq!(
        hex::encode(&after.cert_fingerprint),
        new.identity.certificate.fingerprint()
    );
    assert_eq!(after.link().next, None);

    // And the old one is the machine's no more.
    on_new.link.shutdown().await;
    let on_old_again = world.start_machine(&old, old.endpoints(Trust::Pinned));
    assert_eq!(refused_with(&on_old_again).await, RejectCode::Revoked);
    let renewals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'machine.renewed' AND object_id = $1",
    )
    .bind(id)
    .fetch_one(world.db.pool())
    .await
    .unwrap();
    assert_eq!(renewals, 1);
    world.stop().await;
}

#[tokio::test]
async fn a_machine_behind_the_edge_renews_by_proving_it_holds_the_key_of_its_certificate() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let public = public_client_config(
        root_store(&[world.secrets.ca_certificate()]).unwrap(),
        Arc::clone(&world.clock),
    )
    .unwrap();
    let (_, csr_pem) = renewal::start().unwrap();
    let body = serde_json::to_string(&RenewRequest {
        csr_pem: csr_pem.clone(),
    })
    .unwrap();
    let proof = renewal::sign_request(
        &enrolled.identity.key,
        &enrolled.identity.certificate,
        enrolled.identity.machine_id,
        &csr_pem,
        "127.0.0.1",
        world.clock.as_ref(),
    );
    let headers = |proof: &renewal::RenewalProof| {
        vec![
            (
                "x-orch-renew-certificate".to_owned(),
                URL_SAFE_NO_PAD.encode(proof.certificate_pem.as_bytes()),
            ),
            ("x-orch-renew-signature".to_owned(), proof.signature.clone()),
            ("x-orch-renew-sent-at".to_owned(), proof.sent_at.to_string()),
        ]
    };
    let send = |proof: &renewal::RenewalProof| {
        let headers = headers(proof);
        let (public, body, edge) = (Arc::clone(&public), body.clone(), world.gateway.edge);
        async move {
            let borrowed: Vec<(&str, &str)> = headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            post_json(edge, public, RENEW_PATH, &body, &borrowed).await
        }
    };

    // A proof that is another key's is no proof.
    let (other_key, _) = renewal::start().unwrap();
    let forged = renewal::sign_request(
        &other_key,
        &enrolled.identity.certificate,
        enrolled.identity.machine_id,
        &csr_pem,
        "127.0.0.1",
        world.clock.as_ref(),
    );
    assert_eq!(send(&forged).await.0, 403);
    // A proof for another host is no proof here.
    let elsewhere = renewal::sign_request(
        &enrolled.identity.key,
        &enrolled.identity.certificate,
        enrolled.identity.machine_id,
        &csr_pem,
        "evil.example.com",
        world.clock.as_ref(),
    );
    assert_eq!(send(&elsewhere).await.0, 403);
    // Nothing at all is not one either.
    let (status, _) = post_json(
        world.gateway.edge,
        Arc::clone(&public),
        RENEW_PATH,
        &body,
        &[],
    )
    .await;
    assert_eq!(status, 403);
    // The right one is.
    let (status, reply) = send(&proof).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&reply));
    let reply: RenewReply = serde_json::from_slice(&reply).unwrap();
    assert!(
        Certificate::from_pem(&reply.certificate_pem)
            .unwrap()
            .machine_id()
            .is_some()
    );
    world.stop().await;
}

/// A certificate for the machine that stopped being good `days` days ago, and that the system knows the machine by.
async fn expired_certificate(world: &World, enrolled: &Enrolled, days: i64) -> Enrolled {
    let long_ago = ManualClock::new(Utc::now() - Days::days(365 + days));
    let (key, certificate) = world
        .secrets
        .authority()
        .issue_with_new_key(
            &Holder::Machine(enrolled.identity.machine_id),
            365,
            &long_ago,
        )
        .unwrap();
    sqlx::query("UPDATE machines SET cert_fingerprint = $2 WHERE id = $1")
        .bind(enrolled.identity.machine_id)
        .bind(hex::decode(certificate.fingerprint()).unwrap())
        .execute(world.db.pool())
        .await
        .unwrap();
    Enrolled {
        identity: MachineIdentity {
            machine_id: enrolled.identity.machine_id,
            key,
            certificate,
            authority: world.secrets.ca_certificate(),
        },
        reply: enrolled.reply.clone(),
    }
}

#[tokio::test]
async fn a_certificate_that_ran_out_gets_no_session_but_is_renewed_within_the_grace_and_not_beyond_it()
 {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;

    // Out for thirty-five days: no session, but a renewal is heard.
    let recent = expired_certificate(&world, &enrolled, 35).await;
    let machine = world.start_machine(&recent, recent.endpoints(Trust::Pinned));
    assert_eq!(refused_with(&machine).await, RejectCode::CertificateExpired);
    machine.link.shutdown().await;
    let (status, body, key) = renew_pinned(&world, &recent).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let fresh = renewed(&world, &recent, key, &body);
    let back = world.start_machine(&fresh, fresh.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, back.link.connected())
        .await
        .unwrap();
    back.link.shutdown().await;

    // Out for a hundred days, beyond the ninety that are forgiven: not even the connection is let through.
    let stale = expired_certificate(&world, &fresh, 100).await;
    let (_, csr_pem) = renewal::start().unwrap();
    let attempt = try_request(
        world.gateway.pinned,
        world.pinned_config(&stale),
        "POST",
        RENEW_PATH,
        Some(&serde_json::to_string(&RenewRequest { csr_pem }).unwrap()),
        &[],
    )
    .await;
    assert!(
        attempt.is_err(),
        "a certificate that ran out so long ago is not heard: {attempt:?}"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_service_renews_its_certificate_over_a_new_key_and_the_new_one_is_let_in() {
    let mut world = World::start().await;
    let address: SocketAddr = format!("127.0.0.1:{}", world.api.port).parse().unwrap();
    let client = |identity: &TlsIdentity| {
        pinned_client_config(
            ServerTrust::Authority(Box::new(world.secrets.ca_certificate())),
            Some(ServiceName::Api),
            Some(identity),
            Arc::clone(&world.clock),
        )
        .unwrap()
    };
    let gateway = world.secrets.identity(ServiceName::Gateway);

    let (key, csr_pem) = renewal::start().unwrap();
    let (status, body) = post_json(
        address,
        client(&gateway),
        "/internal/pki/renew",
        &serde_json::to_string(&RenewRequest { csr_pem }).unwrap(),
        &[],
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let reply: RenewReply = serde_json::from_slice(&body).unwrap();
    let renewed = Certificate::from_pem(&reply.certificate_pem).unwrap();
    assert!(renewed.holds(&key), "the certificate is for the new key");
    assert_eq!(
        renewed.identities(),
        gateway.chain[0].identities(),
        "and still names the gateway"
    );

    // The renewed certificate is let in as the old one was: the request below is empty, which
    // the internal listener can only say once the connection was accepted.
    let identity = TlsIdentity {
        chain: vec![renewed],
        key,
    };
    let (status, _) = post_json(
        address,
        client(&identity),
        "/internal/link/enroll",
        "{}",
        &[],
    )
    .await;
    assert_eq!(status, 400);
    world.stop().await;
}

#[tokio::test]
async fn a_gateway_whose_certificate_is_two_thirds_through_renews_it_through_the_system_when_it_starts()
 {
    let mut world = World::start().await;
    let due = world.gateway_identity_issued(250);
    let first = due.chain[0].clone();
    world.restart_gateway_with(due).await;

    // The gateway looks when it starts: its certificate is due, so it asks the system for another.
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let shown = loop {
        let shown = world.gateway.gateway.certificate().unwrap();
        if shown != first {
            break shown;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the gateway did not renew its certificate"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(
        shown.identities(),
        first.identities(),
        "it still names the gateway"
    );
    assert!(shown.not_after() > first.not_after(), "and lasts longer");
    assert_eq!(shown.service(), Some(ServiceName::Gateway));

    // Machines go on knowing it: one enrolls through the pinned port, which shows the renewed certificate.
    world.enroll("after-the-renewal").await;
    world.stop().await;
}
