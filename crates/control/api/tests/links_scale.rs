//! The Links lists and the graph at the size they must hold: a document cited by a hundred thousand parts of other
//! documents, a chat and a thread, and a graph over a large place. Each statement is measured by the rows and index
//! entries it reads in its own transaction, which depend on what is asked for and not on how much there is, never by
//! how long it takes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod continuation_read_support;
mod places;
mod rig;
mod support;

use orch_api_contract::common::Page;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::links::{
    DocumentGraphGet, DocumentGraphGetQuery, DocumentReferencesList, DocumentReferencesListQuery,
    LinkDirection, LinkGraph, LinkedKind, LinkedThing,
};
use orch_db::queries::links as rows;
use orch_types::{Id, new_id};
use rig::Scene;
use serde_json::json;
use sqlx::PgConnection;

/// Documents of each kind of position: ids below and above every identity issued by the clock now.
const LOW: &str = "00000000-0000-7000-8000-";
const HIGH: &str = "ffff0000-0000-7000-8000-";
/// How many documents of each position cite the hub, and how many parts of the chat and of the thread do.
const DOCUMENTS: i64 = 10_000;
const PARTS: i64 = 100_000;

struct Corpus {
    hub: Id,
    chat: Id,
    thread: Id,
    /// The newest and the oldest identity among the documents above the clock's.
    high: Option<(Id, Id)>,
}

async fn created(s: &Scene, topic: Id, title: &str) -> Id {
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&topic])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.into(),
            blocks: None,
            markdown: Some("The dates.".into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id
}

/// The hub, `DOCUMENTS` documents (twice that with `with_high`) that cite it, a chat of `PARTS` messages and a thread of
/// `PARTS` items that cite it as well, all by set-based statements (triggers and foreign keys are off for the
/// session that writes them, and the edges are written as the triggers would).
async fn corpus(s: &Scene, with_high: bool) -> Corpus {
    let topic = s.tree.topic.base.id;
    let org = s.tree.org.base.id;
    let owner = s.ada.id();
    let hub = created(s, topic, "Hub").await;
    let chat = chat_world::chat_of(&s.rig.world, &s.ada, topic).await;
    let thread = s.thread().await.base.id;
    let continuation: Id =
        sqlx::query_scalar("SELECT active_continuation_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    let segment = new_id();
    let mut conn = s.rig.db.pool().acquire().await.unwrap();
    sqlx::query("SET session_replication_role = replica")
        .execute(&mut *conn)
        .await
        .unwrap();
    let mut positions = vec![LOW];
    if with_high {
        positions.push(HIGH);
    }
    for prefix in positions {
        let block_prefix = prefix.replace("-8000-", "-9000-");
        let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
        for statement in [
            format!(
                "CREATE TEMP TABLE docs ON COMMIT DROP AS SELECT g n,('{prefix}'||lpad(to_hex(2*g+4096),12,'0'))::uuid id,
                   ('{block_prefix}'||lpad(to_hex(2*g+4096),12,'0'))::uuid block_id FROM generate_series(1,{DOCUMENTS}) g"
            ),
            format!(
                "INSERT INTO documents (id,node_id,org_id,owner_id,title,basename)
                   SELECT id,'{topic}','{org}','{owner}','Doc '||n||' '||left(id::text,4),'doc'||n||left(id::text,4) FROM docs"
            ),
            "INSERT INTO document_blocks (id,document_id,position,type,seq_created,seq_updated)
               SELECT block_id,id,1,'paragraph',1,1 FROM docs"
                .to_owned(),
            format!(
                "INSERT INTO related_references (source_kind,source_id,part_id,target_kind,target_id)
                   SELECT 'document',id,block_id,'document','{hub}' FROM docs"
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
    }
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    for statement in [
        format!(
            "CREATE TEMP TABLE parts ON COMMIT DROP AS SELECT g n,('00000000-0000-7000-a000-'||lpad(to_hex(g+4096),12,'0'))::uuid id,
               ('00000000-0000-7000-c000-'||lpad(to_hex(g+4096),12,'0'))::uuid item_id FROM generate_series(1,{PARTS}) g"
        ),
        format!(
            "INSERT INTO chat_messages (id,chat_id,content,seq_created,seq_updated)
               SELECT id,'{chat}','[]'::jsonb,n+1000000,n+1000000 FROM parts"
        ),
        format!(
            "INSERT INTO related_references (source_kind,source_id,part_id,target_kind,target_id)
               SELECT 'chat','{chat}',id,'document','{hub}' FROM parts"
        ),
        format!(
            "INSERT INTO items (id,thread_id,segment_id,kind,status,payload,seq_created,seq_updated,continuation_id)
               SELECT item_id,'{thread}','{segment}',CASE WHEN n%2=0 THEN 'user_message' ELSE 'agent_message' END,
                      'completed','{{\"text\":\"x\"}}'::jsonb,n+1000000,n+1000000,'{continuation}' FROM parts"
        ),
        format!(
            "INSERT INTO related_references (source_kind,source_id,part_id,target_kind,target_id)
               SELECT 'conversation','{thread}',item_id,'document','{hub}' FROM parts"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    sqlx::query("SET session_replication_role = DEFAULT")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ANALYZE").execute(&mut *conn).await.unwrap();
    let high = with_high.then(|| {
        (
            Id::parse_str(&format!("{HIGH}{:012x}", 2 * DOCUMENTS + 4096)).unwrap(),
            Id::parse_str(&format!("{HIGH}{:012x}", 2 + 4096)).unwrap(),
        )
    });
    Corpus {
        hub,
        chat,
        thread,
        high,
    }
}

/// The rows of tables, and the entries of indexes, that this transaction has read so far.
async fn read(conn: &mut PgConnection) -> (i64, i64) {
    sqlx::query_as(
        "SELECT coalesce(sum(pg_stat_get_xact_tuples_returned(c.oid)) FILTER (WHERE c.relkind='r'),0)::bigint,
                coalesce(sum(pg_stat_get_xact_tuples_returned(c.oid)) FILTER (WHERE c.relkind='i'),0)::bigint
           FROM pg_class c WHERE c.relnamespace='public'::regnamespace AND c.relkind IN ('r','i')",
    )
    .fetch_one(conn)
    .await
    .unwrap()
}

/// What a step read in rows and in index entries: the sum of both, since either would grow with the data.
async fn cost<T>(
    conn: &mut PgConnection,
    step: impl AsyncFnOnce(&mut PgConnection) -> T,
) -> (T, i64) {
    let (rows_before, entries_before) = read(conn).await;
    let result = step(&mut *conn).await;
    let (rows_after, entries_after) = read(conn).await;
    (
        result,
        (rows_after - rows_before) + (entries_after - entries_before),
    )
}

fn scopes(s: &Scene) -> serde_json::Value {
    json!({"scopes":[s.tree.org.base.id],"documents":[],"chats":[],"groups":[]})
}

#[tokio::test]
async fn the_lists_and_the_graph_read_what_is_asked_for_however_often_a_document_is_cited() {
    let s = Scene::new().await;
    let c = corpus(&s, true).await;
    let (high_newest, high_oldest) = c.high.unwrap();
    let mut tx = s.rig.db.pool().begin().await.unwrap();
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM related_references WHERE target_id=$1")
            .bind(c.hub)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(total > 200_000, "the hub is cited {total} times");

    // The sources, newest first, are found one probe each: a page that crosses the chat of a hundred thousand
    // messages and the thread of as many items reads a few entries per source, not a hundred thousand.
    let (top, spent) = cost(&mut tx, async |conn| {
        rows::raw_sources(conn, c.hub, None, 100).await.unwrap()
    })
    .await;
    assert_eq!(top.len(), 100);
    assert!(
        top.iter().all(|id| *id >= high_oldest),
        "the newest are the documents above the clock"
    );
    assert!(spent <= 400, "the first page read {spent}");
    let (across, spent) = cost(&mut tx, async |conn| {
        rows::raw_sources(conn, c.hub, Some(high_oldest), 100)
            .await
            .unwrap()
    })
    .await;
    assert_eq!(across.len(), 100);
    assert!(
        across.contains(&c.chat) && across.contains(&c.thread),
        "the page crosses both heavy sources"
    );
    assert!(
        across.windows(2).all(|pair| pair[0] > pair[1]),
        "newest first, each once"
    );
    assert!(
        spent <= 400,
        "the page across the heavy sources read {spent}"
    );
    let _ = high_newest;

    // Which of them are alive and reachable.
    let (live, spent) = cost(&mut tx, async |conn| {
        rows::live_sources(conn, c.hub, &across, &scopes(&s))
            .await
            .unwrap()
    })
    .await;
    assert!(live.iter().any(|source| source.source_id == c.thread));
    assert!(spent <= 4_000, "checking one page of sources read {spent}");

    // Their mentions: at most a page of each, counted up to a limit, in the order the parts were written.
    let heavy: Vec<rows::LiveSource> = live
        .iter()
        .filter(|source| source.source_id == c.chat || source.source_id == c.thread)
        .cloned()
        .collect();
    assert_eq!(heavy.len(), 2);
    let (parts, spent) = cost(&mut tx, async |conn| {
        rows::parts(conn, c.hub, &heavy, 6, None).await.unwrap()
    })
    .await;
    assert_eq!(parts.len(), 12);
    assert!(
        spent <= 200,
        "six mentions of two heavy sources read {spent}"
    );
    let mine: Vec<Id> = parts
        .iter()
        .filter(|part| part.source_id == c.chat)
        .map(|part| part.part_id)
        .collect();
    assert!(
        mine.windows(2).all(|pair| pair[0] < pair[1]),
        "in the order they were written"
    );
    let (more, spent) = cost(&mut tx, async |conn| {
        rows::parts(conn, c.hub, &heavy, 6, mine.last().copied())
            .await
            .unwrap()
    })
    .await;
    assert!(
        more.iter()
            .filter(|part| part.source_id == c.chat)
            .all(|part| part.part_id > *mine.last().unwrap())
    );
    assert!(spent <= 200, "the next six read {spent}");
    let (counts, spent) = cost(&mut tx, async |conn| {
        rows::mention_counts(conn, c.hub, &heavy, 101)
            .await
            .unwrap()
    })
    .await;
    assert_eq!(
        counts[&c.chat], 101,
        "counted up to the limit and no further"
    );
    assert_eq!(counts[&c.thread], 101);
    assert!(spent <= 1_500, "counting read {spent}");

    // The graph: the neighbours of the hub, however many cite it, then of many things at once, then the references
    // among the things drawn.
    let (near, spent) = cost(&mut tx, async |conn| {
        rows::neighbours(conn, &[("document", c.hub)], &scopes(&s), 100, 300)
            .await
            .unwrap()
    })
    .await;
    assert!(
        near.len() >= 100 && near.len() <= 300,
        "{} neighbours",
        near.len()
    );
    assert!(spent <= 6_000, "the neighbours of the hub read {spent}");
    let crowd: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM documents WHERE title LIKE 'Doc %' ORDER BY id DESC LIMIT 150",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let walked: Vec<rows::Whole<'_>> = crowd.iter().map(|id| ("document", *id)).collect();
    let (around, spent) = cost(&mut tx, async |conn| {
        rows::neighbours(conn, &walked, &scopes(&s), 4, 500)
            .await
            .unwrap()
    })
    .await;
    assert!(!around.is_empty() && around.len() <= 500);
    assert!(spent <= 20_000, "the neighbours of 150 things read {spent}");
    let mut drawn: Vec<rows::Whole<'_>> = walked.clone();
    drawn.push(("document", c.hub));
    let (linked, spent) = cost(&mut tx, async |conn| {
        rows::among(conn, &drawn).await.unwrap()
    })
    .await;
    assert_eq!(linked.len(), 150, "each of the documents cites the hub");
    assert!(
        spent <= 151 * 151 * 4,
        "the references among 151 things read {spent}"
    );
    let sequential: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(pg_stat_get_xact_tuples_returned(c.oid)),0)::bigint FROM pg_class c
          WHERE c.relnamespace='public'::regnamespace AND c.relkind='r' AND c.relname IN
                ('related_references','documents','document_blocks','chat_messages','items')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(
        sequential < 5_000,
        "no table of these was scanned through: {sequential} rows read"
    );
    tx.rollback().await.unwrap();
}

async fn references(
    s: &Scene,
    document: Id,
    query: &DocumentReferencesListQuery,
) -> Page<LinkedThing> {
    s.rig
        .world
        .app
        .call(DocumentReferencesList, &[&document])
        .token(&s.ada.token)
        .query(query)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_answers_to_the_api_at_that_size_are_one_page_whatever_cites_the_document() {
    let s = Scene::new().await;
    let c = corpus(&s, false).await;
    let first = references(
        &s,
        c.hub,
        &DocumentReferencesListQuery {
            direction: LinkDirection::In,
            limit: Some(20),
            cursor: None,
            of: None,
        },
    )
    .await;
    assert_eq!(first.items.len(), 20, "a page, however many cite the hub");
    assert!(first.next_cursor.is_some());
    let heavy: Vec<&LinkedThing> = first
        .items
        .iter()
        .filter(|item| item.id == c.chat || item.id == c.thread)
        .collect();
    assert_eq!(
        heavy.len(),
        2,
        "the chat and the thread, newest identities, are on the first page"
    );
    for item in heavy {
        assert_eq!(item.mentions, 100, "counted up to the setting's default");
        assert!(item.mentions_more, "and says there are more");
        assert_eq!(item.occurrences.len(), 5, "the first mentions only");
        assert!(item.next_mentions.is_some());
        assert!(matches!(item.kind, LinkedKind::Chat | LinkedKind::Thread));
    }
    // The second page goes on below the last source of the first, with a cursor and no repeat.
    let second = references(
        &s,
        c.hub,
        &DocumentReferencesListQuery {
            direction: LinkDirection::In,
            limit: Some(20),
            cursor: first.next_cursor.clone(),
            of: None,
        },
    )
    .await;
    assert_eq!(second.items.len(), 20);
    assert!(
        second
            .items
            .iter()
            .all(|item| first.items.iter().all(|before| before.id != item.id))
    );
    // The graph is cut to its size setting, saying so, and reads the same however many cite the centre.
    let drawn: LinkGraph = s
        .rig
        .world
        .app
        .call(DocumentGraphGet, &[&c.hub])
        .token(&s.ada.token)
        .query(&DocumentGraphGetQuery {
            depth: Some(2),
            kinds: None,
            meaning: None,
            center: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(drawn.nodes.len(), 150, "the default size setting");
    assert!(drawn.truncated);
    assert!(
        drawn
            .nodes
            .iter()
            .any(|node| node.id == c.hub && node.current)
    );
    let _ = c.thread;
}

#[tokio::test]
async fn a_document_that_refers_to_thousands_of_things_lists_them_a_page_at_a_time_each_once() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let index = created(&s, topic, "Index").await;
    let block: Id = sqlx::query_scalar(
        "SELECT id FROM document_blocks WHERE document_id=$1 AND removed_at IS NULL LIMIT 1",
    )
    .bind(index)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO related_references (source_kind,source_id,part_id,target_kind,target_id)
         SELECT 'document',$1,$2,'document',('00000000-0000-7000-8000-'||lpad(to_hex(g+4096),12,'0'))::uuid
           FROM generate_series(1,3000) g",
    )
    .bind(index)
    .bind(block)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let mut seen: Vec<Id> = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = references(
            &s,
            index,
            &DocumentReferencesListQuery {
                direction: LinkDirection::Out,
                limit: Some(200),
                cursor: cursor.take(),
                of: None,
            },
        )
        .await;
        assert!(page.items.len() <= 200, "a page is a page");
        seen.extend(page.items.iter().map(|item| item.id));
        pages += 1;
        cursor = page.next_cursor;
        if cursor.is_none() || pages > 30 {
            break;
        }
    }
    assert_eq!(pages, 15, "3000 targets, 200 to a page");
    let mut once = seen.clone();
    once.sort();
    once.dedup();
    assert_eq!(
        (seen.len(), once.len()),
        (3000, 3000),
        "each target once, none lost at a page's end"
    );
}
