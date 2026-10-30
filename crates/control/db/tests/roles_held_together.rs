//! Roles held together at one place: a lasting role and others that end, one grant for each role, the strongest in
//! force read as the role held there, and the lasting one left when one that ends has lapsed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{Duration, Utc};
use orch_db::DbError;
use orch_db::queries::access::{self, NewRole};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};

fn grant<'a>(
    node: Id,
    user: Id,
    role: &'a str,
    ends: Option<chrono::DateTime<Utc>>,
    by: Id,
) -> NewRole<'a> {
    NewRole {
        id: new_id(),
        node,
        subject_kind: "user",
        subject: user,
        subject_role: None,
        role,
        expires_at: ends,
        note: "",
        by,
        at: Utc::now(),
    }
}

#[tokio::test]
async fn a_lasting_viewer_and_a_developer_until_tomorrow_are_held_together_and_the_viewer_stays() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let (ada, bo, acme) = (new_id(), new_id(), new_id());
    insert_user(&mut conn, ada, "ada").await.unwrap();
    insert_user(&mut conn, bo, "bob").await.unwrap();
    insert_node(&mut conn, acme, "org", None, "Acme", ada)
        .await
        .unwrap();

    access::put_role(&mut conn, &grant(acme, bo, "viewer", None, ada))
        .await
        .unwrap();
    let tomorrow = Utc::now() + Duration::days(1);
    access::put_role(
        &mut conn,
        &grant(acme, bo, "developer", Some(tomorrow), ada),
    )
    .await
    .unwrap();
    assert_eq!(
        access::role_at(&mut conn, acme, bo)
            .await
            .unwrap()
            .as_deref(),
        Some("developer"),
        "the strongest role in force is the one held"
    );

    // The same role twice, or a second lasting role, is a role held already.
    for again in [
        grant(
            acme,
            bo,
            "developer",
            Some(tomorrow + Duration::days(1)),
            ada,
        ),
        grant(acme, bo, "interactor", None, ada),
    ] {
        assert!(
            matches!(
                access::put_role(&mut conn, &again).await,
                Err(DbError::Unique { .. })
            ),
            "{}",
            again.role
        );
    }

    // The developer's end comes: the lasting viewer is what is left.
    sqlx::query("UPDATE node_roles SET expires_at = now() - interval '1 minute' WHERE subject_id = $1 AND role = 'developer'")
        .bind(bo)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        access::role_at(&mut conn, acme, bo)
            .await
            .unwrap()
            .as_deref(),
        Some("viewer")
    );
    // A role that ended no longer stands in the way of giving it again.
    access::put_role(
        &mut conn,
        &grant(acme, bo, "developer", Some(tomorrow), ada),
    )
    .await
    .unwrap();
    assert_eq!(
        access::role_at(&mut conn, acme, bo)
            .await
            .unwrap()
            .as_deref(),
        Some("developer")
    );
}
