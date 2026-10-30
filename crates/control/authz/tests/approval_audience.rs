//! Approval audiences name direct people only, while group owners and administrators retain current deciding authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::approvals::{owners_to_tell, permission_deciders};
use orch_authz::{Caller, Principal};
use orch_db::queries::access::{PageKey, testing::add_entry};
use support::World;

#[tokio::test]
async fn a_group_owner_may_decide_without_being_told_for_threads_plans_ports_and_scoped_capabilities()
 {
    let w = World::new().await;
    let person = w.member("groupmember").await;
    let group = w.group(w.root, "Owners group", &[person]).await;
    let environment = w.thing("environment", w.maker, w.topic, "Workspace").await;
    let host = w.thing("host", w.maker, w.root, "Build host").await;
    for (kind, id, permission) in [
        (ObjectKind::Thread, w.thread, Permission::ThreadManage),
        (ObjectKind::Thread, w.thread, Permission::ThreadInteract),
        (
            ObjectKind::Environment,
            environment,
            Permission::PortsManage,
        ),
        (ObjectKind::Host, host, Permission::HostManage),
        (ObjectKind::Org, w.root, Permission::AccessManage),
    ] {
        let stored = if kind == ObjectKind::Thread || kind == ObjectKind::Org {
            "node"
        } else {
            kind.as_str()
        };
        let entry = add_entry(
            &mut *w.conn().await,
            &PageKey::new(stored, id, "live"),
            "any",
            "access.own",
            ("group", Some(group), None),
            None,
            w.now.get(),
        )
        .await
        .unwrap();
        let object = ObjectRef { kind, id };
        let eligible = permission_deciders(&mut *w.conn().await, object, permission, w.now)
            .await
            .unwrap();
        assert!(
            eligible.contains(&person),
            "{kind:?}: a group owner may decide"
        );
        let told = owners_to_tell(&mut *w.conn().await, object, &eligible, w.now)
            .await
            .unwrap();
        assert!(
            !told.contains(&person),
            "{kind:?}: groups do not fan out notices"
        );
        assert!(
            !told.is_empty(),
            "{kind:?}: an eligible directly named owner is told"
        );
        sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
            .bind(entry)
            .bind(w.now.get())
            .execute(&mut *w.conn().await)
            .await
            .unwrap();
        let now = permission_deciders(&mut *w.conn().await, object, permission, w.now)
            .await
            .unwrap();
        assert!(
            !now.contains(&person),
            "{kind:?}: pending authority disappears when its source ends"
        );
    }
}

#[tokio::test]
async fn a_suspended_sole_person_owner_falls_back_to_the_nearest_eligible_parent_owner() {
    let w = World::new().await;
    let environment = w.thing("environment", w.maker, w.topic, "Workspace").await;
    let host = w.thing("host", w.maker, w.root, "Build host").await;
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(w.maker)
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
    for (kind, id, permission) in [
        (ObjectKind::Thread, w.thread, Permission::ThreadManage),
        (ObjectKind::Thread, w.thread, Permission::ThreadInteract),
        (
            ObjectKind::Environment,
            environment,
            Permission::PortsManage,
        ),
        (ObjectKind::Host, host, Permission::HostManage),
    ] {
        let object = ObjectRef { kind, id };
        let eligible = permission_deciders(&mut *w.conn().await, object, permission, w.now)
            .await
            .unwrap();
        assert!(!eligible.contains(&w.maker));
        assert_eq!(
            owners_to_tell(&mut *w.conn().await, object, &eligible, w.now)
                .await
                .unwrap(),
            [w.boss],
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_only_admins_may_give_bypasses_the_things_ordinary_owner() {
    let w = World::new().await;
    let requester = w.member("requester").await;
    let principal = Principal::load(&mut *w.conn().await, Caller::session(requester), w.now)
        .await
        .unwrap();
    let object = ObjectRef {
        kind: ObjectKind::Thread,
        id: w.thread,
    };
    let asked = orch_authz::requests::Ask {
        label: None,
        object,
        permissions: vec![Permission::AccessExternal],
        options: None,
        whom: orch_authz::requests::Whom::Requester,
    };
    let sent = orch_authz::requests::plan(
        &mut *w.conn().await,
        &principal,
        &asked,
        &orch_authz::usable::ResourcePolicy {
            owners_name_outsiders: true,
            shared_subscriptions: None,
        },
    )
    .await
    .unwrap();
    let eligible = sent
        .givers
        .nearest
        .into_iter()
        .chain(sent.givers.further)
        .collect::<Vec<_>>();
    assert!(!eligible.contains(&w.maker));
    assert_eq!(
        owners_to_tell(&mut *w.conn().await, object, &eligible, w.now)
            .await
            .unwrap(),
        [w.boss]
    );
}

#[tokio::test]
async fn a_current_holder_is_still_found_when_every_owner_and_admin_is_suspended() {
    let w = World::new().await;
    let person = w.member("remainingdecider").await;
    add_entry(
        &mut *w.conn().await,
        &PageKey::new("node", w.thread, "live"),
        "any",
        "thread.manage",
        ("user", Some(person), None),
        None,
        w.now.get(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE users SET status='suspended' WHERE id<>$1 AND kind='human'")
        .bind(person)
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
    let object = ObjectRef {
        kind: ObjectKind::Thread,
        id: w.thread,
    };
    let eligible = permission_deciders(
        &mut *w.conn().await,
        object,
        Permission::ThreadManage,
        w.now,
    )
    .await
    .unwrap();
    assert_eq!(eligible, [person]);
    assert_eq!(
        owners_to_tell(&mut *w.conn().await, object, &eligible, w.now)
            .await
            .unwrap(),
        [person]
    );
}
