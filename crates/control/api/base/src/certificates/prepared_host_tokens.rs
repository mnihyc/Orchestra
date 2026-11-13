//! Native reservations decide the first ledger write; these are not HTTP scope proofs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use super::*;

use crate::support::World;

use orch_api_contract::access::NewResourceAccess;

use std::sync::atomic::AtomicUsize;

use std::sync::atomic::Ordering;
async fn count(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

fn request(user: Id) -> TokenRequest {
    TokenRequest {
        created_by: user,
        purpose: TokenFor::Host { home: None },
        name_hint: Some("Prepared native node".into()),
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn a_host_token_cut_precedes_its_ledger_and_keeps_exact_choices_on_success() {
    let world = World::new().await;
    let issuer = crate::places::person(&world, "nodeissuer").await;
    let service = world.app.app.link.certificates();
    let before = count(&world).await;
    let choices = EnrollmentChoices {
        roles: vec![Role::Node],
        delegate: vec!["*".into()],
        dedicated: false,
    };
    let mut tx = world.app.app.db.begin().await.unwrap();
    let prepared = service
        .prepare_host_token_choices(
            &mut tx,
            &request(issuer.id()),
            Some(NewResourceAccess::OnlyMe),
            Some(&choices),
        )
        .await
        .unwrap();
    let hash = prepared.row.token_hash.clone();
    assert!(
        resources::lock_enrollment_token(&mut tx, &hash)
            .await
            .unwrap()
            .is_none()
    );
    let cuts = AtomicUsize::new(0);
    let refused = service
        .apply_host_token(&mut tx, prepared, || {
            cuts.fetch_add(1, Ordering::SeqCst);
            Err("stop before reserving the token")
        })
        .await;
    assert!(matches!(refused, Err("stop before reserving the token")));
    assert_eq!(cuts.load(Ordering::SeqCst), 1);
    assert!(
        resources::lock_enrollment_token(&mut tx, &hash)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    assert_eq!(count(&world).await, before);

    let mut tx = world.app.app.db.begin().await.unwrap();
    let prepared = service
        .prepare_host_token_choices(
            &mut tx,
            &request(issuer.id()),
            Some(NewResourceAccess::OnlyMe),
            Some(&choices),
        )
        .await
        .unwrap();
    let hash = prepared.row.token_hash.clone();
    let expected = prepared.issued.token.to_string();
    let expires = prepared.issued.expires_at;
    let made = service
        .apply_host_token(&mut tx, prepared, || {
            cuts.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &'static str>(())
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(made.token, expected);
    assert_eq!(made.expires_at, expires);
    assert_eq!(made.receipt, hex::encode(&hash));
    let wire: EnrollmentToken = made.token.parse().unwrap();
    assert!(wire.choices().unwrap().unwrap().matches(&choices));
    let stored: (Id, String, Option<Id>, Option<String>, Vec<String>) = sqlx::query_as(
        "SELECT created_by,purpose,org_id,access,roles FROM enrollment_tokens WHERE token_hash=$1",
    )
    .bind(&hash)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        stored,
        (
            issuer.id(),
            "host".into(),
            None,
            Some("only_me".into()),
            vec!["node".into()]
        )
    );
    tx.commit().await.unwrap();
    assert_eq!(cuts.load(Ordering::SeqCst), 2);
    assert_eq!(count(&world).await, before + 1);
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_host_token_cut_precedes_its_ledger_and_keeps_exact_choices_on_success()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::a_host_token_cut_precedes_its_ledger_and_keeps_exact_choices_on_success(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn a_host_token_reservation_cannot_cross_its_transaction_or_authority_before_the_cut() {
    let world = World::new().await;
    let issuer = crate::places::person(&world, "nativeissuer").await;
    let service = world.app.app.link.certificates();
    let before = count(&world).await;
    let cuts = AtomicUsize::new(0);
    let mut original = world.app.app.db.begin().await.unwrap();
    let prepared = service
        .prepare_host_token_choices(&mut original, &request(issuer.id()), None, None)
        .await
        .unwrap();
    original.rollback().await.unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    let refused = service
        .apply_host_token(&mut tx, prepared, || {
            cuts.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &'static str>(())
        })
        .await
        .unwrap();
    assert!(refused.is_err());
    tx.rollback().await.unwrap();

    let other_authority = Arc::new(
        CertificateAuthority::create("Other test authority", 3650, world.app.app.clock.as_ref())
            .unwrap(),
    );
    let other = CertificateService::new(
        world.app.app.db.clone(),
        world.app.app.clock.clone(),
        other_authority,
        service.inner.settings.clone(),
    );
    let mut tx = world.app.app.db.begin().await.unwrap();
    let prepared = service
        .prepare_host_token_choices(&mut tx, &request(issuer.id()), None, None)
        .await
        .unwrap();
    let refused = other
        .apply_host_token(&mut tx, prepared, || {
            cuts.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &'static str>(())
        })
        .await
        .unwrap();
    assert!(refused.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(cuts.load(Ordering::SeqCst), 0);
    assert_eq!(count(&world).await, before);
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_host_token_reservation_cannot_cross_its_transaction_or_authority_before_the_cut()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::a_host_token_reservation_cannot_cross_its_transaction_or_authority_before_the_cut(),
    )
}
