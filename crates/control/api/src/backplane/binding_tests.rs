//! A locally discovered subscription is registered once across repeated discovery and reconnects.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::*;
use crate::{registry::Registry, testing::TestApp};
use orch_link::streams::{Received, StreamReceiver};
use orch_link_contract::streams::Ev;
use orch_testkit::{clock, database::TestDb};
use serde_json::json;

/// A connected machine with the agent and provider roles, owned by a new person, with or without a personal home.
async fn machine_of_new_owner(app: &TestApp, name: &str, with_home: bool) -> (Id, Id, Option<Id>) {
    let clock = app.clock.clone();
    let mut conn = app.app.db.acquire().await.unwrap();
    let owner = resources::testing::insert_user(&mut conn, &format!("{name}-owner"))
        .await
        .unwrap();
    let home = if with_home {
        let home = orch_types::new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn, home, "org", None, "Personal", owner,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE users SET personal_org_id = $2 WHERE id = $1")
            .bind(owner)
            .bind(home)
            .execute(&mut *conn)
            .await
            .unwrap();
        // Registration places the owner in their home and starts its canonical Owners row.
        // A node's attribution column alone is not membership or ownership.
        orch_db::queries::access::add_member(&mut conn, home, owner, home, None, clock.now())
            .await
            .unwrap();
        orch_authz::pages::start(
            &mut conn,
            orch_access_contract::pages::StoredKind::Node,
            home,
            owner,
            clock.time(),
        )
        .await
        .unwrap();
        Some(home)
    } else {
        None
    };
    let machine = resources::testing::insert_machine(&mut conn, name, "connected")
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET roles = ARRAY['agent','provider'], owner_id = $2, home_id = $3 WHERE id = $1",
    )
    .bind(machine)
    .bind(owner)
    .bind(home)
    .execute(&mut *conn)
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Machine,
            id: machine,
        },
        owner,
        orch_authz::ownership::NewAccess::Defaults,
        clock.time(),
    )
    .await
    .unwrap();
    (machine, owner, home)
}

/// One `provider.found` report from the machine, applied in its own transaction.
async fn report_sign_in(app: &TestApp, machine: Id, identity: &str) {
    let clock = app.clock.clone();
    let mut receiver = StreamReceiver::new(BTreeMap::new(), vec![Role::Agent, Role::Provider]);
    let delivery = match receiver.on_ev(Ev {
        stream: "machine".to_owned(),
        seq: 1,
        at: clock.time(),
        kind: "provider.found".to_owned(),
        body: json!({"harness_kind": "local-harness", "identity": identity}),
    }) {
        Received::Apply(delivery) => delivery,
        other => panic!("no discovery delivery: {other:?}"),
    };
    let mut tx = app.app.db.begin().await.unwrap();
    app.app
        .link
        .apply_found_providers(&mut tx, machine, &delivery, clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// The machine's registered providers: owner, home, profile and reported identity.
async fn machine_providers(app: &TestApp, machine: Id) -> Vec<(Id, Id, Id, Json)> {
    let mut conn = app.app.db.acquire().await.unwrap();
    sqlx::query_as(
        "SELECT owner_id, home_id, agent_id, facts FROM providers WHERE machine_id = $1 ORDER BY created_at, id",
    )
    .bind(machine)
    .fetch_all(&mut *conn)
    .await
    .unwrap()
}

async fn machine_profiles(app: &TestApp, machine: Id) -> i64 {
    let mut conn = app.app.db.acquire().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM agents WHERE machine_id = $1")
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_sign_in_discovered_again_on_reconnect_keeps_one_provider_and_one_profile() {
    let database = TestDb::fresh().await;
    let clock = clock::manual();
    let app = TestApp::new(
        Db::from_pool(database.pool.clone()),
        clock.clone(),
        Registry::new(),
    );
    let (machine, owner, home) = machine_of_new_owner(&app, "laptop", true).await;
    let home = home.unwrap();
    for identity in [
        "person@example.test",
        "person@example.test",
        "new@example.test",
    ] {
        let mut receiver = StreamReceiver::new(BTreeMap::new(), vec![Role::Agent, Role::Provider]);
        let delivery = match receiver.on_ev(Ev {
            stream: "machine".to_owned(),
            seq: 1,
            at: clock.time(),
            kind: "provider.found".to_owned(),
            body: json!({"harness_kind": "local-harness", "identity": identity}),
        }) {
            Received::Apply(delivery) => delivery,
            other => panic!("no discovery delivery: {other:?}"),
        };
        let mut tx = app.app.db.begin().await.unwrap();
        app.app
            .link
            .apply_found_providers(&mut tx, machine, &delivery, clock.now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let mut conn = app.app.db.acquire().await.unwrap();
    let providers: Vec<(Id, Id, Id, Json)> = sqlx::query_as(
        "SELECT owner_id, home_id, machine_id, facts FROM providers WHERE machine_id = $1",
    )
    .bind(machine)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(
        (&providers[0].0, &providers[0].1, &providers[0].2),
        (&owner, &home, &machine)
    );
    assert_eq!(providers[0].3["identity"], "new@example.test");
    let profiles: i64 = sqlx::query_scalar("SELECT count(*) FROM agents WHERE machine_id = $1")
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(profiles, 1);
    app.app.link.shutdown();
}

#[tokio::test]
async fn an_owner_without_a_personal_home_keeps_a_registration_current_and_gets_no_new_one() {
    let database = TestDb::fresh().await;
    let app = TestApp::new(
        Db::from_pool(database.pool.clone()),
        clock::manual(),
        Registry::new(),
    );
    // A registration made while the owner had a personal home stays current after it is gone.
    let (machine, owner, home) = machine_of_new_owner(&app, "laptop", true).await;
    report_sign_in(&app, machine, "first@example.test").await;
    {
        let mut conn = app.app.db.acquire().await.unwrap();
        sqlx::query("UPDATE users SET personal_org_id = NULL WHERE id = $1")
            .bind(owner)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    report_sign_in(&app, machine, "second@example.test").await;
    let providers = machine_providers(&app, machine).await;
    assert_eq!(providers.len(), 1);
    assert_eq!((providers[0].0, providers[0].1), (owner, home.unwrap()));
    assert_eq!(providers[0].3["identity"], "second@example.test");
    assert_eq!(machine_profiles(&app, machine).await, 1);

    // An owner who never had a personal home chooses the home of a new registration themselves.
    let (bare, _, none) = machine_of_new_owner(&app, "workstation", false).await;
    assert_eq!(none, None);
    report_sign_in(&app, bare, "someone@example.test").await;
    assert!(machine_providers(&app, bare).await.is_empty());
    assert_eq!(machine_profiles(&app, bare).await, 0);
    app.app.link.shutdown();
}

#[tokio::test]
async fn a_profile_pointed_at_another_home_is_never_reused_for_the_default_sign_in() {
    let database = TestDb::fresh().await;
    let app = TestApp::new(
        Db::from_pool(database.pool.clone()),
        clock::manual(),
        Registry::new(),
    );
    let (machine, owner, home) = machine_of_new_owner(&app, "laptop", true).await;
    let home = home.unwrap();
    let custom = orch_types::new_id();
    let custom_profile = json!({"attach": {"home": "/srv/second-sign-in"}});
    {
        let mut conn = app.app.db.acquire().await.unwrap();
        orch_db::queries::resources::routes::insert_agent(
            &mut conn,
            &orch_db::queries::resources::routes::NewAgent {
                id: custom,
                name: "local-harness (second sign-in)",
                owner_id: owner,
                home_id: home,
                harness_kind: "local-harness",
                machine_id: Some(machine),
                profile: &custom_profile,
            },
            app.clock.now(),
        )
        .await
        .unwrap();
    }
    for identity in ["person@example.test", "person@example.test"] {
        report_sign_in(&app, machine, identity).await;
    }
    let providers = machine_providers(&app, machine).await;
    assert_eq!(providers.len(), 1);
    assert_ne!(
        providers[0].2, custom,
        "the default sign-in got its own profile"
    );
    assert_eq!(machine_profiles(&app, machine).await, 2);
    let mut conn = app.app.db.acquire().await.unwrap();
    let kept: Json = sqlx::query_scalar("SELECT profile FROM agents WHERE id = $1")
        .bind(custom)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(kept, custom_profile);
    app.app.link.shutdown();
}
