//! Team-sized live reads use their admitted subscriptions while assignment queries are busy.

use super::*;
use orch_types::new_id;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct SqlCounter(Arc<AtomicUsize>);

impl tracing::Subscriber for SqlCounter {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.target() == "sqlx::query"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_readers_get_a_hundred_live_threads_without_waiting_for_assignment_reads() {
    let rig = Rig::with_unstarted_engine().await;
    let mut conn = rig.pool.acquire().await.unwrap();
    for (key, value) in [
        ("orch.fixture_scale", "1".to_owned()),
        ("orch.fixture_reader", rig.ada.to_string()),
        ("orch.fixture_personal", rig.root.to_string()),
    ] {
        sqlx::query("SELECT set_config($1,$2,false)")
            .bind(key)
            .bind(value)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    sqlx::raw_sql(include_str!("../everything_scale/page_budgets.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let people: Vec<Id> =
        sqlx::query_scalar("SELECT pg_temp.page_person(n) FROM generate_series(0,19) n ORDER BY n")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    let threads: Vec<Id> = sqlx::query_scalar(
        "SELECT pg_temp.page_id('thread',n) FROM generate_series(1,100) n ORDER BY n",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    let mut clients = Vec::new();
    for (person_index, person) in people.into_iter().enumerate() {
        let token = rig.token_for(person, chrono::Duration::days(1)).await;
        let mut client = Client::signed_in(rig.server.addr, &token).await;
        // Each member's first org has ten threads. The two members using that org
        // follow its two halves, so the twenty sockets cover all hundred sources.
        let first = (person_index % 10) * 10 + (person_index / 10) * 5;
        let followed = threads[first..first + 5].to_vec();
        for (number, thread) in followed.iter().enumerate() {
            let id = i64::try_from(number + 1).unwrap();
            client.sub(id, &format!("thread:{thread}"), Some(200)).await;
            loop {
                let frame = client.frame().await;
                assert_ne!(frame["op"], "error", "{frame}");
                if frame["op"] == "subscribed" && frame["id"] == id {
                    break;
                }
            }
        }
        clients.push((client, followed));
    }
    let statement_counts = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&statement_counts);
    rig.server.realtime.trace_frame_sql(
        move |feed, _, phase| -> (tracing::Dispatch, Box<dyn FnOnce() + Send>) {
            let count = Arc::new(AtomicUsize::new(0));
            let recorded = Arc::clone(&recorded);
            let source = feed.to_string();
            (
                tracing::Dispatch::new(SqlCounter(Arc::clone(&count))),
                Box::new(move || {
                    recorded
                        .lock()
                        .unwrap()
                        .push((source, phase, count.load(Ordering::Relaxed)));
                }),
            )
        },
    );
    // A retained lock is a deterministic stand-in for the costly ACL/assignment reads
    // seen under load. No permission changes: each admitted subscription remains valid.
    let mut assignments = rig.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE permission_entries IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *assignments)
        .await
        .unwrap();
    let sent = Instant::now();
    let mut readers = tokio::task::JoinSet::new();
    for (mut client, followed) in clients {
        readers.spawn(async move {
            let ping_started = Instant::now();
            tokio::time::timeout(Duration::from_secs(1), client.heartbeat())
                .await
                .expect("a live socket answers its ping within one second");
            let ping = ping_started.elapsed().as_millis();
            assert!(ping < 1_000);
            let mut pending: std::collections::BTreeSet<_> = followed
                .into_iter()
                .map(|thread| format!("thread:{thread}"))
                .collect();
            while !pending.is_empty() {
                let frame = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(sent + Duration::from_secs(2)),
                    client.frame(),
                )
                .await
                .expect("every frame reaches its reader within two seconds of the writes");
                assert_ne!(frame["op"], "error", "{frame}");
                assert_ne!(
                    frame["op"], "resync",
                    "ordinary live items need no replacement page"
                );
                if frame["op"] == "sync" && frame["seq"].as_i64().is_some_and(|seq| seq >= 201) {
                    let feed = frame["feed"].as_str().unwrap();
                    assert!(
                        frame["data"]["items"].as_array().is_some_and(|items| items
                            .iter()
                            .any(|item| item["payload"]["text"] == "A live team message")),
                        "{frame}"
                    );
                    pending.remove(feed);
                }
            }
            eprintln!(
                "live team frames elapsed_ms={} ping_ms={ping}",
                sent.elapsed().as_millis()
            );
            client
        });
    }
    let mut writers = tokio::task::JoinSet::new();
    for thread in threads {
        let database = rig.pool.clone();
        writers.spawn(async move {
            let mut tx=database.begin().await.unwrap();
            let seq=orch_db::sequences::next(&mut tx,thread).await.unwrap().unwrap();
            assert_eq!(seq,201);
            sqlx::query("INSERT INTO items(id,thread_id,segment_id,kind,status,payload,seq_created,seq_updated)
                SELECT $1,$2,id,'agent_message','completed',jsonb_build_object('text','A live team message','text_len',19),$3,$3
                FROM segments WHERE thread_id=$2 ORDER BY ordinal LIMIT 1")
                .bind(new_id()).bind(thread).bind(seq).execute(&mut *tx).await.unwrap();
            orch_db::sequences::notify(&mut tx,thread,seq).await.unwrap();
            tx.commit().await.unwrap();
        });
    }
    while let Some(done) = writers.join_next().await {
        done.unwrap();
    }
    let mut sockets = Vec::new();
    while let Some(done) = readers.join_next().await {
        sockets.push(done.unwrap());
    }
    assignments.rollback().await.unwrap();
    drop(sockets);
    rig.shutdown().await;
    let counts = statement_counts.lock().unwrap();
    let mut frames = BTreeMap::<_, BTreeMap<_, _>>::new();
    for (source, phase, count) in counts.iter() {
        assert!(
            frames
                .entry(source)
                .or_default()
                .insert(*phase, *count)
                .is_none(),
            "each frame has one measurement of {phase}"
        );
    }
    assert_eq!(
        frames.len(),
        100,
        "each produced thread has one measured frame"
    );
    for phases in frames.values() {
        assert_eq!(
            phases.keys().copied().collect::<Vec<_>>(),
            vec![
                "publication.access",
                "publication.epoch",
                "publication.identity",
                "read"
            ],
            "measure the physical publication boundary as well as the prepared rows"
        );
    }
    let totals: Vec<usize> = frames
        .values()
        .map(|phases| phases.values().sum())
        .collect();
    assert!(
        totals.iter().all(|count| *count > 0),
        "the actual SQL events are measured"
    );
    eprintln!(
        "live frame SQL min={} max={} shared_epoch_statements=1",
        totals.iter().min().unwrap(),
        totals.iter().max().unwrap()
    );
    for phase in [
        "read",
        "publication.identity",
        "publication.epoch",
        "publication.access",
    ] {
        let values: Vec<_> = frames.values().map(|phases| phases[phase]).collect();
        eprintln!(
            "live frame SQL phase={phase} min={} max={}",
            values.iter().min().unwrap(),
            values.iter().max().unwrap()
        );
    }
}
