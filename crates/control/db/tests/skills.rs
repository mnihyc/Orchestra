//! A skill's name follows the skill format and belongs to one live skill of its home; each version is numbered after the last and pins its archive, a draft's files pin theirs, and removing a version's skill or a draft's file gives the pins back.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::Conn;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::resources::skills::{self, DraftFile, NewSkill, NewVersion};
use orch_db::queries::storage::register_blob;
use orch_testkit::{clock, database::TestDb};
use orch_types::{Hash, Id, new_id};
use serde_json::json;

struct World {
    test: TestDb,
    owner: Id,
    org: Id,
}

async fn world() -> World {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let owner = new_id();
    insert_user(&mut conn, owner, "ada").await.unwrap();
    let org = new_id();
    insert_node(&mut conn, org, "org", None, "Acme", owner)
        .await
        .unwrap();
    World { test, owner, org }
}

/// A content hash made of one byte, registered as a stored blob.
async fn blob(conn: &mut Conn, byte: u8) -> Hash {
    let hash = Hash::from_bytes([byte; 32]);
    register_blob(conn, &hash, 100, clock::start())
        .await
        .unwrap();
    hash
}

async fn pins(conn: &mut Conn, hash: &Hash) -> i64 {
    sqlx::query_scalar("SELECT pin_count::bigint FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(conn)
        .await
        .unwrap()
}

async fn skill(conn: &mut Conn, w: &World, name: &str) -> Result<Id, orch_db::DbError> {
    let id = new_id();
    skills::insert(
        conn,
        &NewSkill {
            id,
            home_id: w.org,
            owner_id: w.owner,
            name: name.to_owned(),
        },
        clock::start(),
    )
    .await?;
    Ok(id)
}

fn version(skill: Id, archive: Hash) -> NewVersion {
    NewVersion {
        id: new_id(),
        skill_id: skill,
        name: "pdf-tools".to_owned(),
        description: "Reads PDF forms.".to_owned(),
        archive,
        archive_size: 100,
        files: json!([{"path": "SKILL.md", "size": 40, "sha256": "00", "executable": false}]),
        size: 40,
        origin: json!({"kind": "upload", "name": "pdf-tools.zip", "folder": null}),
        keeps_origin: false,
        created_by: None,
    }
}

#[tokio::test]
async fn a_name_outside_the_skill_format_is_refused_by_the_table() {
    let w = world().await;
    let mut conn = w.test.pool.acquire().await.unwrap();
    let (longest, too_long) = ("a".repeat(64), "b".repeat(65));
    for name in ["pdf-tools", "a", "x9", longest.as_str()] {
        assert!(skill(&mut conn, &w, name).await.is_ok(), "{name}");
    }
    for name in [
        "",
        "PDF",
        "pdf_tools",
        "-pdf",
        "pdf-",
        "pdf--tools",
        "pdf tools",
        too_long.as_str(),
    ] {
        assert!(skill(&mut conn, &w, name).await.is_err(), "{name:?}");
    }
}

#[tokio::test]
async fn a_home_holds_one_live_skill_of_a_name() {
    let w = world().await;
    let mut conn = w.test.pool.acquire().await.unwrap();
    let first = skill(&mut conn, &w, "pdf-tools").await.unwrap();
    assert!(skill(&mut conn, &w, "pdf-tools").await.is_err());
    assert_eq!(
        skills::named(&mut conn, w.org, "pdf-tools").await.unwrap(),
        Some(first)
    );
    skills::delete(&mut conn, first, clock::start())
        .await
        .unwrap();
    assert_eq!(
        skills::named(&mut conn, w.org, "pdf-tools").await.unwrap(),
        None
    );
    let second = skill(&mut conn, &w, "pdf-tools").await.unwrap();
    assert_ne!(first, second);
}

#[tokio::test]
async fn versions_are_numbered_in_turn_and_pin_their_archive_until_the_skill_goes() {
    let w = world().await;
    let mut conn = w.test.pool.acquire().await.unwrap();
    let id = skill(&mut conn, &w, "pdf-tools").await.unwrap();
    let archive = blob(&mut conn, 1).await;
    let first = skills::insert_version(&mut conn, &version(id, archive), clock::start())
        .await
        .unwrap();
    let second = skills::insert_version(&mut conn, &version(id, archive), clock::start())
        .await
        .unwrap();
    assert_eq!((first.number, second.number), (1, 2));
    assert_eq!(pins(&mut conn, &archive).await, 2, "each version pins it");
    let row = skills::skills(&mut conn, &[id]).await.unwrap();
    assert_eq!(row[0].latest_version_id, Some(second.id));
    assert_eq!(row[0].description, "Reads PDF forms.");
    let page = skills::versions(&mut conn, id, None, 1).await.unwrap();
    assert_eq!(page.iter().map(|v| v.number).collect::<Vec<_>>(), [2]);
    let rest = skills::versions(&mut conn, id, Some(2), 10).await.unwrap();
    assert_eq!(rest.iter().map(|v| v.number).collect::<Vec<_>>(), [1]);
    assert_eq!(
        skills::version_counts(&mut conn, &[id]).await.unwrap(),
        [(id, 2)]
    );
    assert_eq!(first.archive_hash().unwrap(), archive);
    sqlx::query("DELETE FROM skills WHERE id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        pins(&mut conn, &archive).await,
        0,
        "a purge gives the pins back"
    );
}

#[tokio::test]
async fn a_drafts_files_pin_their_content_and_move_the_pin_when_replaced() {
    let w = world().await;
    let mut conn = w.test.pool.acquire().await.unwrap();
    let id = skill(&mut conn, &w, "pdf-tools").await.unwrap();
    let (text, script, other) = (
        blob(&mut conn, 2).await,
        blob(&mut conn, 3).await,
        blob(&mut conn, 4).await,
    );
    let file = |path: &str, blob: Hash, executable: bool| DraftFile {
        path: path.to_owned(),
        blob,
        size: 10,
        executable,
    };
    skills::start_draft(
        &mut conn,
        id,
        None,
        &[
            file("SKILL.md", text, false),
            file("scripts/run.sh", script, true),
            file("Z.md", text, false),
        ],
        w.owner,
        clock::start(),
    )
    .await
    .unwrap();
    assert_eq!(pins(&mut conn, &text).await, 2);
    assert_eq!(pins(&mut conn, &script).await, 1);
    let paths: Vec<String> = skills::draft_files(&mut conn, id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.path)
        .collect();
    assert_eq!(paths, ["SKILL.md", "Z.md", "scripts/run.sh"], "byte order");
    let before = skills::lock_draft(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(before.file_count, 3);

    skills::put_draft_file(
        &mut conn,
        id,
        &file("scripts/run.sh", other, true),
        w.owner,
        clock::start(),
    )
    .await
    .unwrap();
    assert_eq!(
        pins(&mut conn, &script).await,
        0,
        "the replaced content is let go"
    );
    assert_eq!(pins(&mut conn, &other).await, 1);
    let after = skills::lock_draft(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(after.revision, before.revision + 1);

    assert!(
        skills::delete_draft_file(&mut conn, id, "Z.md", w.owner, clock::start())
            .await
            .unwrap()
    );
    assert!(
        !skills::delete_draft_file(&mut conn, id, "Z.md", w.owner, clock::start())
            .await
            .unwrap()
    );
    assert_eq!(pins(&mut conn, &text).await, 1);
    assert!(skills::discard_draft(&mut conn, id).await.unwrap());
    assert_eq!(pins(&mut conn, &text).await, 0);
    assert_eq!(pins(&mut conn, &other).await, 0);
    assert!(skills::lock_draft(&mut conn, id).await.unwrap().is_none());
}
