use super::*;

const DOCUMENT_SQL: &str = include_str!("../../src/queries/summaries/document_text.sql");
const TOPIC_SQL: &str = include_str!("../../src/queries/summaries/topic_sources.sql");

async fn block(
    conn: &mut orch_db::Conn,
    document: Id,
    id: u128,
    parent: Option<u128>,
    text: &str,
    sequence: i64,
) -> TestResult {
    sqlx::query("INSERT INTO document_blocks(id,document_id,parent_id,\"position\",type,text,seq_created,seq_updated) VALUES($1,$2,$3,0,'paragraph',$4,1,$5)")
        .bind(Id::from_u128(id)).bind(document).bind(parent.map(Id::from_u128))
        .bind(text).bind(sequence).execute(conn).await?;
    Ok(())
}

#[tokio::test]
async fn document_pages_keep_the_native_revision_cursor_and_live_ancestry() -> TestResult {
    let world = World::new().await?;
    let mut conn = world.db.acquire().await?;
    for (id, parent, text, sequence) in [
        (1, None, "First", 1),
        (2, None, "Parent", 5),
        (3, None, "Sibling at the same revision", 5),
        (4, Some(2), "Newer child", 9),
        (5, Some(2), "  ", 20),
        (6, Some(5), "Text below an empty live parent", 7),
        (7, Some(2), "Removed parent", 19),
        (8, Some(7), "Do not sample below removal", 30),
        (10, None, "Rootless cycle", 40),
        (11, Some(10), "Other member of the cycle", 40),
        (
            u128::MAX,
            None,
            "The natural upper bound is included",
            i64::MAX,
        ),
    ] {
        block(&mut conn, world.document, id, parent, text, sequence).await?;
    }
    sqlx::query("UPDATE document_blocks SET removed_at=$1 WHERE id=$2")
        .bind(clock::start())
        .bind(Id::from_u128(7))
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE document_blocks SET parent_id=$1 WHERE id=$2")
        .bind(Id::from_u128(11))
        .bind(Id::from_u128(10))
        .execute(&mut *conn)
        .await?;
    let mut got = Vec::new();
    let mut before = None;
    loop {
        let page = summaries::document_text(&mut conn, world.document, before, 2).await?;
        let Some(last) = page.last() else { break };
        before = Some((last.ordinal, last.id));
        assert!(page.iter().all(|row| row.ordinal == row.seq_updated));
        got.extend(page.into_iter().map(|row| row.id));
    }
    assert_eq!(got, [u128::MAX, 4, 6, 3, 2, 1].map(Id::from_u128));
    assert_eq!(
        summaries::document_text_total(&mut conn, world.document).await?,
        6
    );
    assert!(
        summaries::document_text(&mut conn, world.document, None, 0)
            .await?
            .is_empty()
    );
    drop(conn);
    world.close().await
}

fn visit_plan(plan: &Json, check: &mut impl FnMut(&Json)) {
    check(plan);
    if let Some(children) = plan.get("Plans").and_then(Json::as_array) {
        for child in children {
            visit_plan(child, check);
        }
    }
}

#[tokio::test]
async fn a_large_document_page_seeks_before_ancestry_and_never_sorts_its_whole_tree() -> TestResult
{
    let world = World::new().await?;
    let mut conn = world.db.acquire().await?;
    sqlx::query("INSERT INTO document_blocks(id,document_id,\"position\",type,text,seq_created,seq_updated) SELECT lpad(to_hex(g.n),32,'0')::uuid,$1,g.n,'paragraph','Source text',1,1 FROM generate_series(1,20000) g(n)")
        .bind(world.document).execute(&mut *conn).await?;
    sqlx::query("ANALYZE document_blocks")
        .execute(&mut *conn)
        .await?;
    assert_eq!(
        summaries::document_text_total(&mut conn, world.document).await?,
        20000
    );
    for before in [None, Some((1_i64, Id::from_u128(3)))] {
        let bound = before.unwrap_or((i64::MAX, Id::from_u128(u128::MAX)));
        let sql = format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)\n{DOCUMENT_SQL}");
        let plan: Json = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(world.document)
            .bind(bound.0)
            .bind(bound.1)
            .bind(2_i64)
            .bind(before.is_none())
            .fetch_one(&mut *conn)
            .await?;
        let mut seek = false;
        visit_plan(&plan[0]["Plan"], &mut |node| {
            if node["Relation Name"] == "document_blocks" {
                let rows = node["Actual Rows"].as_u64().unwrap_or(0);
                assert!(rows <= 3, "{plan}");
                if node["Index Name"] == "document_blocks_summary_recent_idx" {
                    seek = node["Index Cond"]
                        .as_str()
                        .is_some_and(|condition| condition.contains("<="));
                }
            }
            if node["Node Type"] == "Sort" {
                for child in node["Plans"].as_array().into_iter().flatten() {
                    assert!(child["Actual Rows"].as_u64().unwrap_or(0) <= 3, "{plan}");
                }
            }
        });
        assert!(
            seek,
            "the actual production query must enter at its content cursor: {plan}"
        );
        eprintln!("document cursor buffers: {}", plan[0]["Plan"]);
        let page = summaries::document_text(&mut conn, world.document, before, 2).await?;
        let expected = if before.is_none() {
            [20000, 19999]
        } else {
            [2, 1]
        };
        assert_eq!(
            page.iter().map(|row| row.id).collect::<Vec<_>>(),
            expected.map(Id::from_u128)
        );
    }
    drop(conn);
    world.close().await
}

async fn threads(conn: &mut orch_db::Conn, world: &World, through: i32) -> TestResult {
    sqlx::query("INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by,created_at,updated_at) SELECT lpad(to_hex(g.n),32,'0')::uuid,'thread',$1,$2,n.path||lpad(to_hex(g.n),32,'0')::uuid,'Source '||g.n::text,$3,$3,$5,$5 FROM generate_series(1,$4) g(n) CROSS JOIN nodes n WHERE n.id=$1")
        .bind(world.topic).bind(world.root).bind(world.ada).bind(through).bind(clock::start())
        .execute(&mut *conn).await?;
    sqlx::query("INSERT INTO threads(node_id,topic_id,last_activity_at) SELECT id,$1,CASE WHEN (('x'||right(id::text,4))::bit(16)::integer % 2)=0 THEN $2::timestamptz ELSE NULL END FROM nodes WHERE parent_id=$1 AND kind='thread'")
        .bind(world.topic).bind(clock::start()).execute(conn).await?;
    Ok(())
}

#[tokio::test]
async fn topic_activity_pages_keep_equal_id_family_ties_and_native_exclusions() -> TestResult {
    let world = World::new().await?;
    let mut conn = world.db.acquire().await?;
    threads(&mut conn, &world, 4).await?;
    sqlx::query("UPDATE nodes SET archived_at=$1 WHERE id=$2")
        .bind(clock::start())
        .bind(Id::from_u128(3))
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE threads SET side_of=$1 WHERE node_id=$2")
        .bind(Id::from_u128(1))
        .bind(Id::from_u128(4))
        .execute(&mut *conn)
        .await?;
    for (id, visibility, seq) in [
        (0, "public", 1_i64),
        (2, "public", 1),
        (5, "private", 1),
        (6, "public", 0),
    ] {
        sqlx::query("INSERT INTO chats(id,kind,visibility,home_id,org_id,name,created_by,last_message_seq,created_at) VALUES($1,'group',$2,$3,$4,$5,$6,$7,$8)")
            .bind(Id::from_u128(id)).bind(visibility).bind(world.topic).bind(world.root)
            .bind(format!("Group {id}")).bind(world.ada).bind(seq).bind(clock::start()).execute(&mut *conn).await?;
    }
    let mut got = Vec::new();
    let mut after = None;
    loop {
        let page = summaries::topic_sources(
            &mut conn,
            world.topic,
            after.as_ref().map(
                |(at, id, kind): &(chrono::DateTime<chrono::Utc>, Id, String)| {
                    (*at, *id, kind.as_str())
                },
            ),
            1,
        )
        .await?;
        let Some(row) = page.into_iter().next() else {
            break;
        };
        assert_eq!(
            row.place,
            if row.kind == "thread" {
                row.id
            } else {
                world.topic
            }
        );
        after = Some((row.at, row.id, row.kind.clone()));
        got.push((row.id, row.kind));
    }
    assert_eq!(
        got,
        [(2, "thread"), (2, "chat"), (1, "thread"), (0, "chat")]
            .map(|(id, kind)| (Id::from_u128(id), kind.to_owned()))
    );
    drop(conn);
    world.close().await
}

#[tokio::test]
async fn a_late_activity_cursor_seeks_all_three_families_before_a_small_merge() -> TestResult {
    let world = World::new().await?;
    let mut conn = world.db.acquire().await?;
    threads(&mut conn, &world, 20000).await?;
    sqlx::query("INSERT INTO chats(id,kind,visibility,home_id,org_id,name,created_by,last_message_seq,created_at,last_message_at) SELECT lpad(to_hex(g.n),32,'0')::uuid,'group','public',$1,$2,'Group '||g.n::text,$3,1,$4,CASE WHEN g.n%2=0 THEN $4::timestamptz ELSE NULL END FROM generate_series(1,20000) g(n)")
        .bind(world.topic).bind(world.root).bind(world.ada).bind(clock::start()).execute(&mut *conn).await?;
    for table in ["nodes", "threads", "chats"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("ANALYZE {table}")))
            .execute(&mut *conn)
            .await?;
    }
    let sql = format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)\n{TOPIC_SQL}");
    let bound = Id::from_u128(6);
    let plan: Json = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
        .bind(world.topic)
        .bind(clock::start())
        .bind(bound)
        .bind("thread")
        .bind(2_i64)
        .fetch_one(&mut *conn)
        .await?;
    let mut seeks = std::collections::BTreeSet::new();
    visit_plan(&plan[0]["Plan"], &mut |node| {
        if let Some(name) = node["Index Name"].as_str().filter(|name| {
            matches!(
                *name,
                "threads_summary_activity_idx"
                    | "nodes_summary_thread_activity_idx"
                    | "chats_summary_activity_idx"
            )
        }) {
            assert!(
                node["Index Cond"]
                    .as_str()
                    .is_some_and(|condition| condition.contains("<=")),
                "{plan}"
            );
            seeks.insert(name.to_owned());
        }
        if node["Node Type"] == "Sort" {
            for child in node["Plans"].as_array().into_iter().flatten() {
                assert!(child["Actual Rows"].as_u64().unwrap_or(0) <= 6, "{plan}");
            }
        }
    });
    assert_eq!(
        seeks.len(),
        3,
        "all source entrances must seek the activity cursor: {plan}"
    );
    eprintln!("topic activity cursor buffers: {}", plan[0]["Plan"]);
    let page = summaries::topic_sources(
        &mut conn,
        world.topic,
        Some((clock::start(), bound, "thread")),
        2,
    )
    .await?;
    assert_eq!(
        page.iter()
            .map(|row| (row.id, row.kind.as_str()))
            .collect::<Vec<_>>(),
        [(Id::from_u128(6), "chat"), (Id::from_u128(5), "thread")]
    );
    drop(conn);
    world.close().await
}
