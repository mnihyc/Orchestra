//! Native revocation packets cannot move between transactions or pass an unaccepted first effect.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use super::*;

use crate::support::World;

use orch_db::queries::link;

use orch_db::queries::link::NewCommand;

use orch_db::queries::resources::access_fixtures as fixtures;

use orch_types::new_id;

use serde_json::json;

use std::sync::atomic::AtomicUsize;

use std::sync::atomic::Ordering;
async fn machine(world: &World, owner: Id) -> (Id, Id) {
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, owner, "Native revocation operands")
        .await
        .unwrap();
    let command = new_id();
    link::insert_command(
        &mut conn,
        &NewCommand {
            id: command,
            machine_id: machine,
            kind: "machine.probe",
            body: &json!({}),
            serial_key: Some("machine"),
            deadline_at: world.now() + chrono::Duration::hours(1),
        },
        world.now(),
    )
    .await
    .unwrap();
    (machine, command)
}

async fn states(conn: &mut Conn, machine: Id, command: Id) -> (String, String, i64) {
    sqlx::query_as("SELECT m.status,c.state,(SELECT count(*) FROM audit_log WHERE object_id=m.id AND action='machine.revoked') FROM machines m JOIN commands c ON c.machine_id=m.id WHERE m.id=$1 AND c.id=$2")
        .bind(machine).bind(command).fetch_one(conn).await.unwrap()
}

#[cfg(any(test, feature = "test-support"))]
async fn rejected_revocation_cut_leaves_registration_commands_and_audit_unchanged() {
    let world = World::new().await;
    let owner = crate::places::person(&world, "native-owner").await;
    let (machine, command) = machine(&world, owner.id()).await;
    let mut tx = world.app.app.db.begin().await.unwrap();
    let prepared = CertificateService::prepare_revocation(&mut tx, machine)
        .await
        .unwrap();
    assert_eq!(
        states(&mut tx, machine, command).await,
        ("active".into(), "queued".into(), 0)
    );
    let cuts = AtomicUsize::new(0);
    let refused = CertificateService::apply_revocation(
        &mut tx,
        prepared,
        Actor::User(owner.id()),
        &Origin::default(),
        world.now(),
        || {
            cuts.fetch_add(1, Ordering::SeqCst);
            Err("the original caller is closed")
        },
    )
    .await;
    assert!(matches!(refused, Err("the original caller is closed")));
    assert_eq!(cuts.load(Ordering::SeqCst), 1);
    assert_eq!(
        states(&mut tx, machine, command).await,
        ("active".into(), "queued".into(), 0)
    );
    tx.commit().await.unwrap();
    assert_eq!(
        states(&mut *world.conn().await, machine, command).await,
        ("active".into(), "queued".into(), 0)
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_rejected_revocation_cut_leaves_registration_commands_and_audit_unchanged()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::rejected_revocation_cut_leaves_registration_commands_and_audit_unchanged(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn revocation_packet_keeps_its_transaction_and_native_once_only_cancellation() {
    let world = World::new().await;
    let owner = crate::places::person(&world, "packet-owner").await;
    let (machine, command) = machine(&world, owner.id()).await;
    let mut original = world.app.app.db.begin().await.unwrap();
    let prepared = CertificateService::prepare_revocation(&mut original, machine)
        .await
        .unwrap();
    original.rollback().await.unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    let cuts = AtomicUsize::new(0);
    let wrong = CertificateService::apply_revocation(
        &mut tx,
        prepared,
        Actor::User(owner.id()),
        &Origin::default(),
        world.now(),
        || {
            cuts.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &'static str>(())
        },
    )
    .await
    .unwrap();
    assert!(matches!(wrong, Err(DbError::Rule { .. })));
    assert_eq!(cuts.load(Ordering::SeqCst), 0);
    assert_eq!(
        states(&mut tx, machine, command).await,
        ("active".into(), "queued".into(), 0)
    );
    let prepared = CertificateService::prepare_revocation(&mut tx, machine)
        .await
        .unwrap();
    assert!(
        CertificateService::apply_revocation(
            &mut tx,
            prepared,
            Actor::User(owner.id()),
            &Origin::default(),
            world.now(),
            || {
                cuts.fetch_add(1, Ordering::SeqCst);
                Ok::<(), &'static str>(())
            }
        )
        .await
        .unwrap()
        .unwrap()
    );
    assert_eq!(cuts.load(Ordering::SeqCst), 1);
    assert_eq!(
        states(&mut tx, machine, command).await,
        ("revoked".into(), "cancelled".into(), 1)
    );
    let prepared = CertificateService::prepare_revocation(&mut tx, machine)
        .await
        .unwrap();
    assert!(
        !CertificateService::apply_revocation(
            &mut tx,
            prepared,
            Actor::User(owner.id()),
            &Origin::default(),
            world.now(),
            || {
                cuts.fetch_add(1, Ordering::SeqCst);
                Ok::<(), &'static str>(())
            }
        )
        .await
        .unwrap()
        .unwrap()
    );
    assert_eq!(
        cuts.load(Ordering::SeqCst),
        1,
        "an already revoked registration has no first effect"
    );
    tx.commit().await.unwrap();
    assert_eq!(
        states(&mut *world.conn().await, machine, command).await,
        ("revoked".into(), "cancelled".into(), 1)
    );
    let mut conn = world.conn().await;
    assert!(matches!(
        CertificateService::prepare_revocation(&mut conn, machine).await,
        Err(DbError::Rule { .. })
    ));
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_revocation_packet_keeps_its_transaction_and_native_once_only_cancellation()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::revocation_packet_keeps_its_transaction_and_native_once_only_cancellation(),
    )
}
