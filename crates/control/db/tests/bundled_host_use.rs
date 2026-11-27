//! The update that names the first administrator in the Use row of the host installed with the server, for an
//! installation whose host enrolled before enrollment did so: once, as enrollment now does, and nothing else.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{DateTime, Utc};
use orch_db::queries::access::{self, NewEntry, PageKey, RowWrite};
use orch_db::queries::resources::{self, NewHost, testing};
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use serde_json::json;
use std::path::Path;

/// The update's own file, applied again over rows as an installation of 0.1.3 has them.
async fn update(db: &TestDb) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../migrations")
        .join("0023_bundled_host_use.sql");
    let sql = std::fs::read_to_string(file).expect("the update is shipped");
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(&db.pool)
        .await
        .unwrap();
}

/// A host as it enrolled before: its maker alone in its Owners row, its Use row cut loose from the defaults ("only
/// me"), and nobody named in it.
async fn enrolled(
    db: &TestDb,
    maker: Id,
    home: Option<Id>,
    bundled: bool,
    name: &str,
    at: DateTime<Utc>,
) -> Id {
    let mut conn = db.pool.acquire().await.unwrap();
    let machine = testing::insert_machine(&mut conn, name, "managed")
        .await
        .unwrap();
    let id = new_id();
    resources::insert_host(
        &mut conn,
        &NewHost {
            id,
            machine_id: machine,
            owner_id: Some(maker),
            home_id: home,
            name,
        },
        at,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET bundled_with_server=$2 WHERE id=$1")
        .bind(id)
        .bind(bundled)
        .execute(&mut *conn)
        .await
        .unwrap();
    let page = PageKey::new("host", id, "live");
    name_in(&mut conn, &page, "access.own", maker, maker, at).await;
    access::write_row(
        &mut conn,
        &RowWrite {
            page: &page,
            profile: "any",
            permission: "host.use",
            inherit: false,
            locked: false,
        },
        Some(maker),
        at,
    )
    .await
    .unwrap();
    id
}

async fn name_in(
    conn: &mut orch_db::Conn,
    page: &PageKey,
    permission: &str,
    holder: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Id {
    let id = new_id();
    access::insert_entry(
        conn,
        &NewEntry {
            id,
            page,
            profile: "any",
            permission,
            holder_kind: "user",
            holder_id: Some(holder),
            holder_role: None,
            options: &json!({}),
            expires_at: None,
            note: "",
            source: if permission == "access.own" {
                "creation"
            } else {
                "page"
            },
            created_by: Some(by),
            at,
        },
    )
    .await
    .unwrap();
    id
}

/// Who the host's Use row names now: the holder, how, by whom and since when.
async fn use_row(db: &TestDb, host: Id) -> Vec<(Option<Id>, String, Option<Id>, DateTime<Utc>)> {
    sqlx::query_as(
        "SELECT holder_id, source, created_by, created_at FROM permission_entries
          WHERE object_kind='host' AND object_id=$1 AND page='live' AND permission='host.use' AND removed_at IS NULL
          ORDER BY created_at, id",
    )
    .bind(host)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_bundled_hosts_maker_is_named_in_its_use_row_once_and_every_other_row_stays_as_it_is() {
    let db = TestDb::fresh().await;
    let at = chrono::DateTime::parse_from_rfc3339("2026-11-27T09:24:19Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut conn = db.pool.acquire().await.unwrap();
    let first = testing::insert_user(&mut conn, "first").await.unwrap();
    testing::make_system_admin(&mut conn, first).await.unwrap();
    let other = testing::insert_user(&mut conn, "other").await.unwrap();
    let member = testing::insert_person_with_home(&mut conn, "member", at)
        .await
        .unwrap();
    let org: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(member)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    // The host installed with the server, as 0.1.3 enrolled it.
    let bundled = enrolled(&db, first, None, true, "orch-node", at).await;
    // Hosts whose Use row somebody wrote to since: theirs to keep as it is.
    let shared = enrolled(&db, first, None, true, "shared", at).await;
    let cleared = enrolled(&db, first, None, true, "cleared", at).await;
    {
        let mut conn = db.pool.acquire().await.unwrap();
        let page = PageKey::new("host", shared, "live");
        name_in(&mut conn, &page, "host.use", other, first, at).await;
        let page = PageKey::new("host", cleared, "live");
        let named = name_in(&mut conn, &page, "host.use", other, first, at).await;
        sqlx::query("UPDATE permission_entries SET removed_at=$2, removed_by=$3 WHERE id=$1")
            .bind(named)
            .bind(at)
            .bind(first)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    // A bundled host handed over to another owner, an organization's host made "only me" (owning it counts there),
    // and a host of the system that was not installed with the server.
    let handed = enrolled(&db, first, None, true, "handed", at).await;
    sqlx::query("UPDATE hosts SET owner_id=$2 WHERE id=$1")
        .bind(handed)
        .bind(other)
        .execute(&db.pool)
        .await
        .unwrap();
    let homed = enrolled(&db, member, Some(org), false, "homed", at).await;
    let added = enrolled(&db, first, None, false, "added", at).await;

    update(&db).await;
    assert_eq!(
        use_row(&db, bundled).await,
        [(Some(first), "creation".to_owned(), Some(first), at)],
        "named as its maker, from when it enrolled, as enrollment now names them"
    );
    let cut_loose: bool = sqlx::query_scalar(
        "SELECT NOT inherit FROM permission_rows WHERE object_kind='host' AND object_id=$1 AND page='live' AND permission='host.use'",
    )
    .bind(bundled)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(cut_loose, "the row stays cut loose: nobody else is named");
    assert_eq!(
        use_row(&db, shared).await,
        [(Some(other), "page".to_owned(), Some(first), at)]
    );
    for untouched in [cleared, handed, homed, added] {
        assert!(use_row(&db, untouched).await.is_empty());
    }
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries")
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // Applied again, as after an update that stopped half-way: nothing more.
    update(&db).await;
    assert_eq!(use_row(&db, bundled).await.len(), 1);
    let again: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(again, entries);
}
