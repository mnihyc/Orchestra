//! Who is let in: a machine that was revoked, one the system does not know, one that shows another certificate, and what reaches the internal listener.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Enrolled, PATIENCE, Secrets, World, eventually, post_json, try_request};
use orch_db::audit::{Actor, Origin};
use orch_db::queries::resources;
use orch_link::machine::LinkStatus;
use orch_link_contract::commands::{CommandKind, SerialKey, SessionClose};
use orch_link_contract::enrollment::{ENROLL_PATH, Trust};
use orch_link_contract::version::RejectCode;
use orch_pki::ca::Holder;
use orch_pki::certificates::ServiceName;
use orch_pki::enrollment::{MachineIdentity, issue_token, token_digest};
use orch_pki::verification::{ServerTrust, pinned_client_config};
use orch_types::{Time, new_id};
use std::sync::Arc;
use std::time::Duration;

async fn refused_with(machine: &common::Machine) -> RejectCode {
    let mut status = machine.link.status();
    let waited = tokio::time::timeout(PATIENCE, async {
        loop {
            if let LinkStatus::Refused { code, .. } = &*status.borrow_and_update() {
                return *code;
            }
            status.changed().await.unwrap();
        }
    })
    .await;
    waited.expect("the machine to be refused")
}

#[tokio::test]
async fn a_revoked_machine_loses_its_session_at_once_and_is_refused_when_it_comes_back() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    machine.handler.hold(CommandKind::SessionClose);
    let (thread, command) = (new_id(), new_id());
    let dispatcher = world.api.link.dispatcher();
    dispatcher
        .issue_as(
            command,
            machine.id,
            &SerialKey::Thread(thread),
            Time::new(chrono::Utc::now() + chrono::Duration::minutes(5)),
            &SessionClose { thread_id: thread },
        )
        .await
        .unwrap();
    tokio::time::timeout(PATIENCE, machine.handler.started(command))
        .await
        .unwrap();

    let changed = world
        .api
        .link
        .revoke_machine(machine.id, Actor::User(world.user), &Origin::default())
        .await
        .unwrap();
    assert!(changed);

    // The session is dropped at once, and what was asked of the machine is called off.
    eventually("the gateway to let the session go", || {
        let gateway = world.gateway.gateway.clone();
        async move { gateway.sessions_held() == 0 }
    })
    .await;
    assert!(!world.api.link.is_online(machine.id));
    assert_eq!(
        dispatcher.wait(command).await.unwrap(),
        orch_api_core::dispatch::CommandOutcome::Cancelled
    );

    // When the machine comes back it is told why not.
    assert_eq!(refused_with(&machine).await, RejectCode::Revoked);
    let (status, audited): (String, i64) = {
        let mut conn = world.db.acquire().await.unwrap();
        let row = resources::machine(&mut conn, machine.id)
            .await
            .unwrap()
            .unwrap();
        let audited = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'machine.revoked' AND object_id = $1",
        )
        .bind(machine.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        (row.status, audited)
    };
    assert_eq!(status, "revoked");
    assert_eq!(
        audited, 1,
        "the revocation left one line in the audit trail"
    );
    assert!(
        !world
            .api
            .link
            .revoke_machine(machine.id, Actor::User(world.user), &Origin::default())
            .await
            .unwrap(),
        "a machine is revoked once"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_certificate_of_the_system_for_a_machine_it_does_not_know_gets_no_session() {
    let mut world = World::start().await;
    let known = world.enroll("alice-laptop").await;
    let stranger = new_id();
    let authority = world.secrets.authority();
    let (key, certificate) = authority
        .issue_with_new_key(&Holder::Machine(stranger), 365, world.clock.as_ref())
        .unwrap();
    let unknown = Enrolled {
        identity: MachineIdentity {
            machine_id: stranger,
            key,
            certificate,
            authority: world.secrets.ca_certificate(),
        },
        reply: known.reply.clone(),
    };
    let machine = world.start_machine(&unknown, unknown.endpoints(Trust::Pinned));
    assert_eq!(refused_with(&machine).await, RejectCode::UnknownMachine);
    assert_eq!(world.gateway.gateway.sessions_held(), 0);
    world.stop().await;
}

#[tokio::test]
async fn a_machine_that_shows_another_certificate_than_the_one_it_is_known_by_is_refused() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    // The system's authority issues another certificate for the same machine,
    // outside any renewal: it is not the one the machine is known by.
    let authority = world.secrets.authority();
    let (key, certificate) = authority
        .issue_with_new_key(
            &Holder::Machine(enrolled.identity.machine_id),
            365,
            world.clock.as_ref(),
        )
        .unwrap();
    let impostor = Enrolled {
        identity: MachineIdentity {
            machine_id: enrolled.identity.machine_id,
            key,
            certificate,
            authority: world.secrets.ca_certificate(),
        },
        reply: enrolled.reply.clone(),
    };
    let machine = world.start_machine(&impostor, impostor.endpoints(Trust::Pinned));
    assert_eq!(refused_with(&machine).await, RejectCode::Revoked);
    world.stop().await;
}

#[tokio::test]
async fn only_a_service_of_the_system_reaches_the_internal_listener() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let address: std::net::SocketAddr = format!("127.0.0.1:{}", world.api.port).parse().unwrap();
    let ca = world.secrets.ca_certificate();
    let client = |identity: Option<&orch_pki::verification::TlsIdentity>| {
        pinned_client_config(
            ServerTrust::Authority(Box::new(ca.clone())),
            Some(ServiceName::Api),
            identity,
            Arc::clone(&world.clock),
        )
        .unwrap()
    };
    let enroll = |tls| {
        try_request(
            address,
            tls,
            "POST",
            "/internal/link/enroll",
            Some("{}"),
            &[],
        )
    };

    // The gateway's own certificate gets as far as being told its request is no good.
    let gateway = world.secrets.identity(ServiceName::Gateway);
    let (status, _) = enroll(client(Some(&gateway)))
        .await
        .expect("the gateway is heard");
    assert_eq!(status, 400);

    // Another service of the system is let in, and is told that this is not for it.
    let worker = world.secrets.identity(ServiceName::Worker);
    let (status, _) = enroll(client(Some(&worker)))
        .await
        .expect("the worker is heard");
    assert_eq!(status, 403);

    // A certificate of another authority is not let in, even for a gateway.
    let strangers = Secrets::stranger(world.clock.as_ref());
    assert!(
        enroll(client(Some(&strangers))).await.is_err(),
        "another authority's gateway"
    );

    // A machine of the system, with a good certificate, is not a service.
    assert!(
        enroll(client(Some(&enrolled.identity.tls().unwrap())))
            .await
            .is_err(),
        "a machine"
    );

    // And nobody who shows nothing.
    assert!(enroll(client(None)).await.is_err(), "nobody");
    world.stop().await;
}

#[tokio::test]
async fn a_token_is_good_once_and_only_while_it_lasts_and_a_bad_request_does_not_use_it_up() {
    let mut world = World::start().await;

    // A request that is malformed is refused and leaves the token as it was.
    let token = world.token().await;
    let parsed: orch_link_contract::enrollment::EnrollmentToken = token.parse().unwrap();
    let tls = pinned_client_config(
        ServerTrust::Pin(*parsed.pin()),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&world.clock),
    )
    .unwrap();
    let bad = serde_json::json!({
        "token": token, "csr_pem": "not a request", "name": "alice-laptop",
        "platform": { "os": "linux", "arch": "x86_64" }, "connector_version": "0.1.0",
        "protocol_versions": [1], "roles": ["agent"]
    });
    let (status, _) = post_json(
        world.gateway.pinned,
        tls,
        ENROLL_PATH,
        &bad.to_string(),
        &[],
    )
    .await;
    assert_eq!(status, 400);
    let first = world.enroll_with(&token, "alice-laptop").await;
    assert!(
        first.is_ok(),
        "the token was not used up: {:?}",
        first.err()
    );

    // Used, it is gone.
    let second = world.enroll_with(&token, "again").await;
    assert_eq!(second.err().map(|(status, _)| status), Some(410));

    // One that has run out is gone as well.
    let authority = world.secrets.authority();
    let issued = issue_token(&authority, Duration::ZERO, world.clock.as_ref());
    let row = resources::TokenRow {
        token_hash: hex::decode(token_digest(issued.token.secret())).unwrap(),
        created_by: world.user,
        purpose: "connected".to_owned(),
        org_id: None,
        name_hint: None,
        expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
        used_at: None,
        machine_id: None,
        again_machine_id: None,
    };
    {
        let mut conn = world.db.acquire().await.unwrap();
        resources::insert_enrollment_token(&mut conn, &row)
            .await
            .unwrap();
    }
    let late = world.enroll_with(&issued.token.to_string(), "late").await;
    assert_eq!(late.err().map(|(status, _)| status), Some(410));

    // Nobody who was refused left a machine behind.
    let machines: i64 = sqlx::query_scalar("SELECT count(*) FROM machines")
        .fetch_one(world.db.pool())
        .await
        .unwrap();
    assert_eq!(machines, 1);
    world.stop().await;
}
