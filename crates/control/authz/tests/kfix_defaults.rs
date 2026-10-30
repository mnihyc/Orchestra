//! Place admins write defaults; entry managers give only permissions held on a follower.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::{Holder, Origin};
use orch_access_contract::permissions::{Permission as P, Profile};
use orch_access_contract::roles::PlaceRef;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::{Caller, defaults};
use serde_json::json;
use support::{Statements, World, user_holder};

#[tokio::test]
async fn a_place_admin_can_start_a_defaults_row_that_no_follower_gives_them() {
    let w = World::new().await;
    let recipient = w.member("recipient").await;
    let provider = w.provider(w.maker, w.root, "API").await;
    assert!(!w.can_on(w.boss, "provider", provider, P::ProviderUse).await);
    let page = w
        .read(w.boss, Target::Defaults(PlaceRef::Node(w.root)))
        .await;
    assert!(
        page.rows
            .iter()
            .find(|row| row.permission == P::ProviderUse)
            .unwrap()
            .can_give
    );
    let made = w
        .change_defaults(
            w.boss,
            w.root,
            json!({"rows": [{
                "permission": "provider.use", "add": [{"holder": user_holder(recipient)}]
            }]}),
        )
        .await
        .unwrap();
    assert!(made.changed);
    assert!(!made.r#override);
    assert!(
        w.can_on(recipient, "provider", provider, P::ProviderUse)
            .await
    );
    assert!(!w.can_on(w.boss, "provider", provider, P::ProviderUse).await);
}

#[tokio::test]
async fn an_entry_manager_needs_the_followers_permission_before_giving_in_defaults() {
    let w = World::new().await;
    let manager = w.member("manager").await;
    let recipient = w.member("recipient").await;
    w.name(w.root, P::AccessManage, Holder::User { id: manager })
        .await;
    let provider = w.provider(w.maker, w.root, "API").await;
    assert!(w.can(manager, w.root, P::AccessManage).await);
    assert!(
        !w.can_on(manager, "provider", provider, P::ProviderUse)
            .await
    );
    let page = w
        .read(manager, Target::Defaults(PlaceRef::Node(w.root)))
        .await;
    assert!(
        !page
            .rows
            .iter()
            .find(|row| row.permission == P::ProviderUse)
            .unwrap()
            .can_give
    );
    let change = json!({"rows": [{
        "permission": "provider.use", "add": [{"holder": user_holder(recipient)}]
    }]});
    let epoch = w.epoch().await;
    let refused = w.change_defaults(manager, w.root, change.clone()).await;
    let Err(ChangeError::Denied(refusal)) = refused else {
        panic!("an entry manager cannot start an unheld defaults row: {refused:?}");
    };
    assert!(
        refusal
            .missing
            .iter()
            .any(|missing| missing.permissions.contains(&P::ProviderUse))
    );
    assert_eq!(w.epoch().await, epoch);
    assert!(
        !w.can_on(recipient, "provider", provider, P::ProviderUse)
            .await
    );

    w.change_defaults(
        w.boss,
        w.root,
        json!({"rows": [{
            "permission": "provider.use", "add": [{"holder": user_holder(manager)}]
        }]}),
    )
    .await
    .unwrap();
    assert!(
        w.can_on(manager, "provider", provider, P::ProviderUse)
            .await
    );
    w.change_defaults(manager, w.root, change).await.unwrap();
    assert!(
        w.can_on(recipient, "provider", provider, P::ProviderUse)
            .await
    );
}

#[tokio::test]
async fn a_defaults_preview_reads_all_profiles_together_without_losing_a_lock_or_mixing_their_holders()
 {
    let w = World::new().await;
    let locked = w
        .name_default(
            Some(w.root),
            P::ThreadInteract,
            "managed",
            Holder::User { id: w.boss },
        )
        .await;
    w.default_row(w.root, P::ThreadInteract, "managed", false, true)
        .await;
    let shadowed = w
        .name_default(
            Some(w.topic),
            P::ThreadInteract,
            "managed",
            Holder::User { id: w.maker },
        )
        .await;
    w.default_row(w.topic, P::ThreadInteract, "managed", false, false)
        .await;
    let own = w
        .name_default(
            Some(w.topic),
            P::ThreadInteract,
            "connected",
            Holder::User { id: w.maker },
        )
        .await;
    w.default_row(w.topic, P::ThreadInteract, "connected", false, false)
        .await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let rows = [
        (P::ThreadInteract, Profile::Managed),
        (P::ThreadInteract, Profile::Connected),
        (P::ThreadView, Profile::Managed),
        (P::ThreadView, Profile::Connected),
        (P::EnvironmentManage, Profile::Managed),
    ];
    // Warm the statement cache before comparing one defaults row with the whole preview.
    defaults::resolve_rows(&mut conn, &principal, PlaceRef::Node(w.topic), &rows)
        .await
        .unwrap();
    let before = Statements::now();
    defaults::resolve_rows(&mut conn, &principal, PlaceRef::Node(w.topic), &rows[..1])
        .await
        .unwrap();
    let one = Statements::now() - before;
    let before = Statements::now();
    let read = defaults::resolve_rows(&mut conn, &principal, PlaceRef::Node(w.topic), &rows)
        .await
        .unwrap();
    let all = Statements::now() - before;
    eprintln!("defaults preview statements: one={one}, all={all}");
    assert!(one > 0, "statement counting is active");
    assert!(
        all <= one,
        "a preview reads its pages once: one={one}, all={all}"
    );
    assert!(
        read[0].iter().any(|entry| entry.entry.id == locked
            && entry.from == Origin::Defaults(PlaceRef::Node(w.root)))
    );
    assert!(
        !read[0]
            .iter()
            .any(|entry| entry.entry.id == shadowed || entry.entry.id == own)
    );
    assert!(
        read[1].iter().any(|entry| entry.entry.id == own
            && entry.from == Origin::Defaults(PlaceRef::Node(w.topic)))
    );
    assert!(
        !read[1]
            .iter()
            .any(|entry| entry.entry.id == locked || entry.entry.id == shadowed)
    );
    assert!(
        read[2..].iter().all(|entries| !entries.is_empty()),
        "the other rows retain their shipped defaults"
    );
}
