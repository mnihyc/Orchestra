//! Session source refusals through the API's real link and database.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::*;

use crate::registry::Registry;

use crate::testing::TestApp;

use orch_db::queries::hierarchy;

use orch_db::queries::machine_streams;

use orch_db::queries::threads;

use orch_link_contract::events::Event;

use orch_link_contract::events::RawFrame;

use orch_link_contract::streams::Ev;

use orch_link_contract::streams::Reset;

use orch_testkit::clock;

use orch_testkit::database::TestDb;

use orch_types::new_id;

use serde_json::json;
// The production link runs asynchronously; allow its normal command acknowledgement bound.
const ACK_WAIT: Duration = Duration::from_secs(30);

pub struct Wire {
    gateway: Arc<GatewayConn>,
    machine: Id,
    session: Id,
    out: mpsc::UnboundedReceiver<Outbound>,
}

impl Wire {
    pub(super) async fn new(link: &Link, machine: Id) -> Self {
        let (out, receive) = mpsc::unbounded_channel();
        let gateway = Arc::new(GatewayConn {
            generation: 1,
            out,
            closed: CancellationToken::new(),
            stats: Mutex::new(None),
            started_at: orch_types::Time::new(chrono::Utc::now()),
            #[cfg(feature = "test-observers")]
            observed: orch_link::test_observers::QueueGauge::new("api.backplane.fixture"),
        });
        let session = new_id();
        link.attach(&gateway, machine, session, None).await;
        Self {
            gateway,
            machine,
            session,
            out: receive,
        }
    }

    pub(super) async fn send(&self, app: &TestApp, stream: &Stream, seq: i64, event: &Event) {
        app.app
            .link
            .up(
                &self.gateway,
                self.machine,
                self.session,
                UpFrame::Ev(Ev::of(stream, seq, app.clock.time(), event).unwrap()),
            )
            .await;
    }

    pub(super) async fn frame(&self, app: &TestApp, frame: UpFrame) {
        app.app
            .link
            .up(&self.gateway, self.machine, self.session, frame)
            .await;
    }

    /// Whether nothing has been sent back to the machine yet.
    pub(super) fn is_silent(&mut self) -> bool {
        self.out.try_recv().is_err()
    }

    pub(super) async fn ack(&mut self, stream: &Stream, seq: i64) {
        let outgoing = tokio::time::timeout(ACK_WAIT, self.out.recv())
            .await
            .unwrap()
            .unwrap();
        let Outbound::Frame(frame) = outgoing else {
            panic!("expected only an acknowledgement, received {outgoing:?}");
        };
        match *frame {
            BackplaneFrame::Down(Down {
                machine_id,
                frame: DownFrame::Ack(ack),
            }) => {
                assert_eq!(machine_id, self.machine);
                assert_eq!(ack.stream, stream.to_string());
                assert_eq!(ack.seq, seq);
            }
            frame => panic!("expected only an acknowledgement, received {frame:?}"),
        }
    }
}

struct Office {
    db: TestDb,
    app: TestApp,
    owner: Id,
    other: Id,
    thread: Id,
    segment: Id,
}

impl Office {
    async fn new(bound: bool) -> Self {
        let db = TestDb::fresh().await;
        let app = TestApp::new(
            Db::from_pool(db.pool.clone()),
            clock::manual(),
            Registry::new(),
        );
        app.app
            .link
            .set_applier(Arc::new(crate::thread_engine::EngineApplier::new(
                app.app.engine.clone(),
            )));
        let mut conn = db.pool.acquire().await.unwrap();
        let person = resources::testing::insert_user(&mut conn, "session-owner")
            .await
            .unwrap();
        let (root, project, topic, thread, segment) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        for (id, kind, parent) in [
            (root, "org", None),
            (project, "project", Some(root)),
            (topic, "topic", Some(project)),
        ] {
            hierarchy::testing::insert_node(&mut conn, id, kind, parent, kind, person)
                .await
                .unwrap();
        }
        assert!(
            threads::insert_thread(&mut conn, thread, topic, "Session", person, app.clock.now())
                .await
                .unwrap()
        );
        let owner = resources::testing::insert_machine(&mut conn, "host", "connected")
            .await
            .unwrap();
        let other = resources::testing::insert_machine(&mut conn, "other", "connected")
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET roles = ARRAY['agent'] WHERE id = ANY($1)")
            .bind(vec![owner, other])
            .execute(&mut *conn)
            .await
            .unwrap();
        let environment = resources::access_fixtures::insert_environment(
            &mut conn,
            person,
            Some(topic),
            "Session workspace",
            "connected",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE environments SET machine_id=$2 WHERE id=$1")
            .bind(environment)
            .bind(owner)
            .execute(&mut *conn)
            .await
            .unwrap();
        threads::insert_segment(
            &mut conn,
            &threads::NewSegment {
                id: segment,
                thread_id: thread,
                agent_id: new_id(),
                provider_id: new_id(),
                environment_id: environment,
                env_generation: 1,
                arrangement: "together",
                executor: None,
                tool_set: None,
                credential_path: "native",
                native_session_ref: None,
                started_by: Some(person),
                started_at: app.clock.now(),
            },
            1,
        )
        .await
        .unwrap();
        threads::set_current(&mut conn, thread, Some(segment), None, 1)
            .await
            .unwrap();
        if bound {
            assert!(
                machine_streams::bind_segment(&mut conn, thread, segment, owner)
                    .await
                    .unwrap()
            );
        }
        drop(conn);
        Self {
            db,
            app,
            owner,
            other,
            thread,
            segment,
        }
    }

    fn stream(&self) -> Stream {
        Stream::Session {
            thread_id: self.thread,
            segment_id: self.segment,
        }
    }

    async fn raw_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM raw_events WHERE segment_id = $1")
            .bind(self.segment)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn cursor(&self, machine: Id) -> Option<i64> {
        queries::cursors(&mut self.db.pool.acquire().await.unwrap(), machine)
            .await
            .unwrap()
            .get(&self.stream().to_string())
            .copied()
    }
}

fn raw() -> Event {
    Event::RawFrame(RawFrame {
        text: Vec::new(),
        text_complete: true,
        n: 1,
        direction: "in".to_owned(),
        payload: json!({"message": "private session output"}),
    })
}

#[cfg(any(test, feature = "test-support"))]
async fn a_command_reply_refused_by_storage_finishes_once_with_its_safe_gap_reason() {
    let office = Office::new(true).await;
    let id = new_id();
    let mut conn = office.db.pool.acquire().await.unwrap();
    queries::insert_command(
        &mut conn,
        &queries::NewCommand {
            id,
            machine_id: office.owner,
            kind: "fs.read",
            body: &json!({}),
            serial_key: None,
            deadline_at: office.app.clock.now() + chrono::Duration::hours(1),
        },
        office.app.clock.now(),
    )
    .await
    .unwrap();
    sqlx::raw_sql("CREATE FUNCTION reject_reply_data() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
          IF NEW.result->>'text' = 'rejected value' THEN
            RAISE EXCEPTION 'synthetic reply refusal' USING ERRCODE='22P05';
          END IF;
          RETURN NEW;
        END $$;
        CREATE TRIGGER reject_reply_data BEFORE UPDATE ON commands FOR EACH ROW EXECUTE FUNCTION reject_reply_data();")
        .execute(&mut *conn).await.unwrap();
    let progress = Progress::Succeeded(json!({"text":"rejected value"}));
    write_progress(&mut conn, id, &progress, &office.app.clock)
        .await
        .unwrap();
    write_progress(&mut conn, id, &progress, &office.app.clock)
        .await
        .unwrap();
    let (state, error): (String, serde_json::Value) =
        sqlx::query_as("SELECT state,error FROM commands WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(error["code"], "storage.data");
    assert_eq!(error["retryable"], false);
    let code: String =
        sqlx::query_scalar("SELECT sqlstate FROM command_storage_gaps WHERE command_id=$1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(code, "22P05");
    office.app.app.link.shutdown();
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_command_reply_refused_by_storage_finishes_once_with_its_safe_gap_reason()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(a_command_reply_refused_by_storage_finishes_once_with_its_safe_gap_reason())
}

#[cfg(any(test, feature = "test-support"))]
async fn a_rejected_native_value_is_acknowledged_once_with_a_gap_and_the_next_event_continues() {
    let office = Office::new(true).await;
    sqlx::raw_sql("CREATE SEQUENCE native_data_refusals;
        CREATE FUNCTION reject_native_data() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
          IF NEW.payload->>'text' = 'reject this value' THEN
            PERFORM nextval('native_data_refusals');
            RAISE EXCEPTION 'synthetic data refusal' USING ERRCODE='22P05';
          END IF;
          RETURN NEW;
        END $$;
        CREATE TRIGGER reject_native_data BEFORE INSERT OR UPDATE ON items FOR EACH ROW EXECUTE FUNCTION reject_native_data();")
        .execute(&office.db.pool).await.unwrap();
    let stream = office.stream();
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    for (seq, reference, text) in [
        (1, "before", "Before"),
        (2, "rejected", "reject this value"),
        (3, "after", "After"),
    ] {
        let event=Event::decode("item.started",json!({"turn_id":new_id(),"native_ref":reference,"stream":"main","kind":"agent_message","payload":{"text":text,"text_len":text.chars().count()}})).unwrap();
        owner.send(&office.app, &stream, seq, &event).await;
        owner.ack(&stream, seq).await;
    }
    assert_eq!(office.cursor(office.owner).await, Some(3));
    let refusals: i64 = sqlx::query_scalar("SELECT last_value FROM native_data_refusals")
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(
        refusals, 1,
        "a deterministic refusal is never resent as a transient failure"
    );
    let gap: serde_json::Value = sqlx::query_scalar(
        "SELECT storage_gap FROM items WHERE segment_id=$1 AND native_ref='rejected'",
    )
    .bind(office.segment)
    .fetch_one(&office.db.pool)
    .await
    .unwrap();
    assert_eq!(gap["sqlstate"], "22P05");
    assert_eq!(gap["event_seq"], 2);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE segment_id=$1 AND payload->>'text' IN ('Before','After')",
    )
    .bind(office.segment)
    .fetch_one(&office.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    office.app.app.link.shutdown();
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_rejected_native_value_is_acknowledged_once_with_a_gap_and_the_next_event_continues()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(a_rejected_native_value_is_acknowledged_once_with_a_gap_and_the_next_event_continues())
}

#[cfg(any(test, feature = "test-support"))]
async fn a_foreign_session_report_is_acknowledged_before_ordering_without_raw_or_a_cursor_even_after_reconnect()
 {
    let office = Office::new(true).await;
    let stream = office.stream();
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    other.send(&office.app, &stream, 41, &raw()).await;
    other.ack(&stream, 41).await;
    assert_eq!(office.raw_count().await, 0);
    assert_eq!(office.cursor(office.other).await, None);
    other.session = new_id();
    office
        .app
        .app
        .link
        .attach(&other.gateway, other.machine, other.session, None)
        .await;
    other.send(&office.app, &stream, 42, &raw()).await;
    other.ack(&stream, 42).await;
    assert_eq!(office.raw_count().await, 0);
    assert_eq!(office.cursor(office.other).await, None);

    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    owner.send(&office.app, &stream, 1, &raw()).await;
    owner.ack(&stream, 1).await;
    assert_eq!(office.raw_count().await, 1);
    assert_eq!(office.cursor(office.owner).await, Some(1));
    office.app.app.link.shutdown();
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_foreign_session_report_is_acknowledged_before_ordering_without_raw_or_a_cursor_even_after_reconnect()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::a_foreign_session_report_is_acknowledged_before_ordering_without_raw_or_a_cursor_even_after_reconnect())
}

#[cfg(any(test, feature = "test-support"))]
async fn an_unbound_segment_accepts_nobody_until_the_server_assigns_its_source() {
    let office = Office::new(false).await;
    let stream = office.stream();
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    owner.send(&office.app, &stream, 1, &raw()).await;
    owner.ack(&stream, 1).await;
    assert_eq!(office.raw_count().await, 0);
    assert_eq!(office.cursor(office.owner).await, None);
    assert!(
        machine_streams::bind_segment(
            &mut office.db.pool.acquire().await.unwrap(),
            office.thread,
            office.segment,
            office.owner
        )
        .await
        .unwrap()
    );
    owner.send(&office.app, &stream, 1, &raw()).await;
    owner.ack(&stream, 1).await;
    assert_eq!(office.raw_count().await, 1);
    assert_eq!(office.cursor(office.owner).await, Some(1));
    office.app.app.link.shutdown();
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_unbound_segment_accepts_nobody_until_the_server_assigns_its_source()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::an_unbound_segment_accepts_nobody_until_the_server_assigns_its_source(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn a_foreign_session_reset_writes_no_cursor_and_sends_no_resync_while_its_source_can_reset() {
    let office = Office::new(true).await;
    let stream = office.stream();
    let other = Wire::new(&office.app.app.link, office.other).await;
    office
        .app
        .app
        .link
        .up(
            &other.gateway,
            other.machine,
            other.session,
            UpFrame::Reset(Reset {
                stream: stream.to_string(),
                from: 11,
                reason: ResetReason::SpoolLost,
            }),
        )
        .await;
    assert_eq!(office.cursor(office.other).await, None);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // Applying a legitimate reset directly awaits its transaction and command issuance.
    let entry = office.app.app.link.entry(office.owner).await.unwrap();
    office
        .app
        .app
        .link
        .apply_reset(
            &entry,
            &orch_link::streams::ResetNotice {
                stream,
                from: 11,
                reason: ResetReason::SpoolLost,
                resync: true,
            },
        )
        .await;
    assert_eq!(office.cursor(office.owner).await, Some(10));
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM commands")
        .fetch_all(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(kinds, ["session.resync"]);
    office.app.app.link.shutdown();
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_foreign_session_reset_writes_no_cursor_and_sends_no_resync_while_its_source_can_reset()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::a_foreign_session_reset_writes_no_cursor_and_sends_no_resync_while_its_source_can_reset())
}

#[path = "session_source_tests/port_creation_replays.rs"]
pub mod port_creation_replays;
