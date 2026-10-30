//! Import page indexes bound examined rows, including cached generic plans and live node-order maintenance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::Conn;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::imports::{self, ReportFilter};
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use serde_json::Value;

const PAGE: i64 = 50;

struct Site {
    user: Id,
    org: Id,
    project: Id,
    topic: Id,
    import: Id,
}

async fn site(conn: &mut Conn) -> Site {
    let (user, org, project, topic, import) = (new_id(), new_id(), new_id(), new_id(), new_id());
    insert_user(conn, user, "storage").await.unwrap();
    insert_node(conn, org, "org", None, "Storage", user)
        .await
        .unwrap();
    insert_node(conn, project, "project", Some(org), "Project", user)
        .await
        .unwrap();
    insert_node(conn, topic, "topic", Some(project), "Topic", user)
        .await
        .unwrap();
    sqlx::query("INSERT INTO imports(id,user_id,node_id,client_id,request,state,files,losses)
        VALUES($1,$2,$3,'storage','{\"keep\":[1,{\"text\":\"original\"}]}'::jsonb,'done',
               ARRAY['original/a.json','原文/b.md'],'[{\"code\":\"original\",\"count\":7}]'::jsonb)")
        .bind(import).bind(user).bind(project).execute(&mut *conn).await.unwrap();
    Site {
        user,
        org,
        project,
        topic,
        import,
    }
}

/// Each scan must examine a page, rather than sorting or rejecting a growing prefix.
fn examined(plan: &Value) -> f64 {
    match plan {
        Value::Array(values) => values.iter().map(examined).sum(),
        Value::Object(map) => {
            let kind = map.get("Node Type").and_then(Value::as_str).unwrap_or("");
            assert!(
                !matches!(kind, "Seq Scan" | "Sort" | "Incremental Sort"),
                "{plan}"
            );
            let rows = if kind.ends_with("Scan") {
                let count = |key| map.get(key).and_then(Value::as_f64).unwrap_or(0.0);
                (count("Actual Rows")
                    + count("Rows Removed by Filter")
                    + count("Rows Removed by Index Recheck"))
                    * count("Actual Loops")
            } else {
                0.0
            };
            rows + map.values().map(examined).sum::<f64>()
        }
        _ => 0.0,
    }
}

async fn plan(conn: &mut Conn, statement: &str, arguments: &str) -> Value {
    // This deliberately exercises the plan used after repeated executions, not
    // only a one-off plan that knows the values of all optional parameters.
    sqlx::query("SET plan_cache_mode=force_generic_plan")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "PREPARE import_storage_page AS {statement}"
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    let result: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN(ANALYZE,BUFFERS,FORMAT JSON) EXECUTE import_storage_page({arguments})"
    )))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        result[0]["Plan"]["Actual Rows"].as_i64(),
        Some(PAGE),
        "the indexed plan returns a complete page: {result}"
    );
    sqlx::query("DEALLOCATE import_storage_page")
        .execute(&mut *conn)
        .await
        .unwrap();
    result
}

async fn corpus(conn: &mut Conn, site: &Site, from: i64, to: i64) {
    // Ordered keys keep bulk fixture generation from thrashing the disk-backed
    // primary indexes. The stored values and query assertions stay identical.
    sqlx::query(
        "INSERT INTO imports(id,user_id,node_id,client_id,request,state,created_at)
      SELECT md5('history-'||g)::uuid,$1,$2,'history-'||g,'{}','done','2026-01-01T00:00:00Z'
        FROM generate_series($3::bigint,$4::bigint) g ORDER BY 1",
    )
    .bind(site.user)
    .bind(site.project)
    .bind(from)
    .bind(to)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("INSERT INTO import_report_rows(import_id,position,source_id,kind,outcome,warned,row)
      SELECT $1,g,md5('report-'||g)::uuid,'document','created',false,jsonb_build_object('position',g)
        FROM generate_series($2::bigint,$3::bigint) g")
        .bind(site.import).bind(from).bind(to).execute(&mut *conn).await.unwrap();
    // Every rare filter has only a fixed tail of matching rows. Interleaving all
    // eight combinations also detects an index that serves only one predicate.
    sqlx::query(
        "UPDATE import_report_rows
        SET kind=CASE WHEN position>$2-800 AND position%8>=4 THEN 'thread' ELSE 'document' END,
            outcome=CASE WHEN position>$2-800 AND position%4>=2 THEN 'failed' ELSE 'created' END,
            warned=position>$2-800 AND position%2=1
        WHERE import_id=$1 AND (position BETWEEN greatest(1,$3-800) AND $3-1 OR position>$2-800)",
    )
    .bind(site.import)
    .bind(to)
    .bind(from)
    .execute(&mut *conn)
    .await
    .unwrap();
    // Generate the read corpus in sets. Trigger maintenance has its own behavior
    // regression, rather than a million individual lookups during fixture setup.
    sqlx::query("ALTER TABLE nodes DISABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by,deleted_at)
      SELECT g.id,CASE WHEN g.n%5=0 THEN 'thread' ELSE 'topic' END,
             CASE WHEN g.n%5=0 THEN $4 ELSE $2 END,$3,
             CASE WHEN g.n%5=0 THEN ARRAY[$3,$2,$4,g.id] ELSE ARRAY[$3,$2,g.id] END,
             'Node '||g.n,$1,$1,CASE WHEN g.n%10=0 THEN now() END
        FROM (SELECT n,md5('node-'||n)::uuid id FROM generate_series($5::bigint,$6::bigint) n) g ORDER BY 1",
    )
    .bind(site.user)
    .bind(site.project)
    .bind(site.org)
    .bind(site.topic)
    .bind(from)
    .bind(to)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE nodes ENABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE import_origins DISABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO import_origins(scope_id,source_id,kind,object_id,created_by_import,last_import,source_name,imported_at,node_depth)
      SELECT $1,n.id,n.kind,n.id,$2,$2,n.name,now(),
             CASE WHEN n.deleted_at IS NULL THEN cardinality(n.path) END
        FROM generate_series($3::bigint,$4::bigint) g JOIN nodes n ON n.id=md5('node-'||g)::uuid ORDER BY n.id")
        .bind(site.project).bind(site.import).bind(from).bind(to).execute(&mut *conn).await.unwrap();
    sqlx::query("ALTER TABLE import_origins ENABLE TRIGGER USER")
        .execute(&mut *conn)
        .await
        .unwrap();
    for table in ["imports", "import_report_rows", "import_origins", "nodes"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("ANALYZE {table}")))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn filtered_report_history_and_undo_pages_examine_a_page_at_both_corpus_sizes() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let site = site(&mut conn).await;
    let mut before = Vec::new();
    let mut from = 1;
    for size in [100_000_i64, 1_000_000] {
        corpus(&mut conn, &site, from, size).await;
        let mut observations = Vec::new();
        for outcome in [None, Some("failed")] {
            for kind in [None, Some("thread")] {
                for warned in [false, true] {
                    let filter = ReportFilter {
                        outcome,
                        kind,
                        warned,
                    };
                    for after in [-1, size - 401] {
                        let rows = imports::report_rows(
                            &mut conn,
                            site.import,
                            &filter,
                            Some(after),
                            PAGE,
                        )
                        .await
                        .unwrap();
                        assert_eq!(rows.len(), usize::try_from(PAGE).unwrap());
                        assert!(
                            rows.iter().all(|(position, body)| *position > after
                                && body["position"] == *position)
                        );
                        let sql_value = |value: Option<&str>| {
                            value.map_or_else(|| "NULL".to_owned(), |value| format!("'{value}'"))
                        };
                        let arguments = format!(
                            "'{}',{after},{},{},{warned},{PAGE}",
                            site.import,
                            sql_value(outcome),
                            sql_value(kind)
                        );
                        let proof = plan(
                            &mut conn,
                            &imports::report_rows_statement(&filter),
                            &arguments,
                        )
                        .await;
                        let count = examined(&proof);
                        assert!(
                            count <= 51.0,
                            "report corpus={size}, filter={filter:?}, examined={count}: {proof}"
                        );
                        observations.push(count);
                    }
                }
            }
        }
        for after in [false, true] {
            let arguments = if after {
                format!(
                    "'{}','2026-01-01T00:00:00Z','80000000-0000-0000-0000-000000000000',{PAGE}",
                    site.user
                )
            } else {
                format!("'{}',NULL,NULL,{PAGE}", site.user)
            };
            let proof = plan(
                &mut conn,
                &imports::history_page_statement(after),
                &arguments,
            )
            .await;
            let count = examined(&proof);
            assert!(
                count <= 51.0,
                "history corpus={size}, examined={count}: {proof}"
            );
            observations.push(count);
        }
        for (statement, arguments) in [
            (
                imports::NODES_PAGE_SQL,
                format!("'{}',NULL,NULL,{PAGE}", site.import),
            ),
            (
                imports::NODES_PAGE_AFTER_SQL,
                format!(
                    "'{}',3,'80000000-0000-0000-0000-000000000000',{PAGE}",
                    site.import
                ),
            ),
        ] {
            let proof = plan(&mut conn, statement, &arguments).await;
            let count = examined(&proof);
            assert!(
                count <= 51.0,
                "undo corpus={size}, examined={count}: {proof}"
            );
            observations.push(count);
        }
        if !before.is_empty() {
            assert_eq!(
                observations, before,
                "ten times more data examines the same page"
            );
        }
        before = observations;
        from = size + 1;
    }
    drop(conn);
    db.close().await.unwrap();
}

#[tokio::test]
async fn imported_origins_track_current_live_node_depth() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let site = site(&mut conn).await;
    corpus(&mut conn, &site, 1, 10).await;
    sqlx::query("INSERT INTO import_origins(scope_id,source_id,kind,object_id,created_by_import,last_import,source_name,imported_at)
      VALUES($1,md5('non-node')::uuid,'document',md5('node-1')::uuid,$2,$2,'keep document',now()),
            ($1,md5('missing-node')::uuid,'topic',md5('missing-object')::uuid,$2,$2,'keep missing',now()),
            ($1,md5('reused-node')::uuid,'topic',md5('node-2')::uuid,NULL,$2,'keep reused',now())")
        .bind(site.project).bind(site.import).execute(&mut *conn).await.unwrap();
    let wrong:i64=sqlx::query_scalar("SELECT count(*) FROM import_origins o LEFT JOIN nodes n ON n.id=o.object_id
        WHERE o.node_depth IS DISTINCT FROM CASE WHEN o.created_by_import IS NOT NULL
          AND o.kind IN ('org','project','topic','thread') AND n.deleted_at IS NULL THEN cardinality(n.path) END")
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        wrong, 0,
        "origins are indexed only for current live nodes created by the import"
    );
    sqlx::query("INSERT INTO import_origins(scope_id,source_id,kind,object_id,created_by_import,last_import,source_name,imported_at)
      VALUES($1,md5('added-live')::uuid,'topic',md5('node-3')::uuid,$2,$2,'added1',now()),
            ($1,md5('added-deleted')::uuid,'thread',md5('node-10')::uuid,$2,$2,'added2',now()),
            ($1,md5('added-document')::uuid,'document',md5('node-3')::uuid,$2,$2,'added3',now())")
        .bind(site.project).bind(site.import).execute(&mut *conn).await.unwrap();
    let depths: Vec<Option<i32>> = sqlx::query_scalar(
        "SELECT node_depth FROM import_origins WHERE source_name IN ('added1','added2','added3') ORDER BY source_name",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        depths,
        vec![Some(3), None, None],
        "new origins derive their depth only for live nodes"
    );
    let object: Id = sqlx::query_scalar("SELECT md5('node-1')::uuid")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE nodes SET parent_id=$2,path=ARRAY[$3,$4,$2,id] WHERE id=$1")
        .bind(object)
        .bind(site.topic)
        .bind(site.org)
        .bind(site.project)
        .execute(&mut *conn)
        .await
        .unwrap();
    let depth: Option<i32> =
        sqlx::query_scalar("SELECT node_depth FROM import_origins WHERE source_id=$1")
            .bind(object)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        depth,
        Some(4),
        "a move changes the indexed order in the same transaction"
    );
    sqlx::query("UPDATE nodes SET deleted_at=now() WHERE id=$1")
        .bind(object)
        .execute(&mut *conn)
        .await
        .unwrap();
    let depth: Option<i32> =
        sqlx::query_scalar("SELECT node_depth FROM import_origins WHERE source_id=$1")
            .bind(object)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        depth, None,
        "deleted nodes leave no holes in the live-order index"
    );
    sqlx::query("UPDATE nodes SET deleted_at=NULL WHERE id=$1")
        .bind(object)
        .execute(&mut *conn)
        .await
        .unwrap();
    let depth: Option<i32> =
        sqlx::query_scalar("SELECT node_depth FROM import_origins WHERE source_id=$1")
            .bind(object)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        depth,
        Some(4),
        "restoration uses the current path, not import-time depth"
    );
    sqlx::query("DELETE FROM nodes WHERE id=$1")
        .bind(object)
        .execute(&mut *conn)
        .await
        .unwrap();
    let depth: Option<i32> =
        sqlx::query_scalar("SELECT node_depth FROM import_origins WHERE source_id=$1")
            .bind(object)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(depth, None, "purged objects cannot appear in Undo pages");
    sqlx::query(
        "UPDATE import_origins SET created_by_import=NULL WHERE source_id=md5('node-2')::uuid",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    let depth: Option<i32> = sqlx::query_scalar(
        "SELECT node_depth FROM import_origins WHERE source_id=md5('node-2')::uuid",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        depth, None,
        "taking over an existing object does not make it undoable"
    );
    sqlx::query(
        "UPDATE import_origins SET object_id=md5('node-5')::uuid,kind='thread',created_by_import=$1
        WHERE source_id=md5('node-2')::uuid",
    )
    .bind(site.import)
    .execute(&mut *conn)
    .await
    .unwrap();
    let depth: Option<i32> = sqlx::query_scalar(
        "SELECT node_depth FROM import_origins WHERE source_id=md5('node-2')::uuid",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        depth,
        Some(4),
        "remapped origins use the new object's current depth"
    );
    sqlx::query(
        "INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
        VALUES(md5('missing-object')::uuid,'topic',$1,$2,ARRAY[$2,$1,md5('missing-object')::uuid],
        'Found',$3,$3)",
    )
    .bind(site.project)
    .bind(site.org)
    .bind(site.user)
    .execute(&mut *conn)
    .await
    .unwrap();
    let depth: Option<i32> = sqlx::query_scalar(
        "SELECT node_depth FROM import_origins WHERE source_id=md5('missing-node')::uuid",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        depth,
        Some(3),
        "a previously missing node is indexed when it appears"
    );
    drop(conn);
    db.close().await.unwrap();
}
