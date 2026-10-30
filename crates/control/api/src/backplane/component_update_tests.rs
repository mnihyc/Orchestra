//! The real authorization supplier retains an idle fence for a below-minimum node's authenticated recovery.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
use super::*;
use crate::{registry::Registry, testing::TestApp};
use orch_link_contract::session::TlsMode;
use orch_testkit::{clock, database::TestDb};

#[tokio::test]
async fn a_minimum_version_node_gets_one_audited_idle_grant_until_it_reconnects_on_the_recommendation()
 {
    let db = TestDb::fresh().await;
    let app = TestApp::new(
        Db::from_pool(db.pool.clone()),
        clock::manual(),
        Registry::new(),
    );
    let mut conn = app.app.db.acquire().await.unwrap();
    let user = resources::testing::insert_user(&mut conn, "operator")
        .await
        .unwrap();
    let host =
        resources::access_fixtures::insert_resource(&mut conn, "host", Some(user), None, "node")
            .await
            .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET roles=ARRAY['node'] WHERE id=$1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    let row = resources::machine(&mut conn, machine)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    let mut query = AuthQuery {
        endpoints: None,
        hosted_endpoints: None,
        n: 1,
        machine_id: machine,
        fingerprint: hex::encode(row.cert_fingerprint),
        tls: TlsMode::Server,
        ip: "127.0.0.1".parse().unwrap(),
        connector_version: "0.0.9".into(),
        roles: vec![Role::Node],
    };
    let reply = app.app.link.authorize(&query).await;
    assert!(!reply.ok, "the proved old node still gets no session");
    assert_eq!(reply.code, Some(RejectCode::UpgradeRequired));
    let recommendation = reply.update_permit.unwrap().recommended;
    assert!(crate::component_updates::newer(
        &recommendation,
        &query.connector_version
    ));
    let repeated = app.app.link.authorize(&query).await;
    assert!(repeated.update_permit.is_some());
    app.app.link.advance_component_updates().await.unwrap();
    let mut conn = app.app.db.acquire().await.unwrap();
    let update = orch_db::queries::component_updates::among(&mut conn, &[machine])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(update.state, "updating");
    assert!(
        update.command_id.is_none(),
        "a refused connection cannot receive the ordinary link command"
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='machine.update_requested' AND object_id=$1",
    )
    .bind(machine)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    drop(conn);
    let good_fingerprint = query.fingerprint.clone();
    query.fingerprint.replace_range(
        0..1,
        if good_fingerprint.starts_with('0') {
            "1"
        } else {
            "0"
        },
    );
    let bad = app.app.link.authorize(&query).await;
    assert!(!bad.ok);
    assert!(bad.update_permit.is_none());
    query.fingerprint = good_fingerprint;
    query.connector_version = recommendation;
    let current = app.app.link.authorize(&query).await;
    assert!(current.ok);
    assert!(current.update_permit.is_none());
    let mut conn = app.app.db.acquire().await.unwrap();
    assert!(
        orch_db::queries::component_updates::pending(&mut conn)
            .await
            .unwrap()
            .is_empty()
    );
}
