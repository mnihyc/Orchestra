//! Access requests reach current entitled givers without naming hidden content to the requester.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::pages::Target;
use orch_authz::requests::{self, Ask, Asked, Giving, Whom};
use orch_authz::usable::ResourcePolicy;
use orch_authz::{Caller, Policy};
use orch_db::audit::Origin;
use orch_db::queries::identity::testing::suspend;
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_nearer_page_only_manager_is_not_sent_a_request_an_entitled_admin_can_decide() {
    let w = World::new().await;
    let requester = w.member("requester").await;
    let manager = w.member("manager").await;
    w.change(w.maker, Target::Live(w.thread), json!({"whole": "private"}))
        .await
        .unwrap();
    w.name(w.thread, P::AccessManage, Holder::User { id: manager })
        .await;
    w.name(
        w.thread,
        P::ThreadInteract,
        Holder::Role {
            role: Role::Admin,
            at: None,
        },
    )
    .await;
    let mut conn = w.conn().await;
    suspend(&mut conn, w.maker).await.unwrap();
    let principal = w.principal(Caller::session(requester)).await;
    let ask = Ask {
        label: None,
        object: ObjectRef {
            kind: ObjectKind::Thread,
            id: w.thread,
        },
        permissions: vec![P::ThreadInteract],
        options: None,
        whom: Whom::Requester,
    };
    assert_eq!(
        requests::named_for(&mut conn, &principal, ask.object, Policy::default())
            .await
            .unwrap(),
        None
    );
    let planned = requests::plan(&mut conn, &principal, &ask, &ResourcePolicy::default())
        .await
        .unwrap();
    assert_eq!(planned.givers.nearest, [w.boss]);
    assert!(!planned.givers.further.contains(&manager));
    let principal = w.principal(Caller::session(manager)).await;
    let standing = requests::standing(
        &mut conn,
        &principal,
        ask.object,
        &ask.permissions,
        requester,
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(standing.may_decide);
    assert!(!standing.can_give);
    assert!(!standing.sent_to_them);
    // An admin decides as one, with As admin on in their own session.
    let principal = w
        .principal(Caller::session(w.boss).with_admin_mode(true))
        .await;
    let standing = requests::standing(
        &mut conn,
        &principal,
        ask.object,
        &ask.permissions,
        requester,
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(standing.may_decide && standing.can_give && standing.sent_to_them);
    drop(conn);
    let mut tx = w.db.pool.begin().await.unwrap();
    requests::grant(
        &mut tx,
        &principal,
        requester,
        ask.object,
        &Asked::of(&ask, "please"),
        &Giving::default(),
        &ResourcePolicy::default(),
        &Origin::default(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(w.can(requester, w.thread, P::ThreadInteract).await);
    assert!(!w.can(requester, w.thread, P::ThreadManage).await);
}
