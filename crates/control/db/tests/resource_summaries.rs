//! Native storage/replay/fencing cases. Opaque origin JSON here tests persistence only;
//! actual captured-authority/source/model admission belongs to the engine/API cases.
use chrono::Duration;
use orch_api_contract::summaries::SummaryTarget;
use orch_db::{
    Db,
    queries::{
        chats, documents, hierarchy, resources,
        summaries::{self, Enqueued, Origin, Outcome, Paragraph},
    },
};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Clock, Id, Json, new_id};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[path = "resource_summaries/revival.rs"]
mod revival;

#[path = "resource_summaries/target_index.rs"]
mod target_index;

#[path = "resource_summaries/source_sampling.rs"]
mod source_sampling;

struct World {
    test: TestDb,
    db: Db,
    ada: Id,
    bob: Id,
    root: Id,
    topic: Id,
    document: Id,
    chat: Id,
}

impl World {
    async fn new() -> TestResult<Self> {
        let test = TestDb::fresh().await;
        let db = Db::connect(&test.url, 4).await?;
        let mut tx = db.begin().await?;
        let ada = resources::testing::insert_user(&mut tx, "ada").await?;
        let bob = resources::testing::insert_user(&mut tx, "bob").await?;
        let (root, project, topic, document, chat) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        hierarchy::testing::insert_node(&mut tx, root, "org", None, "Office", ada).await?;
        hierarchy::testing::insert_node(&mut tx, project, "project", Some(root), "Work", ada)
            .await?;
        hierarchy::testing::insert_node(&mut tx, topic, "topic", Some(project), "Plan", ada)
            .await?;
        documents::insert(
            &mut tx,
            &documents::NewDocument {
                id: document,
                node_id: topic,
                org_id: root,
                owner_id: ada,
                created_by_thread: None,
                created_by_agent: None,
                title: "Notes",
                now: clock::start(),
            },
        )
        .await?;
        chats::insert_chat(
            &mut tx,
            &chats::NewChat {
                id: chat,
                kind: chats::KIND_GROUP,
                org_id: root,
                direct_key: None,
                name: Some("Plan"),
                created_by: ada,
                at: clock::start(),
            },
        )
        .await?
        .ok_or("chat missing")?;
        tx.commit().await?;
        Ok(Self {
            test,
            db,
            ada,
            bob,
            root,
            topic,
            document,
            chat,
        })
    }

    async fn cache(
        &self,
        target: SummaryTarget,
        viewer: Option<Id>,
    ) -> TestResult<summaries::Cache> {
        let mut tx = self.db.begin().await?;
        let row = summaries::ensure(&mut tx, target, viewer, clock::start())
            .await?
            .ok_or("cache missing")?;
        tx.commit().await?;
        Ok(row)
    }

    async fn person(
        &self,
        cache: Id,
        issuer: Id,
        client: Id,
        args: &Json,
        operation: &Json,
    ) -> TestResult<Enqueued> {
        let mut tx = self.db.begin().await?;
        let result = summaries::enqueue(
            &mut tx,
            cache,
            Origin::Person {
                issuer,
                client_id: client,
                accepted_operation: operation,
            },
            args,
            args,
            clock::start(),
            clock::start(),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    async fn close(self) -> TestResult {
        self.db.pool().close().await;
        self.test.close().await?;
        Ok(())
    }
}

fn new(result: Enqueued) -> TestResult<summaries::Request> {
    match result {
        Enqueued::New(row) => Ok(row),
        other => Err(format!("expected new request, got {other:?}").into()),
    }
}

fn paragraph<'a>(source: &'a Json, refs: &'a Json) -> Outcome<'a> {
    Outcome::Paragraph(Paragraph {
        text: "The work is ready for review.",
        producer: source,
        source_snapshot: source,
        source_refs: refs,
    })
}

#[tokio::test]
async fn reads_do_not_create_work_and_targets_and_readers_have_distinct_caches() -> TestResult {
    let world = World::new().await?;
    let target = SummaryTarget::Document { id: world.document };
    let mut conn = world.db.acquire().await?;
    assert!(summaries::get(&mut conn, target, None).await?.is_none());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM resource_summary_requests")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(count, 0);
    drop(conn);
    let shared = world.cache(target, None).await?;
    let personal = world.cache(target, Some(world.ada)).await?;
    assert_ne!(shared.id, personal.id);
    assert_eq!(world.cache(target, None).await?.id, shared.id);
    let ada = world
        .cache(SummaryTarget::Chat { id: world.chat }, Some(world.ada))
        .await?;
    let bob = world
        .cache(SummaryTarget::Chat { id: world.chat }, Some(world.bob))
        .await?;
    assert_ne!(ada.id, bob.id);
    let mut tx = world.db.begin().await?;
    assert!(
        summaries::ensure(
            &mut tx,
            SummaryTarget::Topic { id: world.document },
            None,
            clock::start()
        )
        .await?
        .is_none()
    );
    assert!(
        summaries::ensure(
            &mut tx,
            SummaryTarget::Thread { id: world.topic },
            None,
            clock::start()
        )
        .await?
        .is_none()
    );
    assert!(
        summaries::ensure(
            &mut tx,
            SummaryTarget::Topic { id: world.root },
            None,
            clock::start()
        )
        .await?
        .is_none()
    );
    tx.commit().await?;
    world.close().await
}

#[tokio::test]
async fn a_persons_replay_keeps_its_original_origin_range_and_model_instead_of_broadening_it()
-> TestResult {
    let world = World::new().await?;
    let cache = world
        .cache(SummaryTarget::Chat { id: world.chat }, Some(world.ada))
        .await?;
    let original =
        json!({"operation":"summary.refresh","boundary":"original restricted credential"});
    let broader = json!({"operation":"summary.refresh","boundary":"later browser session"});
    let args = json!({"range":{"after":7,"through":12},"model":"chosen"});
    let client = new_id();
    let accepted = new(world
        .person(cache.id, world.ada, client, &args, &original)
        .await?)?;
    assert_eq!(accepted.requested_by, Some(world.ada));
    assert_eq!(accepted.paying_user, Some(world.ada));
    let Enqueued::Replay(replayed) = world
        .person(cache.id, world.ada, client, &args, &broader)
        .await?
    else {
        return Err("repeat did not replay".into());
    };
    assert_eq!(accepted, replayed);
    assert_eq!(replayed.accepted_operation, Some(original.clone()));
    assert_eq!(
        world
            .person(
                cache.id,
                world.ada,
                client,
                &json!({"range":{"after":12,"through":20}}),
                &broader
            )
            .await?,
        Enqueued::Conflict
    );
    let other = world
        .cache(
            SummaryTarget::Document { id: world.document },
            Some(world.ada),
        )
        .await?;
    assert_eq!(
        world
            .person(other.id, world.ada, client, &args, &broader)
            .await?,
        Enqueued::Conflict
    );
    assert_eq!(
        world
            .person(cache.id, world.ada, new_id(), &args, &original)
            .await?,
        Enqueued::Busy
    );
    assert_eq!(
        world
            .person(cache.id, world.bob, new_id(), &args, &original)
            .await?,
        Enqueued::WrongOrigin
    );
    let mut tx = world.db.begin().await?;
    assert_eq!(
        summaries::enqueue(
            &mut tx,
            cache.id,
            Origin::Automatic { writer: world.ada },
            &json!({}),
            &json!({}),
            clock::start(),
            clock::start()
        )
        .await?,
        Enqueued::WrongOrigin
    );
    tx.commit().await?;
    world.close().await
}

#[tokio::test]
async fn concurrent_repeats_accept_one_act_and_keep_one_original_capture() -> TestResult {
    let world = World::new().await?;
    let cache = world
        .cache(SummaryTarget::Topic { id: world.topic }, Some(world.ada))
        .await?;
    let client = new_id();
    let args = json!({});
    let op = json!({"operation":"summary.refresh"});
    let (a, b) = tokio::join!(
        world.person(cache.id, world.ada, client, &args, &op),
        world.person(cache.id, world.ada, client, &args, &op)
    );
    let (a, b) = (a?, b?);
    let ((Enqueued::New(first), Enqueued::Replay(second))
    | (Enqueued::Replay(second), Enqueued::New(first))) = (a, b)
    else {
        return Err("one original acceptance and one replay required".into());
    };
    assert_eq!(first.id, second.id);
    let mut conn = world.db.acquire().await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM resource_summary_requests")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(count, 1);
    drop(conn);
    world.close().await
}

#[tokio::test]
async fn reclaim_keeps_the_act_but_fences_its_old_worker_and_does_not_replace_prepared_material()
-> TestResult {
    let world = World::new().await?;
    let target = SummaryTarget::Document { id: world.document };
    let cache = world.cache(target, None).await?;
    let clock = clock::manual();
    let mut tx = world.db.begin().await?;
    let accepted = new(summaries::enqueue(
        &mut tx,
        cache.id,
        Origin::Automatic { writer: world.ada },
        &json!({}),
        &json!({}),
        clock.now(),
        clock.now(),
    )
    .await?)?;
    assert_eq!(
        accepted.requested_by, None,
        "writer is not an automatic request's issuer"
    );
    assert_eq!(accepted.paying_user, Some(world.ada));
    tx.commit().await?;
    let mut conn = world.db.acquire().await?;
    let first = summaries::claim(&mut conn, clock.now(), clock.now() + Duration::seconds(30))
        .await?
        .ok_or("first claim missing")?;
    let first_token = first.lease_token.ok_or("no token")?;
    let input = json!({"doc_seq":9,"model":"original"});
    assert!(summaries::prepare(&mut conn, first.id, first_token, clock.now(), &input).await?);
    assert!(
        !summaries::prepare(
            &mut conn,
            first.id,
            first_token,
            clock.now(),
            &json!({"doc_seq":10})
        )
        .await?
    );
    clock.advance(Duration::seconds(30));
    let expired = clock.now();
    drop(conn);
    let mut tx = world.db.begin().await?;
    assert!(
        !summaries::finish(
            &mut tx,
            first.id,
            first_token,
            &clock,
            paragraph(&input, &json!([]))
        )
        .await?,
        "an expired claim cannot publish even before reclaim"
    );
    tx.commit().await?;
    let mut conn = world.db.acquire().await?;
    let second = summaries::claim(&mut conn, expired, expired + Duration::seconds(30))
        .await?
        .ok_or("reclaim missing")?;
    assert_eq!(first.id, second.id);
    assert_eq!(second.prepared, Some(input.clone()));
    assert_ne!(first.lease_token, second.lease_token);
    assert!(!summaries::prepare(&mut conn, first.id, first_token, expired, &input).await?);
    drop(conn);
    let mut tx = world.db.begin().await?;
    assert!(
        !summaries::finish(
            &mut tx,
            first.id,
            first_token,
            &clock,
            paragraph(&input, &json!([]))
        )
        .await?
    );
    assert!(
        summaries::finish(
            &mut tx,
            second.id,
            second.lease_token.ok_or("no token")?,
            &clock,
            paragraph(&input, &json!([{"kind":"document","id":world.document}]))
        )
        .await?
    );
    tx.commit().await?;
    let mut conn = world.db.acquire().await?;
    let row = summaries::get(&mut conn, target, None)
        .await?
        .ok_or("cache missing")?;
    assert_eq!(row.completed_request_id, Some(accepted.id));
    assert_eq!(row.text.as_deref(), Some("The work is ready for review."));
    assert_eq!(
        summaries::request(&mut conn, first.id)
            .await?
            .ok_or("work missing")?
            .state,
        "done"
    );
    drop(conn);
    world.close().await
}

#[tokio::test]
async fn options_and_source_changes_discard_stale_output_and_failures_retain_the_previous_paragraph()
-> TestResult {
    let world = World::new().await?;
    let target = SummaryTarget::Document { id: world.document };
    let shared = world.cache(target, None).await?;
    let personal = world.cache(target, Some(world.ada)).await?;
    let clock = clock::manual();
    let mut tx = world.db.begin().await?;
    let first = new(summaries::enqueue(
        &mut tx,
        shared.id,
        Origin::Automatic { writer: world.ada },
        &json!({}),
        &json!({}),
        clock.now(),
        clock.now(),
    )
    .await?)?;
    tx.commit().await?;
    let input = json!({"doc_seq":1});
    let refs = json!([{"kind":"document","id":world.document}]);
    let mut conn = world.db.acquire().await?;
    let work = summaries::claim(&mut conn, clock.now(), clock.now() + Duration::seconds(30))
        .await?
        .ok_or("claim missing")?;
    let token = work.lease_token.ok_or("token missing")?;
    assert!(summaries::prepare(&mut conn, work.id, token, clock.now(), &input).await?);
    drop(conn);
    let mut tx = world.db.begin().await?;
    assert!(summaries::finish(&mut tx, work.id, token, &clock, paragraph(&input, &refs)).await?);
    tx.commit().await?;
    for invalidation in ["options", "source", "failure"] {
        let mut tx = world.db.begin().await?;
        let next = new(summaries::enqueue(
            &mut tx,
            shared.id,
            Origin::Automatic { writer: world.ada },
            &json!({}),
            &json!({}),
            clock.now(),
            clock.now(),
        )
        .await?)?;
        tx.commit().await?;
        let mut conn = world.db.acquire().await?;
        let work = summaries::claim(&mut conn, clock.now(), clock.now() + Duration::seconds(30))
            .await?
            .ok_or("claim missing")?;
        let token = work.lease_token.ok_or("token missing")?;
        assert!(summaries::prepare(&mut conn, work.id, token, clock.now(), &input).await?);
        if invalidation == "options" {
            let changed = summaries::configure(
                &mut conn,
                shared.id,
                0,
                Some(&json!({"automatic":false,"output_chars":1200})),
                world.ada,
                clock.now(),
            )
            .await?
            .ok_or("configure failed")?;
            assert_eq!(changed.options_revision, 1);
            assert!(
                summaries::configure(&mut conn, shared.id, 0, None, world.ada, clock.now())
                    .await?
                    .is_none(),
                "stale CAS"
            );
            assert!(
                summaries::configure(&mut conn, personal.id, 0, None, world.ada, clock.now())
                    .await?
                    .is_none(),
                "personal caches are not settings stores"
            );
            assert_eq!(
                summaries::get(&mut conn, target, Some(world.ada))
                    .await?
                    .ok_or("personal missing")?
                    .dirty_revision,
                1
            );
        } else if invalidation == "source" {
            assert_eq!(
                summaries::invalidate(&mut conn, target, clock.now()).await?,
                2
            );
        }
        drop(conn);
        let problem = json!({"code":"summary_failed"});
        let mut tx = world.db.begin().await?;
        let outcome = if invalidation == "failure" {
            Outcome::Failed(&problem)
        } else {
            paragraph(&input, &refs)
        };
        assert!(summaries::finish(&mut tx, work.id, token, &clock, outcome).await?);
        tx.commit().await?;
        let mut conn = world.db.acquire().await?;
        let cache = summaries::get(&mut conn, target, None)
            .await?
            .ok_or("cache missing")?;
        assert_eq!(cache.completed_request_id, Some(first.id));
        assert_eq!(cache.text.as_deref(), Some("The work is ready for review."));
        let final_work = summaries::latest(&mut conn, shared.id)
            .await?
            .ok_or("latest missing")?;
        assert_eq!(
            final_work.id, next.id,
            "acceptance order is stable with equal timestamps"
        );
        assert_eq!(
            final_work.state,
            if invalidation == "failure" {
                "failed"
            } else {
                "discarded"
            }
        );
    }
    world.close().await
}

#[tokio::test]
async fn replay_of_an_omitted_chat_range_keeps_the_original_server_capture() -> TestResult {
    let world = World::new().await?;
    let cache = world
        .cache(SummaryTarget::Chat { id: world.chat }, Some(world.ada))
        .await?;
    let client = new_id();
    let arguments = json!({"model":null});
    let operation = json!({"operation":"summary.refresh"});
    let first_source = json!({"range":{"after":7,"through":12}});
    let mut tx = world.db.begin().await?;
    let first = new(summaries::enqueue(
        &mut tx,
        cache.id,
        Origin::Person {
            issuer: world.ada,
            client_id: client,
            accepted_operation: &operation,
        },
        &arguments,
        &first_source,
        clock::start(),
        clock::start(),
    )
    .await?)?;
    tx.commit().await?;
    let mut tx = world.db.begin().await?;
    let result = summaries::enqueue(
        &mut tx,
        cache.id,
        Origin::Person {
            issuer: world.ada,
            client_id: client,
            accepted_operation: &operation,
        },
        &arguments,
        &json!({"range":{"after":12,"through":20}}),
        clock::start(),
        clock::start(),
    )
    .await?;
    let Enqueued::Replay(replayed) = result else {
        return Err("expected replay".into());
    };
    assert_eq!(first.id, replayed.id);
    assert_eq!(replayed.accepted_source, first_source);
    tx.commit().await?;
    world.close().await
}

#[tokio::test]
async fn a_change_inside_the_trigger_gap_stays_durable_until_its_due_time() -> TestResult {
    let world = World::new().await?;
    let cache = world
        .cache(SummaryTarget::Document { id: world.document }, None)
        .await?;
    let at = clock::start();
    let due = at + Duration::seconds(5);
    let mut tx = world.db.begin().await?;
    let accepted = new(summaries::enqueue(
        &mut tx,
        cache.id,
        Origin::Automatic { writer: world.ada },
        &json!({}),
        &json!({"doc_seq":2}),
        at,
        due,
    )
    .await?)?;
    tx.commit().await?;
    let mut conn = world.db.acquire().await?;
    assert!(
        summaries::claim(&mut conn, at, at + Duration::seconds(30))
            .await?
            .is_none()
    );
    assert!(
        summaries::claim(
            &mut conn,
            due - Duration::milliseconds(1),
            due + Duration::seconds(30)
        )
        .await?
        .is_none()
    );
    let work = summaries::claim(&mut conn, due, due + Duration::seconds(30))
        .await?
        .ok_or("due work missing")?;
    assert_eq!(work.id, accepted.id);
    assert_eq!(work.accepted_source, json!({"doc_seq":2}));
    assert_eq!(work.due_at, due);
    drop(conn);
    world.close().await
}
