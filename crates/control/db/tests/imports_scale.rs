//! What an import made, at the size the owner asked for: a hundred thousand places, report rows, messages and items made by one import are
//! counted by a few statements, paged or taken back in batches of what a page holds, none of the statements reads a whole table, and the
//! pages together are every row once. What is asserted is how much each call returns and how many calls it takes, never the time.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::Conn;
use orch_db::queries::chats::{self, ChatRow};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::imports::{
    self, EMPTY_FOLDERS_SQL, NODE_IS_CLEAR_SQL, NODES_PAGE_SQL, REPORT_ROWS_SQL, ReportFilter,
    SIBLING_DOCUMENTS_TOP_SQL, SIBLING_FOLDERS_TOP_SQL,
};
use orch_db::queries::imports_chats as extra;
use orch_db::queries::imports_threads as thread_rows;
use orch_db::queries::threads;
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use serde_json::Value;
use std::collections::HashSet;

/// How many places the one import made: a hundred thousand, by one statement each for the places and for what says they were made.
const PLACES: i64 = 100_000;
/// How many a page holds.
const PAGE: i64 = 1_000;
/// How many rows of its report an import wrote.
const REPORT_ROWS: i64 = 100_000;
/// How many messages and items an import added to a chat and a thread that were there.
const ADDED: i64 = 100_000;
/// How many rows one batch of taking back may take.
const BATCH: i64 = 500;

struct World {
    project: Id,
    import: Id,
}

async fn world(conn: &mut Conn) -> World {
    let (ada, acme, project, import) = (new_id(), new_id(), new_id(), new_id());
    insert_user(conn, ada, "ada").await.unwrap();
    insert_node(conn, acme, "org", None, "Acme", ada)
        .await
        .unwrap();
    insert_node(conn, project, "project", Some(acme), "Web", ada)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO imports (id, user_id, node_id, client_id, request, state) VALUES ($1, $2, $3, 'scale', '{}'::jsonb, 'done')",
    )
    .bind(import)
    .bind(ada)
    .bind(project)
    .execute(&mut *conn)
    .await
    .unwrap();
    // The places, by SQL and not one at a time.
    sqlx::query(
        "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by)
         SELECT g.id, 'topic', p.id, p.org_id, p.path || g.id, 'Topic ' || g.n, $2, $2
           FROM (SELECT gen_random_uuid() AS id, n FROM generate_series(1, $3) AS n) g, nodes p WHERE p.id = $1",
    )
    .bind(project)
    .bind(ada)
    .bind(PLACES)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO topics (node_id) SELECT id FROM nodes WHERE parent_id = $1 AND kind = 'topic'",
    )
    .bind(project)
    .execute(&mut *conn)
    .await
    .unwrap();
    // What says the import made them.
    sqlx::query(
        "INSERT INTO import_origins (scope_id, source_id, kind, object_id, parent_id, created_by_import, last_import, source_name, imported_by, imported_at)
         SELECT $1, gen_random_uuid(), 'topic', n.id, $1, $2, $2, n.name, $3, now() FROM nodes n WHERE n.parent_id = $1 AND n.kind = 'topic'",
    )
    .bind(project)
    .bind(import)
    .bind(ada)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ANALYZE import_origins")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ANALYZE nodes")
        .execute(&mut *conn)
        .await
        .unwrap();
    World { project, import }
}

/// A person, an org, a project, a topic that is not the import's, and an import into the project that is done.
struct Site {
    ada: Id,
    acme: Id,
    project: Id,
    topic: Id,
    import: Id,
}

async fn site(conn: &mut Conn) -> Site {
    let (ada, acme, project, topic, import) = (new_id(), new_id(), new_id(), new_id(), new_id());
    insert_user(conn, ada, "ada").await.unwrap();
    insert_node(conn, acme, "org", None, "Acme", ada)
        .await
        .unwrap();
    insert_node(conn, project, "project", Some(acme), "Web", ada)
        .await
        .unwrap();
    insert_node(conn, topic, "topic", Some(project), "Notes", ada)
        .await
        .unwrap();
    // insert_node creates the topic row and its node chat as well.
    sqlx::query(
        "INSERT INTO imports (id, user_id, node_id, client_id, request, state) VALUES ($1, $2, $3, 'scale', '{}'::jsonb, 'done')",
    )
    .bind(import)
    .bind(ada)
    .bind(project)
    .execute(&mut *conn)
    .await
    .unwrap();
    Site {
        ada,
        acme,
        project,
        topic,
        import,
    }
}

/// How many times a plan reads a whole table.
fn whole_reads(plan: &Value, relation: &str) -> usize {
    let mut found = 0;
    match plan {
        Value::Array(items) => {
            for item in items {
                found += whole_reads(item, relation);
            }
        }
        Value::Object(map) => {
            if map.get("Node Type").and_then(Value::as_str) == Some("Seq Scan")
                && map.get("Relation Name").and_then(Value::as_str) == Some(relation)
            {
                found += 1;
            }
            for member in map.values() {
                found += whole_reads(member, relation);
            }
        }
        _ => {}
    }
    found
}

/// The plan of a statement whose parameters are written in as `values` (`$1` first), as the planner makes it on the table as it is.
async fn plan_of(conn: &mut Conn, statement: &str, values: &[String]) -> Value {
    let mut text = statement.to_owned();
    for (at, value) in values.iter().enumerate().rev() {
        let number = at + 1;
        text = text
            .replace(&format!("${number}::int"), &format!("{value}::int"))
            .replace(&format!("${number}::uuid"), &format!("{value}::uuid"))
            .replace(&format!("${number}::text"), &format!("{value}::text"))
            .replace(&format!("${number}"), value);
    }
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("EXPLAIN (FORMAT JSON) {text}")))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

/// As [`plan_of`], with a whole read of a table forbidden to the planner: on a table that holds nothing but the rows asked for it would
/// choose one, so what is asked is whether the statement CAN be served by an index, which on a table of other imports' rows is how it is.
async fn plan_without_whole_reads(conn: &mut Conn, statement: &str, values: &[String]) -> Value {
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .unwrap();
    let plan = plan_of(conn, statement, values).await;
    sqlx::query("RESET enable_seqscan")
        .execute(&mut *conn)
        .await
        .unwrap();
    plan
}

#[tokio::test]
async fn what_an_import_made_is_counted_without_loading_it_and_paged_by_identifier_through_the_index()
 {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let world = world(&mut conn).await;

    // Counted by a few statements: a hundred thousand places are one row of the answer.
    let counts = imports::counts_made(&mut conn, world.import).await.unwrap();
    assert_eq!(counts, vec![("topic".to_owned(), PLACES)]);
    assert_eq!(
        imports::added_since(&mut conn, world.import, chrono::Utc::now())
            .await
            .unwrap(),
        0
    );

    // Paged: no page holds more than it was asked for, the identifiers only go up, and the pages together are every place once.
    let mut seen: HashSet<Id> = HashSet::new();
    let mut after: Option<(i32, Id)> = None;
    let mut pages = 0_i64;
    loop {
        let page = imports::nodes_page(&mut conn, world.import, after, PAGE)
            .await
            .unwrap();
        assert!(i64::try_from(page.len()).unwrap() <= PAGE);
        let Some((last, _, depth)) = page.last() else {
            break;
        };
        if let Some((before_depth, before)) = after {
            assert!(
                page.iter()
                    .all(|(id, _, each)| (*each, *id) < (before_depth, before)),
                "a page goes on from where the one before ended, the deepest first"
            );
        }
        after = Some((*depth, *last));
        pages += 1;
        for (id, _, _) in &page {
            assert!(seen.insert(*id), "a place is in one page only");
        }
    }
    assert_eq!(i64::try_from(seen.len()).unwrap(), PLACES);
    assert_eq!(pages, PLACES / PAGE, "every page was full");

    // The index can serve a page: the table of what imports made is never read whole.
    let plan = plan_without_whole_reads(
        &mut conn,
        NODES_PAGE_SQL,
        &[
            format!("'{}'", world.import),
            "NULL".to_owned(),
            "NULL".to_owned(),
            PAGE.to_string(),
        ],
    )
    .await;
    assert_eq!(
        whole_reads(&plan, "import_origins"),
        0,
        "a page of what an import made is read through the index: {plan}"
    );
    let _ = world.project;
}

#[tokio::test]
async fn a_report_of_a_hundred_thousand_rows_is_read_a_page_at_a_time_through_the_primary_key() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let site = site(&mut conn).await;
    sqlx::query(
        "INSERT INTO import_report_rows (import_id, position, source_id, kind, outcome, warned, row)
         SELECT $1, g, gen_random_uuid(), 'document', 'created', g % 100 = 0, jsonb_build_object('position', g)
           FROM generate_series(0::bigint, $2 - 1) AS g",
    )
    .bind(site.import)
    .bind(REPORT_ROWS)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ANALYZE import_report_rows")
        .execute(&mut *conn)
        .await
        .unwrap();

    // Counted by two statements, without reading a row into the process.
    let (by_outcome, warned) = imports::report_counts(&mut conn, site.import)
        .await
        .unwrap();
    assert_eq!(by_outcome, vec![("created".to_owned(), REPORT_ROWS)]);
    assert_eq!(warned, REPORT_ROWS / 100);

    // Paged: a page holds at most what it was asked for, the positions only go up, and the pages together are every row once.
    let (mut after, mut pages, mut seen) = (None::<i64>, 0_i64, 0_i64);
    loop {
        let page = imports::report_rows(
            &mut conn,
            site.import,
            &ReportFilter::default(),
            after,
            PAGE,
        )
        .await
        .unwrap();
        assert!(i64::try_from(page.len()).unwrap() <= PAGE);
        let Some((last, _)) = page.last() else {
            break;
        };
        assert!(page.windows(2).all(|pair| pair[0].0 < pair[1].0));
        if let Some(before) = after {
            assert!(
                page[0].0 > before,
                "a page goes on after where the one before ended"
            );
        }
        after = Some(*last);
        pages += 1;
        seen += i64::try_from(page.len()).unwrap();
    }
    assert_eq!((pages, seen), (REPORT_ROWS / PAGE, REPORT_ROWS));

    // The rows with a warning are paged the same way, and the answer is the rows with a warning only.
    let only_warned = ReportFilter {
        warned: true,
        ..ReportFilter::default()
    };
    let (mut after, mut pages, mut seen) = (None::<i64>, 0_i64, 0_i64);
    loop {
        let page = imports::report_rows(&mut conn, site.import, &only_warned, after, 100)
            .await
            .unwrap();
        let Some((last, _)) = page.last() else {
            break;
        };
        assert!(page.iter().all(|(position, _)| position % 100 == 0));
        after = Some(*last);
        pages += 1;
        seen += i64::try_from(page.len()).unwrap();
    }
    assert_eq!((pages, seen), (REPORT_ROWS / 100 / 100, REPORT_ROWS / 100));

    // A page is read through the primary key, never by reading the whole report.
    let plan = plan_without_whole_reads(
        &mut conn,
        REPORT_ROWS_SQL,
        &[
            format!("'{}'", site.import),
            "-1".to_owned(),
            "NULL".to_owned(),
            "NULL".to_owned(),
            "false".to_owned(),
            PAGE.to_string(),
        ],
    )
    .await;
    assert_eq!(
        whole_reads(&plan, "import_report_rows"),
        0,
        "a page of a report is read through its key: {plan}"
    );
}

#[tokio::test]
async fn what_an_import_added_to_a_chat_is_taken_back_five_hundred_at_a_time_and_forgotten() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let site = site(&mut conn).await;
    let chat = chats::chat_of_node(&mut conn, site.topic)
        .await
        .unwrap()
        .unwrap()
        .id;
    // A hundred thousand messages the import added, and the rows that say so, by SQL.
    sqlx::query(
        "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, search_text, seq_created, seq_updated)
         SELECT gen_random_uuid(), $1, 'message', $2, '[{\"type\":\"text\",\"text\":\"hello\"}]'::jsonb, 'hello', g, g
           FROM generate_series(1::bigint, $3) AS g",
    )
    .bind(chat)
    .bind(site.ada)
    .bind(ADDED)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO import_origins (scope_id, source_id, kind, object_id, parent_id, created_by_import, last_import, imported_by, imported_at)
         SELECT $1, gen_random_uuid(), 'message', m.id, m.chat_id, $2, $2, $3, now() FROM chat_messages m WHERE m.chat_id = $4",
    )
    .bind(site.project)
    .bind(site.import)
    .bind(site.ada)
    .bind(chat)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE chats SET next_seq = $2 WHERE id = $1")
        .bind(chat)
        .bind(ADDED + 1)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ANALYZE chat_messages")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ANALYZE import_origins")
        .execute(&mut *conn)
        .await
        .unwrap();

    // Counted, and the chat it was added to is found, without reading a message.
    let counts = imports::counts_made(&mut conn, site.import).await.unwrap();
    assert_eq!(counts, vec![("message".to_owned(), ADDED)]);
    let parents = imports::parents_with_added(&mut conn, site.import, "message", None, 50)
        .await
        .unwrap();
    assert_eq!(parents, vec![chat]);

    // Taken back a batch at a time: no batch holds more than the batch, each is deleted by one statement for all of it, and the rows that
    // said the import added them are gone with them, so that the next batch is the next five hundred.
    let (mut batches, mut taken) = (0_i64, 0_i64);
    let at = chrono::Utc::now();
    loop {
        let added = extra::added_to_chat(&mut conn, site.import, chat, BATCH)
            .await
            .unwrap();
        if added.is_empty() {
            break;
        }
        assert!(i64::try_from(added.len()).unwrap() <= BATCH);
        let looked: Vec<Id> = added.iter().map(|(message, _)| *message).collect();
        let live: Vec<Id> = added
            .iter()
            .filter(|(_, there)| *there)
            .map(|(message, _)| *message)
            .collect();
        let count = i64::try_from(live.len()).unwrap();
        let first = extra::take_seqs(&mut conn, chat, count)
            .await
            .unwrap()
            .unwrap();
        let seqs: Vec<i64> = (first..first + count).collect();
        let said = extra::delete_messages(&mut conn, chat, &live, &seqs, site.ada, at)
            .await
            .unwrap();
        assert_eq!(
            said.len(),
            live.len(),
            "each message deleted says what it said"
        );
        extra::unpin_blobs(&mut conn, &[]).await.unwrap();
        imports::forget_objects(&mut conn, site.import, &looked)
            .await
            .unwrap();
        batches += 1;
        taken += i64::try_from(looked.len()).unwrap();
    }
    assert_eq!((batches, taken), (ADDED / BATCH, ADDED));
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE chat_id = $1 AND deleted_at IS NULL",
    )
    .bind(chat)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(left, 0, "every message the import added is deleted");
    assert!(
        imports::counts_made(&mut conn, site.import)
            .await
            .unwrap()
            .is_empty(),
        "and the import no longer says it added them"
    );
    // The chat's own numbers went on for each deletion, so that readers who follow the chat see them.
    let row: ChatRow = chats::lock(&mut conn, chat).await.unwrap().unwrap();
    assert!(row.next_seq > ADDED + 1);
}

#[tokio::test]
async fn what_an_import_added_to_a_thread_is_taken_back_five_hundred_at_a_time_and_what_hangs_on_an_item_waits_for_it()
 {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let site = site(&mut conn).await;
    let (thread, segment) = (new_id(), new_id());
    let at = chrono::Utc::now();
    assert!(
        threads::insert_thread(&mut conn, thread, site.topic, "History", site.ada, at)
            .await
            .unwrap()
    );
    let seq = thread_rows::take_seqs(&mut conn, thread, 2)
        .await
        .unwrap()
        .unwrap();
    thread_rows::put_segment(
        &mut conn,
        &thread_rows::NewSegment {
            id: segment,
            thread,
            started_by: site.ada,
            started_at: at,
            ended_at: at,
            arrangement: "together",
            credential_path: "native",
            report: &serde_json::json!({}),
            seq: seq + 1,
        },
    )
    .await
    .unwrap();
    // A hundred thousand items the import added; every tenth is the one nine others are made under (a sub-agent's work under its
    // call), written by SQL with the triggers of the table off for the making only, so that taking them back meets all of them.
    sqlx::query("ALTER TABLE items DISABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "WITH g AS (SELECT n, gen_random_uuid() AS id FROM generate_series(1, $3::int) AS n),
              c AS (SELECT active_continuation_id AS id FROM threads WHERE node_id = $1)
         INSERT INTO items (id, thread_id, segment_id, continuation_id, parent_item_id, stream, kind, status, payload, native_ref,
                            before_orch, seq_created, seq_updated)
         SELECT g.id, $1, $2, c.id, p.id, 'main', 'agent_message', 'completed',
                '{\"text\":\"x\",\"text_len\":1,\"phase\":null}'::jsonb, 'import:' || g.id, true, g.n + 10, g.n + 10
           FROM g CROSS JOIN c LEFT JOIN g p ON (g.n - 1) % 10 <> 0 AND p.n = g.n - ((g.n - 1) % 10)",
    )
    .bind(thread)
    .bind(segment)
    .bind(i32::try_from(ADDED).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE items ENABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO import_origins (scope_id, source_id, kind, object_id, parent_id, created_by_import, last_import, imported_by, imported_at)
         SELECT $1, gen_random_uuid(), 'item', i.id, i.thread_id, $2, $2, $3, now() FROM items i WHERE i.thread_id = $4",
    )
    .bind(site.project)
    .bind(site.import)
    .bind(site.ada)
    .bind(thread)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ANALYZE items")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ANALYZE import_origins")
        .execute(&mut *conn)
        .await
        .unwrap();

    let counts = imports::counts_made(&mut conn, site.import).await.unwrap();
    assert_eq!(counts, vec![("item".to_owned(), ADDED)]);
    let parents = imports::parents_with_added(&mut conn, site.import, "item", None, 50)
        .await
        .unwrap();
    assert_eq!(parents, vec![thread]);

    // Taken back a batch at a time, a leaf before what it hangs on: no batch holds more than the batch, and no batch leaves an item
    // without the one it was made under.
    let (mut batches, mut taken) = (0_i64, 0_i64);
    loop {
        let looked = thread_rows::take_back_items(&mut conn, site.import, thread, BATCH)
            .await
            .unwrap();
        if looked == 0 {
            break;
        }
        assert!(looked <= BATCH);
        batches += 1;
        taken += looked;
    }
    assert_eq!(taken, ADDED);
    assert!(
        (ADDED / BATCH..=ADDED / BATCH + 40).contains(&batches),
        "a batch is a leaf of the items still there, so there are the batches the items fill and a few more for the ones they hung on: {batches}"
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(left, 0, "every item the import added is gone");
    assert!(
        imports::counts_made(&mut conn, site.import)
            .await
            .unwrap()
            .is_empty(),
        "and the import no longer says it added them"
    );
}

#[tokio::test]
async fn the_statements_that_look_into_a_place_are_read_through_the_indexes_of_what_it_holds() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let site = site(&mut conn).await;
    // A topic that holds twenty thousand folders and twenty thousand documents, made by SQL with the triggers of the tables off for the making
    // only, and a hundred thousand other places beside it.
    sqlx::query("ALTER TABLE document_containers DISABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE documents DISABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO document_containers (id, node_id, org_id, owner_id, name, created_at, updated_at)
         SELECT gen_random_uuid(), $1, $2, $3, 'Folder ' || g, now(), now() FROM generate_series(1, 20000) AS g",
    )
    .bind(site.topic)
    .bind(site.acme)
    .bind(site.ada)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO documents (id, node_id, org_id, owner_id, title, basename)
         SELECT gen_random_uuid(), $1, $2, $3, 'Doc ' || g, document_basename('Doc ' || g) FROM generate_series(1, 20000) AS g",
    )
    .bind(site.topic)
    .bind(site.acme)
    .bind(site.ada)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE document_containers ENABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE documents ENABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by)
         SELECT g.id, 'topic', p.id, p.org_id, p.path || g.id, 'Other ' || g.n, $2, $2
           FROM (SELECT gen_random_uuid() AS id, n FROM generate_series(1, 100000) AS n) g, nodes p WHERE p.id = $1",
    )
    .bind(site.project)
    .bind(site.ada)
    .execute(&mut *conn)
    .await
    .unwrap();
    for table in ["nodes", "documents", "document_containers"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("ANALYZE {table}")))
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    // A place is asked whether it is clear through the index of parents.
    let names = |word: &str| {
        let each: Vec<String> = (1..=50).map(|n| format!("\"{word} {n}\"")).collect();
        format!("'{{{}}}'", each.join(","))
    };
    let plan = plan_of(
        &mut conn,
        NODE_IS_CLEAR_SQL,
        &[
            format!("'{}'", site.topic),
            "'2026-01-01T00:00:00Z'".to_owned(),
        ],
    )
    .await;
    assert_eq!(
        whole_reads(&plan, "nodes"),
        0,
        "the places below a place are found through the index of parents: {plan}"
    );
    // Which folders hold nothing is asked of fifty folders through the indexes that begin with the place.
    let folders: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM document_containers WHERE node_id = $1 ORDER BY id LIMIT 50",
    )
    .bind(site.topic)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let list = format!(
        "'{{{}}}'",
        folders
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    let plan = plan_of(&mut conn, EMPTY_FOLDERS_SQL, &[list]).await;
    assert_eq!(
        whole_reads(&plan, "document_containers"),
        0,
        "folders that hold nothing are found through the indexes of the place: {plan}"
    );
    assert_eq!(
        imports::empty_folders(&mut conn, &folders)
            .await
            .unwrap()
            .len(),
        folders.len(),
        "none of them holds anything"
    );
    // The folders and the documents that have the names of a source are found by their names, a few names in twenty thousand.
    let plan = plan_of(
        &mut conn,
        SIBLING_FOLDERS_TOP_SQL,
        &[format!("'{}'", site.topic), names("Folder")],
    )
    .await;
    assert_eq!(
        whole_reads(&plan, "document_containers"),
        0,
        "folders of a name are found through the index of live names: {plan}"
    );
    let plan = plan_of(
        &mut conn,
        SIBLING_DOCUMENTS_TOP_SQL,
        &[format!("'{}'", site.topic), names("Doc")],
    )
    .await;
    assert_eq!(
        whole_reads(&plan, "documents"),
        0,
        "documents of a name are found through the index of live names: {plan}"
    );
    let asked: Vec<String> = (1..=50).map(|n| format!("doc {n}")).collect();
    let found = imports::sibling_documents(&mut conn, site.topic, None, &asked)
        .await
        .unwrap();
    assert_eq!(
        found.len(),
        50,
        "a name matches whatever the case it is written in"
    );
}
