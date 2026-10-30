//! Role giving and page offers obey current permission-name scopes across both default profiles.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_authz::giving::{self, RoleStanding};
use orch_authz::pages::{self, Target};
use orch_authz::{Caller, Policy, TokenGrant};
use orch_types::Id;
use serde_json::json;
use support::World;

fn scoped(user: Id, place: Id, permissions: &[P]) -> Caller {
    let names: Vec<String> = permissions
        .iter()
        .map(|permission| permission.as_str().to_owned())
        .collect();
    Caller::token(user, TokenGrant::from_stored(&names, Some(place)).unwrap())
}

async fn standing(w: &World, caller: Caller, place: Id, holders_share: bool) -> RoleStanding {
    let principal = w.principal(caller).await;
    let mut conn = w.conn().await;
    giving::role_standing(
        &mut conn,
        &principal,
        place,
        Policy {
            holders_share,
            ..Policy::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_view_only_token_cannot_give_a_shipped_role_and_its_page_offers_agree() {
    let w = World::new().await;
    let caller = scoped(w.boss, w.project, &[P::ProjectView]);
    let held = standing(&w, caller.clone(), w.project, true).await;
    assert_eq!(held.held, Some(Role::Admin));
    assert!(held.roles().is_empty());
    let principal = w.principal(caller).await;
    let mut conn = w.conn().await;
    let page = pages::read(
        &mut conn,
        &principal,
        Target::Live(w.project),
        Policy {
            holders_share: true,
            ..Policy::default()
        },
    )
    .await
    .unwrap();
    assert!(page.gives.roles.is_empty());
    let refusal = giving::role_refusal(
        &mut conn,
        &principal,
        w.project,
        Role::Viewer,
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(!refusal.requestable);
    assert!(refusal.missing.iter().all(|missing| missing.ask.is_empty()));
    assert!(
        refusal
            .missing
            .iter()
            .any(|missing| missing.permissions.contains(&P::ThreadView))
    );

    // Naming every permission is sufficient, without using the wildcard scope;
    // ownership still permits sharing where the policy keeps it to owners.
    let held = standing(&w, scoped(w.boss, w.project, P::ALL), w.project, false).await;
    assert!(held.may_give(Role::Viewer));
    assert!(held.may_give(Role::Developer));
    let other = w.node("project", w.root, "Other").await;
    assert!(
        !standing(&w, scoped(w.boss, w.project, P::ALL), other, true)
            .await
            .may_give(Role::Viewer)
    );
}

#[tokio::test]
async fn role_scopes_include_implied_rows_both_profiles_and_the_places_own_entries() {
    let w = World::new().await;
    w.change(
        w.boss,
        Target::Live(w.project),
        json!({"whole": "private", "rows": [{
            "permission": "project.view", "add": [{"holder": {"kind": "role", "role": "viewer"}}]
        }]}),
    )
    .await
    .unwrap();
    w.change_defaults(w.boss, w.project, json!({"whole": "private", "rows": [
        {"permission": "thread.manage", "profile": "managed", "add": [{"holder": {"kind": "role", "role": "viewer"}}]},
        {"permission": "files.manage", "profile": "connected", "add": [{"holder": {"kind": "role", "role": "viewer"}}]}
    ]})).await.unwrap();
    for omitted in [
        P::ThreadView,
        P::ThreadExport,
        P::FilesView,
        P::FilesManage,
        P::ProjectView,
    ] {
        let permissions: Vec<P> = P::ALL
            .iter()
            .copied()
            .filter(|permission| *permission != omitted)
            .collect();
        let caller = scoped(w.boss, w.project, &permissions);
        let held = standing(&w, caller.clone(), w.project, true).await;
        assert!(!held.may_give(Role::Viewer), "missing {omitted}");
        let principal = w.principal(caller).await;
        let mut conn = w.conn().await;
        let refusal = giving::role_refusal(
            &mut conn,
            &principal,
            w.project,
            Role::Viewer,
            Policy::default(),
        )
        .await
        .unwrap();
        assert!(
            refusal
                .missing
                .iter()
                .any(|missing| missing.permissions.contains(&omitted)),
            "missing {omitted}"
        );
    }
    assert!(
        standing(&w, scoped(w.boss, w.project, P::ALL), w.project, true)
            .await
            .may_give(Role::Viewer)
    );
    let without_management: Vec<P> = P::ALL
        .iter()
        .copied()
        .filter(|permission| *permission != P::AccessManage)
        .collect();
    let held = standing(
        &w,
        scoped(w.boss, w.project, &without_management),
        w.project,
        true,
    )
    .await;
    assert!(
        held.is_admin(),
        "the identity's role is retained for retraction checks"
    );
    assert!(
        !held.may_give(Role::Admin),
        "giving administration takes its management scope"
    );
}
