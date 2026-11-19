//! A large hierarchy stays a paged indexed read rather than loading every thread to decide visibility.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::everything::EVERYTHING_LIMIT_DEFAULT;
use orch_authz::{Caller, Principal};
use orch_db::queries::everything::{self, Query};
use orch_types::{Id, Time};
use places::{join, make_org, make_project, person};
use support::World;

#[tokio::test]
async fn the_admin_marker_filter_is_applied_before_the_first_row_limit() {
    let world = World::new().await;
    let owner = person(&world, "inventory-owner").await;
    let reader = person(&world, "inventory-reader").await;
    let root = make_org(&world, &owner, "Inventory", "inventory")
        .await
        .base
        .id;
    let ordinary = make_project(&world, &owner, root, "A ordinary")
        .await
        .base
        .id;
    let elevated = make_project(&world, &owner, root, "Z native").await.base.id;
    // This statement consumes a frame's decisions. Native authorization itself has independent
    // route tests; here the deliberately later marked row must survive a one-row page boundary.
    let ordinary_grant = everything::Grant {
        view: true,
        listed: true,
        named: true,
        participating: true,
        access: "view".to_owned(),
        ..Default::default()
    };
    let native_grant = everything::Grant {
        admin_access: Some(orch_access_contract::roles::AdminReach {
            place: orch_access_contract::roles::PlaceRef::Node(root),
            private: false,
        }),
        participating: false,
        ..ordinary_grant.clone()
    };
    let objects = serde_json::Map::from_iter([
        (
            ordinary.to_string(),
            serde_json::to_value(ordinary_grant).unwrap(),
        ),
        (
            elevated.to_string(),
            serde_json::to_value(native_grant).unwrap(),
        ),
    ]);
    let grants = serde_json::json!({ "objects": objects });
    let parent = root.to_string();
    let mut query = Query {
        navigation: Default::default(),
        resource: Default::default(),
        admin_access: None,
        participating: false,
        user: reader.id(),
        groups: &[],
        member_roots: &[],
        grants: &grants,
        parent: Some(&parent),
        system: false,
        text: "",
        kinds: &[],
        owner: None,
        created_from: None,
        created_to: None,
        active_from: None,
        sort: "name",
        descending: false,
        after: None,
        limit: 1,
        at: world.now(),
    };
    let mut conn = world.conn().await;
    assert_eq!(
        everything::level(&mut conn, &query).await.unwrap()[0].id,
        ordinary.to_string()
    );
    query.admin_access = Some(true);
    let marked = everything::level(&mut conn, &query).await.unwrap();
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].id, elevated.to_string());
    query.admin_access = Some(false);
    let own = everything::level(&mut conn, &query).await.unwrap();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].id, ordinary.to_string());
}

/// The release-sized regression is 100,000 threads spread over 2,000 topics, not one giant branch.
const TOPICS: i64 = 2_000;
/// Fifty threads per topic gives exactly 100,000 while leaving ordinary branch pages realistic.
const THREADS_PER_TOPIC: i64 = 50;
/// Every production statement over one second must be investigated and optimized.
const MAX_STATEMENT_MS: f64 = 1_000.0;

fn index_names(plan: &serde_json::Value, found: &mut Vec<String>) {
    match plan {
        serde_json::Value::Object(object) => {
            if let Some(name) = object.get("Index Name").and_then(serde_json::Value::as_str) {
                found.push(name.to_owned());
            }
            for value in object.values() {
                index_names(value, found);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                index_names(item, found);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn a_hundred_thousand_threads_use_indexes_and_each_level_finishes_within_one_second() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &owner, "Large team", "large-team")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "developer", &owner).await;
    let project = make_project(&world, &owner, root, "Work").await.base.id;
    let mut conn = world.conn().await;
    // Set-based inserts make the fixture cost proportional to the actual tree, with no HTTP loop.
    // Production pools disable JIT for indexed reads; this fixture supplies its own raw pool.
    sqlx::query("SET jit=off")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by,created_at,updated_at)
      SELECT id,'topic',$1,$2,ARRAY[$2,$1,id]::uuid[],'Topic ' || lpad(number::text,4,'0'),$3,$3,$5,$5
      FROM (SELECT gen_random_uuid() id,number FROM generate_series(1,$4::bigint) number) made")
        .bind(project).bind(root).bind(owner.id()).bind(TOPICS).bind(world.now()).execute(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO topics (node_id) SELECT id FROM nodes WHERE parent_id = $1 AND kind = 'topic'",
    )
    .bind(project)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by,created_at,updated_at)
      SELECT id,'thread',parent_id,$1,path || id,'Thread ' || lpad(number::text,4,'0'),$2,$2,$5,$5
      FROM (SELECT gen_random_uuid() id,n.id parent_id,n.path,number FROM nodes n
        CROSS JOIN generate_series(1,$3::bigint) number WHERE n.parent_id = $4 AND n.kind = 'topic') made")
        .bind(root).bind(owner.id()).bind(THREADS_PER_TOPIC).bind(project).bind(world.now()).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO threads (node_id,topic_id) SELECT id,parent_id FROM nodes WHERE org_id = $1 AND kind = 'thread'")
        .bind(root).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO permission_entries (id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by,created_at)
      SELECT gen_random_uuid(),'node',n.id,'live','any','access.own','user',$2,$2,$3
        FROM nodes n WHERE n.org_id = $1 AND n.kind = 'thread'")
        .bind(root).bind(owner.id()).bind(world.now()).execute(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO chats (id,kind,node_id,org_id,created_by)
      SELECT gen_random_uuid(),'node',n.id,n.org_id,$2 FROM nodes n WHERE n.org_id = $1
        AND NOT EXISTS (SELECT 1 FROM chats c WHERE c.node_id = n.id)",
    )
    .bind(root)
    .bind(owner.id())
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ANALYZE nodes, topics, threads, chats, permission_entries")
        .execute(&mut *conn)
        .await
        .unwrap();
    let topic: Id = sqlx::query_scalar(
        "SELECT id FROM nodes WHERE parent_id = $1 AND kind = 'topic' ORDER BY id LIMIT 1",
    )
    .bind(project)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id WHERE n.org_id = $1",
    )
    .bind(root)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(count, TOPICS * THREADS_PER_TOPIC);
    // The setup may take longer; every reader statement, including shared access preparation, may not.
    sqlx::query("SELECT set_config('statement_timeout',$1,false)")
        .bind(format!("{MAX_STATEMENT_MS}ms"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(reader.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let projection = orch_authz::everything::project(&mut conn, &principal, None)
        .await
        .unwrap();
    // A slow listing must still return its plan so the failure names the actual work to optimize.
    sqlx::query("SELECT set_config('statement_timeout','0',false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    // Ordinary threads share their parent's decision: their identifiers are not an in-memory allow list.
    assert!(projection.grants["objects"].as_object().unwrap().len() < count as usize);
    let mut plans = Vec::new();
    let page_limit = i64::from(EVERYTHING_LIMIT_DEFAULT) + 1;
    for (number, (parent, text, owner_filter, sort, descending, created_from)) in [
        (project, "", None, "name", false, None),
        (topic, "", None, "kind", false, None),
        (topic, "", None, "created", true, None),
        (topic, "", None, "last_activity", true, None),
        (project, "", None, "size", true, None),
        (project, "", Some(owner.id()), "name", false, None),
        (project, "", None, "name", false, Some(world.now())),
        (topic, "Thread 0001", None, "name", false, None),
    ]
    .into_iter()
    .enumerate()
    {
        let parent = parent.to_string();
        let query = Query {
            navigation: Default::default(),
            resource: Default::default(),
            participating: false,
            admin_access: None,
            user: reader.id(),
            groups: principal.groups(),
            member_roots: &projection.member_roots,
            grants: &projection.grants,
            parent: Some(&parent),
            system: false,
            text,
            kinds: &[],
            owner: owner_filter,
            created_from,
            created_to: None,
            active_from: None,
            sort,
            descending,
            after: None,
            limit: page_limit,
            at: world.now(),
        };
        let plan = everything::explain(&mut conn, &query).await.unwrap();
        let ms = plan[0]["Execution Time"].as_f64().unwrap();
        let diagnostic = if let Some(directory) = std::env::var_os("ORCH_TEST_SCALE_PLANS") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join(format!("{number}-{sort}.json"));
            std::fs::write(&path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
            eprintln!(
                "hierarchy parent={parent} sort={sort} execution_ms={ms} plan={}",
                path.display()
            );
            format!("plan saved to {}", path.display())
        } else {
            plan.to_string()
        };
        assert!(
            ms < MAX_STATEMENT_MS,
            "hierarchy statement took {ms} ms: {diagnostic}"
        );
        index_names(&plan, &mut plans);
        let page = everything::level(&mut conn, &query).await.unwrap();
        assert!(!page.is_empty());
        assert!(page.len() <= page_limit as usize);
    }
    assert!(
        plans
            .iter()
            .any(|name| name == "nodes_everything_parent_idx"),
        "the hierarchy's parent index must serve the real statement: {plans:?}"
    );
}
