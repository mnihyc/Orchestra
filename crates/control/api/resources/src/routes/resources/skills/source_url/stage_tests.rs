//! Native setup controls use the actual source sealer and secret INSERT, not an HTTP privilege proof.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::*;

use crate::rig::Scene;

use std::sync::atomic::AtomicUsize;

use std::sync::atomic::Ordering;
const PRIVATE: &str = "https://example.invalid/skill.tar.gz?fixture-source=private";

async fn count(scene: &Scene, skill: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM secrets WHERE owner_kind='skill' AND owner_id=$1")
        .bind(skill)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap()
}

#[cfg(any(test, feature = "test-support"))]
async fn native_source_sealing_prepares_without_custody_and_consumes_one_real_first_insert_cut() {
    let scene = Scene::new().await;
    let skill = new_id();
    let prepared = prepare_stage(skill, PRIVATE, scene.ada.id()).unwrap();
    assert_eq!(count(&scene, skill).await, 0);
    let calls = AtomicUsize::new(0);
    let mut tx = scene.rig.db.begin().await.unwrap();
    let refused = apply_stage(
        &mut tx,
        prepared,
        scene.ada.id(),
        scene.rig.world.now(),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "native setup cut closed",
            ))
        },
    )
    .await;
    assert!(
        matches!(refused,Err(error)if error.code()==orch_api_contract::common::ErrorCode::AuthzDenied)
    );
    tx.rollback().await.unwrap();
    assert_eq!(count(&scene, skill).await, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let prepared = prepare_stage(skill, PRIVATE, scene.ada.id()).unwrap();
    let mut tx = scene.rig.db.begin().await.unwrap();
    let (shown, secret) = apply_stage(
        &mut tx,
        prepared,
        scene.ada.id(),
        scene.rig.world.now(),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap();
    let secret = secret.unwrap();
    assert!(!shown.contains("fixture-source"));
    assert_eq!(secret.owner_kind, "skill");
    assert_eq!(secret.owner_id, skill);
    let row = secrets::secret(&mut tx, sealing::SKILL, skill, &secret.name)
        .await
        .unwrap()
        .unwrap();
    let opened = sealing::installed_master_key()
        .unwrap()
        .open(
            &row.sealed,
            &sealing::binding(sealing::SKILL, skill, &secret.name),
        )
        .unwrap();
    assert_eq!(opened, PRIVATE.as_bytes());
    assert!(
        sealing::installed_master_key()
            .unwrap()
            .open(
                &row.sealed,
                &sealing::binding(sealing::SKILL, new_id(), &secret.name)
            )
            .is_err()
    );
    tx.commit().await.unwrap();
    assert_eq!(count(&scene, skill).await, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_native_source_sealing_prepares_without_custody_and_consumes_one_real_first_insert_cut()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::native_source_sealing_prepares_without_custody_and_consumes_one_real_first_insert_cut(
        ),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn native_source_public_noop_actor_and_transaction_rules_do_not_fabricate_secret_or_first_effect()
 {
    let scene = Scene::new().await;
    let skill = new_id();
    let calls = AtomicUsize::new(0);
    let mut tx = scene.rig.db.begin().await.unwrap();
    let prepared = prepare_stage(
        skill,
        "https://example.invalid/public.tar.gz",
        scene.ada.id(),
    )
    .unwrap();
    let (shown, secret) = apply_stage(
        &mut tx,
        prepared,
        scene.ada.id(),
        scene.rig.world.now(),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(shown, "https://example.invalid/public.tar.gz");
    assert!(secret.is_none());
    tx.commit().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(count(&scene, skill).await, 0);
    let prepared = prepare_stage(skill, PRIVATE, scene.ada.id()).unwrap();
    let mut tx = scene.rig.db.begin().await.unwrap();
    assert!(
        apply_stage(
            &mut tx,
            prepared,
            scene.cai.id(),
            scene.rig.world.now(),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        )
        .await
        .is_err()
    );
    tx.rollback().await.unwrap();
    let prepared = prepare_stage(skill, PRIVATE, scene.ada.id()).unwrap();
    let mut conn = scene.rig.db.acquire().await.unwrap();
    assert!(
        apply_stage(
            &mut conn,
            prepared,
            scene.ada.id(),
            scene.rig.world.now(),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        )
        .await
        .is_err()
    );
    drop(conn);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(count(&scene, skill).await, 0);
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_native_source_public_noop_actor_and_transaction_rules_do_not_fabricate_secret_or_first_effect()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::native_source_public_noop_actor_and_transaction_rules_do_not_fabricate_secret_or_first_effect())
}
