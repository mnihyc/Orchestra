//! Participation keeps descendant assignments, exact deadlines and transactional moves without reading the owned inventory.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{Duration, TimeZone, Utc};
use orch_db::{
    Conn,
    queries::{
        access::{self, PageKey},
        hierarchy::testing::insert_node,
        identity::testing::insert_user,
    },
};
use orch_testkit::database::TestDb;
use orch_types::{Id, Time, new_id};
use std::collections::BTreeSet;

struct Fixture {
    person: Id,
    root: Id,
    branch: Id,
    deep: Id,
    destination: Id,
    document: Id,
    at: Time,
}
#[derive(Default)]
struct Focus {
    groups: Vec<Id>,
    members: Option<Vec<Id>>,
    closed: Vec<Id>,
    subtree: Option<Id>,
    at: Option<Time>,
}

async fn fixture(conn: &mut Conn) -> Fixture {
    let [person, root, branch, deep, destination, document] = std::array::from_fn(|_| new_id());
    insert_user(conn, person, "reader").await.unwrap();
    insert_node(conn, root, "org", None, "Root", person)
        .await
        .unwrap();
    insert_node(conn, branch, "topic", Some(root), "Branch", person)
        .await
        .unwrap();
    insert_node(conn, deep, "topic", Some(branch), "Deep", person)
        .await
        .unwrap();
    insert_node(
        conn,
        destination,
        "topic",
        Some(root),
        "Destination",
        person,
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,basename) VALUES($1,$2,$3,$4,$4,'A document','document.md')")
        .bind(document).bind(deep).bind(root).bind(person).execute(&mut *conn).await.unwrap();
    Fixture {
        person,
        root,
        branch,
        deep,
        destination,
        document,
        at: Time::new(Utc.with_ymd_and_hms(2026, 10, 30, 0, 0, 0).unwrap()),
    }
}

async fn marks(conn: &mut Conn, f: &Fixture, focus: &Focus) -> BTreeSet<(String, Id)> {
    let nodes = [f.root, f.branch, f.deep, f.destination];
    let mut pages: Vec<_> = nodes
        .iter()
        .map(|id| PageKey::new("node", *id, "live"))
        .collect();
    pages.push(PageKey::new("document", f.document, "live"));
    access::participation::marks(
        conn,
        f.person,
        &focus.groups,
        &[f.root],
        focus.at.unwrap_or(f.at),
        &nodes,
        &pages,
        focus.members.as_deref().unwrap_or(&[f.root]),
        &focus.closed,
        focus.subtree,
        &[f.root],
    )
    .await
    .unwrap()
    .into_iter()
    .map(|m| (m.kind, m.id))
    .collect()
}
fn expected(f: &Fixture, parent: Id) -> BTreeSet<(String, Id)> {
    [
        ("node", f.root),
        ("node", parent),
        ("node", f.deep),
        ("document", f.document),
    ]
    .into_iter()
    .map(|(kind, id)| (kind.to_owned(), id))
    .collect()
}

#[tokio::test]
async fn descendant_assignments_keep_membership_group_time_closed_and_move_semantics() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let f = fixture(&mut conn).await;
    let owner = new_id();
    sqlx::query("INSERT INTO permission_entries(id,object_kind,object_id,page,permission,holder_kind,holder_id,source) VALUES($1,'document',$2,'live','access.own','user',$3,'creation')")
        .bind(owner).bind(f.document).bind(f.person).execute(&mut *conn).await.unwrap();
    assert_eq!(
        marks(&mut conn, &f, &Focus::default()).await,
        expected(&f, f.branch)
    );
    let outsider = Focus {
        members: Some(vec![]),
        ..Focus::default()
    };
    assert!(
        marks(&mut conn, &f, &outsider).await.is_empty(),
        "Owners still need membership"
    );
    let [group, grant] = std::array::from_fn(|_| new_id());
    sqlx::query("INSERT INTO permission_entries(id,object_kind,object_id,page,permission,holder_kind,holder_id,source,expires_at) VALUES($1,'document',$2,'live','doc.view','group',$3,'page',$4)")
        .bind(grant).bind(f.document).bind(group).bind(f.at.get()+Duration::microseconds(1)).execute(&mut *conn).await.unwrap();
    let grouped = Focus {
        groups: vec![group],
        members: Some(vec![]),
        ..Focus::default()
    };
    assert_eq!(marks(&mut conn, &f, &grouped).await, expected(&f, f.branch));
    assert!(
        marks(
            &mut conn,
            &f,
            &Focus {
                at: Some(Time::new(f.at.get() + Duration::microseconds(1))),
                groups: vec![group],
                members: Some(vec![]),
                ..Focus::default()
            }
        )
        .await
        .is_empty(),
        "the exact expiry instant is excluded"
    );
    assert!(
        marks(
            &mut conn,
            &f,
            &Focus {
                closed: vec![f.branch],
                ..Focus::default()
            }
        )
        .await
        .is_empty()
    );
    assert!(
        marks(
            &mut conn,
            &f,
            &Focus {
                subtree: Some(f.destination),
                ..Focus::default()
            }
        )
        .await
        .is_empty()
    );
    assert_eq!(
        marks(
            &mut conn,
            &f,
            &Focus {
                subtree: Some(f.deep),
                ..Focus::default()
            }
        )
        .await,
        expected(&f, f.branch)
    );
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
        .bind(grant)
        .bind(f.at.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(marks(&mut conn, &f, &grouped).await.is_empty());
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(f.branch)
        .bind(f.at.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(marks(&mut conn, &f, &Focus::default()).await.is_empty());
    sqlx::query("UPDATE nodes SET deleted_at=NULL WHERE id=$1")
        .bind(f.branch)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        marks(&mut conn, &f, &Focus::default()).await,
        expected(&f, f.branch)
    );
    sqlx::query("UPDATE documents SET deleted_at=$2 WHERE id=$1")
        .bind(f.document)
        .bind(f.at.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(marks(&mut conn, &f, &Focus::default()).await.is_empty());
    sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=$1")
        .bind(f.document)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE nodes SET parent_id=$2,path=ARRAY[$3,$2,$1] WHERE id=$1")
        .bind(f.deep)
        .bind(f.destination)
        .bind(f.root)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        marks(&mut conn, &f, &Focus::default()).await,
        expected(&f, f.destination)
    );
    sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
    assert_eq!(
        marks(&mut conn, &f, &Focus::default()).await,
        expected(&f, f.branch)
    );
    sqlx::query("DELETE FROM permission_entries WHERE id=$1")
        .bind(owner)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(marks(&mut conn, &f, &Focus::default()).await.is_empty());
}

#[tokio::test]
async fn time_prefixes_are_an_exact_strict_future_cover_including_negative_and_infinite_deadlines()
{
    let db = TestDb::fresh().await;
    let exact: bool = sqlx::query_scalar("WITH times AS (
       SELECT to_timestamp(i)::timestamptz at FROM generate_series(-64,64)i
       UNION ALL SELECT unnest(ARRAY['-infinity','infinity','2026-10-30 00:00:00.000001+00','2500-01-01+00']::timestamptz[])
      ), deadlines AS (SELECT at FROM times UNION ALL SELECT NULL::timestamptz)
      SELECT bool_and((SELECT count(*) FROM orch_participation_time_keys(d.at) k
       JOIN orch_participation_after_keys(t.at) a USING(time_shift,time_bucket))
       =CASE WHEN d.at IS NULL OR d.at>t.at THEN 1 ELSE 0 END) FROM times t CROSS JOIN deadlines d")
        .fetch_one(&db.pool).await.unwrap();
    assert!(
        exact,
        "each live deadline has exactly one matching prefix; an expired deadline has none"
    );
}

#[tokio::test]
async fn legacy_match_pages_resolve_only_the_selected_document_at_two_inventory_sizes() {
    use orch_db::queries::{everything, search};
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let f = fixture(&mut conn).await;
    let grants = serde_json::json!({});
    let mut previous = 0;
    for size in [1000_i64, 10000_i64] {
        sqlx::query("INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,basename)
          SELECT md5('unrelated/'||i)::uuid,$1,$2,$3,$3,'Unrelated '||i,'unrelated-'||i||'.md' FROM generate_series($4::bigint,$5::bigint)i")
            .bind(f.deep).bind(f.root).bind(f.person).bind(previous+1).bind(size).execute(&mut *conn).await.unwrap();
        let raw = search::content_candidates(
            &mut conn,
            &search::Content {
                continuation: None,
                user: f.person,
                groups: &[],
                member_roots: &[],
                grants: &grants,
                at: f.at.get(),
                scope: None,
                chat: None,
                document: None,
                participating: false,
                relevance: false,
                names: false,
                with_file: false,
                mentions_me: None,
                text: "A document",
                item_kinds: &[],
                chat_messages: false,
                document_content: true,
                author: None,
                from: None,
                to: None,
                after: None,
                limit: 201,
                selected: None,
                items: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(raw.len(), 1);
        let selected = search::CandidateSet::from_content(&raw);
        assert_eq!(selected.documents, vec![f.document]);
        let seeds = everything::search_seeds(
            &mut conn,
            None,
            &selected.documents,
            &selected.chats,
            &selected.nodes,
        )
        .await
        .unwrap();
        assert_eq!(
            seeds.len(),
            2,
            "one document and its one home, independent of the unrelated inventory"
        );
        previous = size;
    }
}
