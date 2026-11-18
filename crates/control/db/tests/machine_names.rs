//! What a machine is called wherever another thing points at it: the machine of a host's container is called by its host's name, since its own name is the container's hostname and says nothing to a person.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::HashMap;

use chrono::Utc;
use orch_db::Db;
use orch_db::queries::resources::routes::machine_names;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::queries::resources::thread_engine::machine_name;
use orch_db::queries::resources::{NewHost, insert_host};
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use sqlx::postgres::PgPoolOptions;

/// A pool for a database of its own.
async fn world() -> (TestDb, Db) {
    let db = TestDb::fresh().await;
    let pool = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&db.url)
            .await
            .unwrap(),
    );
    (db, pool)
}

#[tokio::test]
async fn a_container_machine_is_called_by_its_host_and_a_connected_machine_by_its_own_name() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let host_machine = insert_machine(&mut conn, "eu-1 machine", "connected")
        .await
        .unwrap();
    let host = new_id();
    insert_host(
        &mut conn,
        &NewHost {
            id: host,
            machine_id: host_machine,
            owner_id: None,
            home_id: None,
            name: "eu-1",
        },
        Utc::now(),
    )
    .await
    .unwrap();
    let container = insert_machine(&mut conn, "fc7220076919", "managed")
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET host_id = $2 WHERE id = $1")
        .bind(container)
        .bind(host)
        .execute(&mut *conn)
        .await
        .unwrap();
    let laptop = insert_machine(&mut conn, "Ada's laptop", "connected")
        .await
        .unwrap();

    let named: HashMap<Id, _> = machine_names(&mut conn, &[container, laptop, host_machine])
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    assert_eq!(named[&container].name, "eu-1");
    assert_eq!(named[&container].host_id, Some(host));
    assert_eq!(named[&laptop].name, "Ada's laptop");
    assert_eq!(named[&laptop].host_id, None);
    assert_eq!(named[&host_machine].name, "eu-1 machine");

    let one = machine_name(&mut conn, container).await.unwrap().unwrap();
    assert_eq!(one.name, "eu-1");
    assert_eq!(one.host_id, Some(host));
    assert_eq!(
        one.placement, "managed",
        "a screen can still say that it is a container"
    );
    let own = machine_name(&mut conn, laptop).await.unwrap().unwrap();
    assert_eq!(own.name, "Ada's laptop");
    assert_eq!(own.host_id, None);
}
