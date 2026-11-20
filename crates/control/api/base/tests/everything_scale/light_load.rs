//! Attribute the installed workload's four expensive query families to actual pages and idle work.
use super::{
    hotspots::{Capture, Plan},
    page_budgets::fixture,
    realtime_support::{Client, Server},
    support::World,
};
use orch_api_contract::{
    access::NodeDefaultsGet, chats::ChatOpeningGet, threads::ThreadOpeningGet,
};
use orch_db::{
    Db,
    queries::{chats, threads},
};
use orch_types::Id;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
struct Work {
    calls: i64,
    ms: f64,
}

impl Work {
    fn average(self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            self.ms / self.calls as f64
        }
    }
}

// Top-level statements get their server duration from auto_explain. Nested holder and
// scope functions get their inclusive time directly from PostgreSQL's function counters.
async fn configure(db: &Db) {
    Capture::configure(db, true).await;
    let mut leases = Vec::new();
    for _ in 0..db.pool().options().get_max_connections() {
        leases.push(db.acquire().await.unwrap());
    }
    for conn in &mut leases {
        sqlx::raw_sql(
            "SET auto_explain.log_nested_statements=off;
            SET auto_explain.log_analyze=off; SET auto_explain.log_min_duration=0;
            SET track_functions='all'",
        )
        .execute(&mut **conn)
        .await
        .unwrap();
    }
}

async fn functions(world: &World) -> [Work; 2] {
    let db = &world.app.app.db;
    let mut leases = Vec::new();
    for _ in 0..db.pool().options().get_max_connections() {
        leases.push(db.acquire().await.unwrap());
    }
    for conn in &mut leases {
        sqlx::query("SELECT pg_stat_force_next_flush()")
            .execute(&mut **conn)
            .await
            .unwrap();
    }
    drop(leases);
    let rows: Vec<(String, i64, f64)> = sqlx::query_as(
        "SELECT funcname::text,calls,total_time FROM pg_stat_user_functions
          WHERE schemaname='public' AND funcname IN ('access_holders','access_view_scope_counts','access_view_scope_bounds')",
    ).fetch_all(&world.db.pool).await.unwrap();
    let mut counters = [Work::default(); 2];
    for (name, calls, ms) in rows {
        let cost = &mut counters[usize::from(name != "access_holders")];
        cost.calls += calls;
        cost.ms += ms;
    }
    counters
}

fn measured(plans: Vec<Plan>, before: [Work; 2], after: [Work; 2]) -> [Work; 4] {
    let mut work = [Work::default(); 4];
    for plan in plans {
        let sql = plan.body["Query Text"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase();
        let entry = if sql.contains("ordinary assignment summaries")
            || sql.contains("from participation_direct_counts")
        {
            &mut work[0]
        } else if sql.contains("observation_revision")
            && sql.contains("held_by_environment_id")
            && (sql.contains("from providers where id")
                || sql.contains("from providers p left join machines"))
        {
            &mut work[2]
        } else {
            continue;
        };
        entry.calls += 1;
        entry.ms += plan.duration_ms;
    }
    for (index, family) in [1, 3].into_iter().enumerate() {
        work[family] = Work {
            calls: after[index].calls - before[index].calls,
            ms: after[index].ms - before[index].ms,
        };
    }
    work
}

fn check(
    label: &str,
    work: [Work; 4],
    budgets: [i64; 4],
    samples: i64,
    failures: &mut Vec<String>,
) {
    for ((family, cost), budget) in ["participation", "holders", "provider", "scope"]
        .into_iter()
        .zip(work)
        .zip(budgets)
    {
        eprintln!(
            "database workload window={label} family={family} samples={samples} calls={} total_ms={:.3} mean_ms={:.3}",
            cost.calls,
            cost.ms,
            cost.average()
        );
        // Five milliseconds is the per-family steady-state database goal. The cold native
        // statement limit, including nested work, is separately exercised by page_budgets.
        if cost.calls > samples * budget || cost.average() > 5.0 {
            failures.push(format!(
                "{label} {family}: {} calls, {:.3} ms mean",
                cost.calls,
                cost.average()
            ));
        }
    }
}

#[derive(Clone, Copy)]
enum Page {
    Thread(Id),
    Chat(Id),
    Defaults(Id),
}

impl Page {
    async fn open(self, world: &World, token: &str) {
        let reply = match self {
            Self::Thread(id) => {
                world
                    .app
                    .call(ThreadOpeningGet, &[&id])
                    .token(token)
                    .raw_query("limit=100")
                    .send_bytes()
                    .await
            }
            Self::Chat(id) => {
                world
                    .app
                    .call(ChatOpeningGet, &[&id])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Self::Defaults(id) => {
                world
                    .app
                    .call(NodeDefaultsGet, &[&id])
                    .token(token)
                    .send_bytes()
                    .await
            }
        };
        assert!(reply.status.is_success(), "page returned {}", reply.status);
    }
}

#[tokio::test]
async fn one_person_opening_two_pages_a_second_keeps_idle_and_per_page_database_work_bounded() {
    let (world, reader, [org, project, topic, thread, _]) = fixture(1, true).await;
    // The installed defaults include real local rows. Exercise both a role-valued source and
    // a local inheritance cut, so the role and page-scope counters have a positive control.
    let mut setup = world.db.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO node_roles(id,node_id,subject_kind,subject_id,subject_role,role,created_by)
        VALUES(gen_random_uuid(),$1,'role',$2,'developer','developer',$3)",
    )
    .bind(topic)
    .bind(project)
    .bind(reader.id())
    .execute(&mut *setup)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO permission_rows(object_kind,object_id,page,profile,permission,inherit,locked)
        VALUES('node',$1,'live','any','topic.archive',false,false)
        ON CONFLICT (object_kind,object_id,page,profile,permission) DO UPDATE SET inherit=false",
    )
    .bind(topic)
    .execute(&mut *setup)
    .await
    .unwrap();
    setup.commit().await.unwrap();
    let chat = chats::chat_of_node(&mut *world.conn().await, topic)
        .await
        .unwrap()
        .unwrap()
        .id;
    let app = &world.app.app;
    let pages = [
        // One page may obtain focus facts or the bound provider in its render and publication snapshots.
        // A stable ordinary holder set needs no recursive rebuild or scope expansion after warm-up.
        ("thread", Page::Thread(thread), [2, 0, 2, 0]),
        ("chat", Page::Chat(chat), [2, 0, 0, 0]),
        ("topic defaults", Page::Defaults(topic), [2, 0, 0, 0]),
        ("org defaults", Page::Defaults(org), [2, 0, 0, 0]),
    ];
    for (_, page, _) in &pages {
        page.open(&world, &reader.token).await;
    }
    configure(&app.db).await;
    let capture = Capture::default();
    let before = functions(&world).await;
    let mut conn = app.db.acquire().await.unwrap();
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM access_holders($1,$2,false)")
        .bind(reader.id())
        .bind(world.now())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let provider: Id = sqlx::query_scalar("SELECT provider_id FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    capture
        .run(orch_db::queries::resources::thread_engine::provider(
            &mut conn, provider,
        ))
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    let control = measured(capture.take(), before, functions(&world).await);
    assert_eq!(
        control[1].calls, 1,
        "the native holder walk must be counted"
    );
    assert_eq!(
        control[2].calls, 1,
        "the actual provider lookup must be counted"
    );
    assert!(
        control[3].calls > 0,
        "the local cut must resolve scope bounds"
    );
    let mut failures = Vec::new();
    for (name, page, budgets) in pages {
        let window = Instant::now();
        let before = functions(&world).await;
        for _ in 0..3 {
            let start = tokio::time::Instant::now();
            capture.run(page.open(&world, &reader.token)).await;
            tokio::time::sleep_until(start + Duration::from_millis(500)).await;
        }
        let after = functions(&world).await;
        let plans = capture.take();
        let active_ms: f64 = plans.iter().map(|plan| plan.duration_ms).sum();
        eprintln!(
            "database workload page={name} seconds={:.3} statement_ms={active_ms:.3}",
            window.elapsed().as_secs_f64()
        );
        if active_ms >= window.elapsed().as_secs_f64() * 500.0 {
            failures.push(format!(
                "{name} used at least half of one backend at two requests a second"
            ));
        }
        check(
            name,
            measured(plans, before, after),
            budgets,
            3,
            &mut failures,
        );
    }

    // The installed hour mixed request traffic and periodic work. Count the latter on its own
    // with the same populated teams, machines and runtimes, after startup queues have drained.
    let _subscriber = tracing::subscriber::set_default(capture.clone());
    app.link.start().await.unwrap();
    app.engine.start();
    app.realtime.start(app.engine.clone());
    tokio::time::timeout(Duration::from_secs(30), async {
        while app.engine.sweeps_run() < 2 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let server = Server::serve(
        app.clone(),
        app.realtime.clone(),
        app.engine.clone(),
        None,
        None,
    )
    .await;
    let cursor = threads::thread(&mut *world.conn().await, thread)
        .await
        .unwrap()
        .unwrap()
        .seq();
    let mut clients = Vec::new();
    for _ in 0..5 {
        let mut client = Client::signed_in(server.addr, &reader.token).await;
        client
            .sub(1, &format!("thread:{thread}"), Some(cursor))
            .await;
        let accepted = client.expect("subscribed").await;
        assert_eq!(accepted["id"], 1);
        assert_eq!(client.expect("presence").await["id"], 1);
        client.sub(2, "user", None).await;
        // The thread remains live while the browser waits for its personal feed.
        tokio::time::timeout(super::realtime_support::PATIENCE, async {
            loop {
                let frame = client.frame().await;
                if frame["op"] == "subscribed" {
                    assert_eq!(frame["id"], 2);
                    break;
                }
                assert_eq!(frame["id"], 1, "{frame}");
                assert_ne!(frame["op"], "error", "{frame}");
                assert_ne!(frame["op"], "closed", "{frame}");
            }
        })
        .await
        .expect("the browser receives its personal subscription acknowledgment");
        clients.push(client);
    }
    let before = functions(&world).await;
    drop(capture.take());
    let start = Instant::now();
    let sweeps = app.engine.sweeps_run();
    for second in 1..=30 {
        if second % 10 == 0 {
            for client in &mut clients {
                client.heartbeat().await;
            }
        }
        world.advance(chrono::Duration::seconds(1));
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let elapsed = start.elapsed();
    let after = functions(&world).await;
    let plans = capture.take();
    let active_ms: f64 = plans.iter().map(|p| p.duration_ms).sum();
    assert!(app.engine.sweeps_run() > sweeps && !plans.is_empty());
    drop(clients);
    server.stop().await;
    check(
        "idle",
        measured(plans, before, after),
        [0; 4],
        1,
        &mut failures,
    );
    eprintln!(
        "database workload idle seconds={:.3} statement_ms={active_ms:.3}",
        elapsed.as_secs_f64()
    );
    assert!(
        active_ms < elapsed.as_secs_f64() * 500.0,
        "idle SQL occupied at least half of one backend"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
