//! Foreign environment, harness and host reports are dropped through the real link.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::session_source_tests::Wire;
use super::*;
use crate::{registry::Registry, testing::TestApp};
use orch_db::queries::hierarchy;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::events::{EnvStatus, Event, HarnessStatus, NodeRuntime, ProviderStatus};
use orch_link_contract::streams::{Eph, Ev, Evb, EvbEntry, StreamFamily};
use orch_testkit::{clock, database::TestDb};
use orch_types::new_id;
use serde_json::json;

struct Office {
    db: TestDb,
    app: TestApp,
    owner: Id,
    other: Id,
    environment: Id,
    other_environment: Id,
    provider: Id,
    agent: Id,
    runtime: Id,
    node_job: Id,
    file_job: Id,
}

impl Office {
    async fn session(&self) -> (Id, Id, Id, Id, Id) {
        let mut conn = self.db.pool.acquire().await.unwrap();
        let (person, root): (Id, Id) =
            sqlx::query_as("SELECT owner_id, org_id FROM providers WHERE id = $1")
                .bind(self.provider)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let (project, topic, thread, old, current, input, turn, lease) = (
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
        );
        hierarchy::testing::insert_node(
            &mut conn,
            project,
            "project",
            Some(root),
            "Project",
            person,
        )
        .await
        .unwrap();
        hierarchy::testing::insert_node(&mut conn, topic, "topic", Some(project), "Topic", person)
            .await
            .unwrap();
        assert!(
            orch_db::queries::threads::insert_thread(
                &mut conn,
                thread,
                topic,
                "Session",
                person,
                self.app.clock.now()
            )
            .await
            .unwrap()
        );
        for (segment, ordinal, machine) in [(old, 1_i32, self.other), (current, 2_i32, self.owner)]
        {
            sqlx::query("INSERT INTO segments(id,thread_id,ordinal,agent_id,provider_id,environment_id,env_generation,arrangement,credential_path,machine_id,seq_updated)
                         VALUES($1,$2,$3,$4,$5,$6,1,'together','native',$7,1)")
                .bind(segment).bind(thread).bind(ordinal).bind(self.agent).bind(self.provider).bind(self.environment).bind(machine)
                .execute(&mut *conn).await.unwrap();
        }
        sqlx::query(
            "UPDATE segments SET ended_at = $2, end_reason = 'environment_moved' WHERE id = $1",
        )
        .bind(old)
        .bind(self.app.clock.now())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO thread_inputs(id,thread_id,author_id,source,content,delivery,state,position,seq_updated)
                     VALUES($1,$2,$3,'user','[]','queue','delivered',1,1)")
            .bind(input).bind(thread).bind(person).execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO turns(id,thread_id,segment_id,ordinal,state,input_id,model,seq_updated) VALUES($1,$2,$3,1,'running',$4,'fake-1',1)")
            .bind(turn).bind(thread).bind(current).bind(input).execute(&mut *conn).await.unwrap();
        sqlx::query("UPDATE threads SET current_segment_id=$2,current_turn_id=$3,agent_id=$4,provider_id=$5,environment_id=$6,state='running' WHERE node_id=$1")
            .bind(thread).bind(current).bind(turn).bind(self.agent).bind(self.provider).bind(self.environment).execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO leases(id,provider_id,thread_id,turn_id,allowed_models) VALUES($1,$2,$3,$4,ARRAY['fake-1'])")
            .bind(lease).bind(self.provider).bind(thread).bind(turn).execute(&mut *conn).await.unwrap();
        sqlx::query("UPDATE turns SET lease_id=$2 WHERE id=$1")
            .bind(turn)
            .bind(lease)
            .execute(&mut *conn)
            .await
            .unwrap();
        (thread, old, current, turn, lease)
    }

    async fn new() -> Self {
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
        let person = resources::testing::insert_user(&mut conn, "report-owner")
            .await
            .unwrap();
        let root = new_id();
        hierarchy::testing::insert_node(&mut conn, root, "org", None, "Reports", person)
            .await
            .unwrap();
        let owner = resources::testing::insert_machine(&mut conn, "rightful", "connected")
            .await
            .unwrap();
        let other = resources::testing::insert_machine(&mut conn, "foreign", "connected")
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET roles = ARRAY['agent','environment','provider','node'] WHERE id = ANY($1)")
            .bind(vec![owner, other]).execute(&mut *conn).await.unwrap();
        let provider = resources::access_fixtures::insert_subscription(
            &mut conn, person, root, "Provider", "fake",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE providers SET machine_id = $2 WHERE id = $1")
            .bind(provider)
            .bind(owner)
            .execute(&mut *conn)
            .await
            .unwrap();
        let agent: Id = sqlx::query_scalar("SELECT agent_id FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET machine_id = $2 WHERE id = $1")
            .bind(agent)
            .bind(owner)
            .execute(&mut *conn)
            .await
            .unwrap();
        let template = new_id();
        sqlx::query(
            "INSERT INTO env_templates(id,name,driver,spec) VALUES($1,'Reports','docker','{}')",
        )
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
        let (environment, other_environment, runtime, other_runtime) =
            (new_id(), new_id(), new_id(), new_id());
        for (env, unit, machine, name) in [
            (environment, runtime, owner, "Rightful"),
            (other_environment, other_runtime, other, "Other"),
        ] {
            let host = new_id();
            sqlx::query("INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,$3)")
                .bind(host)
                .bind(machine)
                .bind(name)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,home_node_id,placement,template_id,machine_id,status,generation)
                         VALUES($1,$2,$3,$4,$4,'managed',$5,$6,'running',1)")
                .bind(env).bind(name).bind(person).bind(root).bind(template).bind(machine).execute(&mut *conn).await.unwrap();
            sqlx::query("INSERT INTO runtimes(id,kind,host_id,machine_id,unit_name,environment_id,driver,template_id,spec_hash,state)
                         VALUES($1,'environment',$2,$3,$4,$5,'docker',$6,$7,'running')")
                .bind(unit).bind(host).bind(machine).bind(format!("report-{unit}")).bind(env).bind(template).bind(vec![0u8;32])
                .execute(&mut *conn).await.unwrap();
        }
        let (node_job, file_job) = (new_id(), new_id());
        let host: Id = sqlx::query_scalar("SELECT host_id FROM runtimes WHERE id = $1")
            .bind(runtime)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        for (job, kind) in [(node_job, "environment_provision"), (file_job, "delivery")] {
            sqlx::query("INSERT INTO jobs(id,kind,environment_id,state,plan,progress) VALUES($1,$2,$3,'running',$4,$5)")
                .bind(job).bind(kind).bind(environment).bind(json!({"host":host,"runtime":runtime})).bind(progress(0))
                .execute(&mut *conn).await.unwrap();
        }
        drop(conn);
        Self {
            db,
            app,
            owner,
            other,
            environment,
            other_environment,
            provider,
            agent,
            runtime,
            node_job,
            file_job,
        }
    }

    async fn cursor(&self, machine: Id, stream: &Stream) -> Option<i64> {
        queries::cursors(&mut self.db.pool.acquire().await.unwrap(), machine)
            .await
            .unwrap()
            .get(&stream.to_string())
            .copied()
    }

    async fn value(&self, query: &'static str, object: Id) -> Json {
        sqlx::query_scalar(query)
            .bind(object)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn refuse(&self, wire: &mut Wire, stream: &Stream, seq: i64, event: &Event) {
        let mut tx = self.db.pool.begin().await.unwrap();
        self.app
            .app
            .engine
            .apply_stream_events(
                &mut tx,
                self.other,
                stream,
                &[DeliveredEvent {
                    seq,
                    at: self.app.clock.time(),
                    kind: event.kind().as_str().to_owned(),
                    body: event.body().unwrap(),
                }],
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        wire.send(&self.app, stream, seq, event).await;
        wire.ack(stream, seq).await;
        assert_eq!(self.cursor(self.other, stream).await, None);
    }
}

#[tokio::test]
async fn an_old_segments_machine_cannot_report_usage_for_a_new_segments_turn() {
    let office = Office::new().await;
    let (thread, old, current, turn, _) = office.session().await;
    let figures = json!({"input_tokens":11,"output_tokens":2,"total_tokens":13});
    let report = Event::decode("usage.updated", json!({"turn_id":turn,"turn":figures,"thread":figures,"context_window":null,"context_used":null})).unwrap();
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: old,
    };
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    office.refuse(&mut other, &stream, 1, &report).await;
    assert_eq!(
        office
            .value("SELECT coalesce(usage,'{}') FROM turns WHERE id=$1", turn)
            .await,
        json!({})
    );
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: current,
    };
    owner.send(&office.app, &stream, 1, &report).await;
    owner.ack(&stream, 1).await;
    assert_eq!(
        office
            .value("SELECT usage FROM turns WHERE id=$1", turn)
            .await["total_tokens"],
        13
    );
}

#[tokio::test]
async fn a_providers_machine_cannot_report_another_providers_lease() {
    let office = Office::new().await;
    let (_, _, _, turn, lease) = office.session().await;
    let mut conn = office.db.pool.acquire().await.unwrap();
    let (person, root): (Id, Id) =
        sqlx::query_as("SELECT owner_id,org_id FROM providers WHERE id=$1")
            .bind(office.provider)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let provider = resources::access_fixtures::insert_subscription(
        &mut conn,
        person,
        root,
        "Other provider",
        "fake",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
        .bind(provider)
        .bind(office.other)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let report = Event::decode("lease.usage",json!({"lease_id":lease,"model":"fake-1","usage":{"input_tokens":11,"output_tokens":2,"total_tokens":13},"request_ref":"owned-request"})).unwrap();
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    office
        .refuse(
            &mut other,
            &Stream::Provider {
                provider_id: provider,
            },
            1,
            &report,
        )
        .await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE turn_id=$1")
        .bind(turn)
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let stream = Stream::Provider {
        provider_id: office.provider,
    };
    owner.send(&office.app, &stream, 1, &report).await;
    owner.ack(&stream, 1).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE turn_id=$1 AND origin='forwarder'",
    )
    .bind(turn)
    .fetch_one(&office.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn an_environment_cannot_close_another_environments_browser_or_terminal() {
    let office = Office::new().await;
    let (browser, terminal) = (new_id(), new_id());
    let person: Id = sqlx::query_scalar("SELECT owner_id FROM environments WHERE id=$1")
        .bind(office.environment)
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO browser_sessions(id,environment_id,env_generation,origin,opened_by,endpoint,state) VALUES($1,$2,1,'started',$3,'http://127.0.0.1:41001','open')")
        .bind(browser).bind(office.environment).bind(person).execute(&office.db.pool).await.unwrap();
    sqlx::query("INSERT INTO terminal_sessions(id,environment_id,env_generation,opened_by,shell,cwd,cols,rows,state) VALUES($1,$2,1,$3,'/bin/sh','/workspace',80,24,'open')")
        .bind(terminal).bind(office.environment).bind(person).execute(&office.db.pool).await.unwrap();
    let reports = [
        Event::decode(
            "browser.state",
            json!({"browser_id":browser,"state":"closed","tabs":null}),
        )
        .unwrap(),
        Event::decode(
            "pty.exited",
            json!({"terminal_id":terminal,"exit_code":0,"bytes_out":0,"kept":null}),
        )
        .unwrap(),
    ];
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let stream = Stream::Env {
        environment_id: office.other_environment,
    };
    for (index, report) in reports.iter().enumerate() {
        office
            .refuse(&mut other, &stream, index as i64 + 1, report)
            .await;
    }
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(state) FROM browser_sessions WHERE id=$1",
                browser
            )
            .await,
        "open"
    );
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(state) FROM terminal_sessions WHERE id=$1",
                terminal
            )
            .await,
        "open"
    );
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let stream = Stream::Env {
        environment_id: office.environment,
    };
    for (index, report) in reports.iter().enumerate() {
        owner
            .send(&office.app, &stream, index as i64 + 1, report)
            .await;
        owner.ack(&stream, index as i64 + 1).await;
    }
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(state) FROM browser_sessions WHERE id=$1",
                browser
            )
            .await,
        "closed"
    );
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(state) FROM terminal_sessions WHERE id=$1",
                terminal
            )
            .await,
        "closed"
    );
}

impl Drop for Office {
    fn drop(&mut self) {
        self.app.app.link.shutdown();
    }
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

#[derive(sqlx::FromRow)]
struct RefusalAudit {
    details: Json,
    node_id: Option<Id>,
    org_id: Option<Id>,
    environment_id: Option<Id>,
    history: bool,
    actor_kind: String,
}

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn private_raw(n: i64) -> Event {
    Event::decode(
        "raw.frame",
        json!({"n":n,"direction":"in","payload":{"text":"refused-private-thread-content"}}),
    )
    .unwrap()
}

#[tokio::test]
async fn twenty_foreign_reports_are_acknowledged_counted_and_audited_once_without_content() {
    let office = Office::new().await;
    let (thread, _, segment, turn, _) = office.session().await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let capture = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    // The link's asynchronous tasks use the process subscriber, as in the running service.
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let events = (1..=20)
        .map(|seq| EvbEntry {
            seq,
            at: office.app.clock.time(),
            kind: "raw.frame".to_owned(),
            body: private_raw(seq).body().unwrap(),
        })
        .collect();
    other
        .frame(
            &office.app,
            UpFrame::Evb(Evb {
                stream: stream.to_string(),
                events,
            }),
        )
        .await;
    other.ack(&stream, 20).await;
    assert_eq!(office.cursor(office.other, &stream).await, None);
    assert_eq!(
        office
            .app
            .app
            .link
            .inner
            .refusals
            .total(office.other, StreamFamily::Session)
            .await,
        20
    );
    let raw: i64 = sqlx::query_scalar("SELECT count(*) FROM raw_events WHERE segment_id=$1")
        .bind(segment)
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(raw, 0);
    assert_eq!(
        office
            .value("SELECT to_jsonb(state) FROM turns WHERE id=$1", turn)
            .await,
        "running"
    );
    let rows:Vec<RefusalAudit> = sqlx::query_as("SELECT details,node_id,org_id,environment_id,history,actor_kind FROM audit_log WHERE action='machine.report_refused' AND object_id=$1")
        .bind(office.other).fetch_all(&office.db.pool).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].details,
        json!({"machine_id":office.other,"stream_kind":"session","count":20})
    );
    assert_eq!(
        (
            rows[0].node_id,
            rows[0].org_id,
            rows[0].environment_id,
            rows[0].history
        ),
        (None, None, None, false)
    );
    assert_eq!(rows[0].actor_kind, "system");
    let log = String::from_utf8(lock(&capture.0).clone()).unwrap();
    assert!(
        log.lines()
            .any(|line| line.contains(&office.other.to_string())
                && line.contains("session")
                && line.contains("count=20"))
    );
    assert!(!log.contains("refused-private-thread-content"));
    assert!(!log.contains(&thread.to_string()));
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    owner.send(&office.app, &stream, 1, &private_raw(1)).await;
    owner.ack(&stream, 1).await;
    assert_eq!(office.cursor(office.owner, &stream).await, Some(1));
    let raw: i64 = sqlx::query_scalar("SELECT count(*) FROM raw_events WHERE segment_id=$1")
        .bind(segment)
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(raw, 1);
    assert_eq!(
        office
            .app
            .app
            .link
            .inner
            .refusals
            .total(office.owner, StreamFamily::Session)
            .await,
        0
    );
}

#[tokio::test]
async fn separate_foreign_frames_keep_the_hourly_audit_append_only_and_carry_the_pending_count() {
    let office = Office::new().await;
    let (thread, _, segment, _, _) = office.session().await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    for seq in 1..=20 {
        other
            .send(&office.app, &stream, seq, &private_raw(seq))
            .await;
        other.ack(&stream, seq).await;
    }
    assert_eq!(
        office
            .app
            .app
            .link
            .inner
            .refusals
            .total(office.other, StreamFamily::Session)
            .await,
        20
    );
    let counts:Vec<i64>=sqlx::query_scalar("SELECT (details->>'count')::bigint FROM audit_log WHERE action='machine.report_refused' AND object_id=$1 ORDER BY at")
        .bind(office.other).fetch_all(&office.db.pool).await.unwrap();
    assert_eq!(counts, vec![1]);
    office.app.clock.advance(chrono::Duration::hours(1));
    other.send(&office.app, &stream, 21, &private_raw(21)).await;
    other.ack(&stream, 21).await;
    let counts:Vec<i64>=sqlx::query_scalar("SELECT (details->>'count')::bigint FROM audit_log WHERE action='machine.report_refused' AND object_id=$1 ORDER BY at")
        .bind(office.other).fetch_all(&office.db.pool).await.unwrap();
    assert_eq!(counts, vec![1, 20]);
    assert_eq!(office.cursor(office.other, &stream).await, None);
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    owner.send(&office.app, &stream, 1, &private_raw(1)).await;
    owner.ack(&stream, 1).await;
    assert_eq!(office.cursor(office.owner, &stream).await, Some(1));
}

#[tokio::test]
async fn replacing_an_api_process_does_not_write_a_second_refusal_row_in_the_same_hour() {
    let office = Office::new().await;
    let (thread, _, segment, _, _) = office.session().await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let mut first = Wire::new(&office.app.app.link, office.other).await;
    first.send(&office.app, &stream, 1, &private_raw(1)).await;
    first.ack(&stream, 1).await;
    let second = TestApp::new(
        Db::from_pool(office.db.pool.clone()),
        office.app.clock.clone(),
        Registry::new(),
    );
    let mut next = Wire::new(&second.app.link, office.other).await;
    next.send(&second, &stream, 2, &private_raw(2)).await;
    next.ack(&stream, 2).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='machine.report_refused' AND object_id=$1",
    )
    .bind(office.other)
    .fetch_one(&office.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(office.cursor(office.other, &stream).await, None);
    assert_eq!(
        second
            .app
            .link
            .inner
            .refusals
            .total(office.other, StreamFamily::Session)
            .await,
        1
    );
    second.app.link.shutdown();
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    owner.send(&office.app, &stream, 1, &private_raw(1)).await;
    owner.ack(&stream, 1).await;
    assert_eq!(office.cursor(office.owner, &stream).await, Some(1));
}

fn progress(percent: u32) -> Json {
    json!({"phase":"image","phases":[{"name":"image","weight":100,"state":"running"}],
        "percent":percent,"files_done":0,"files_total":null,"bytes_done":0,"bytes_total":null,
        "rate_bps":null,"eta_seconds":null,"message":null})
}

fn job_progress(job: Id) -> Event {
    Event::decode(
        "job.progress",
        json!({"job_id":job,"progress":progress(80),"log":"image progress"}),
    )
    .unwrap()
}

fn facts(root: &str) -> Event {
    Event::decode("env.facts", json!({"os":"linux","arch":"x86_64","shell":{"name":"sh","path":"/bin/sh"},
        "user":"agent","home":"/home/agent","workspace_root":root,"path_style":"posix","case_sensitive":true,
        "executors":[],"mcp_found":[],"index":"live","mounts":[],"private_views":false,"suspend":true,"git":true,"generation":1})).unwrap()
}

#[tokio::test]
async fn foreign_environment_facts_status_and_progress_are_dropped_and_the_source_is_taken() {
    let office = Office::new().await;
    let stream = Stream::Env {
        environment_id: office.environment,
    };
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let reports = [
        facts("/rightful"),
        Event::EnvStatus(EnvStatus {
            status: "offline".to_owned(),
            detail: None,
            generation: 1,
        }),
        job_progress(office.file_job),
    ];
    for (index, report) in reports.iter().enumerate() {
        office
            .refuse(&mut other, &stream, 41 + index as i64, report)
            .await;
    }
    assert_eq!(
        office
            .value(
                "SELECT facts FROM environments WHERE id = $1",
                office.environment
            )
            .await,
        json!({})
    );
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(status) FROM environments WHERE id = $1",
                office.environment
            )
            .await,
        "running"
    );
    assert_eq!(
        office
            .value("SELECT progress FROM jobs WHERE id = $1", office.file_job)
            .await["percent"],
        0
    );
    for (index, report) in reports.iter().enumerate() {
        owner
            .send(&office.app, &stream, index as i64 + 1, report)
            .await;
        owner.ack(&stream, index as i64 + 1).await;
    }
    assert_eq!(
        office
            .value(
                "SELECT facts FROM environments WHERE id = $1",
                office.environment
            )
            .await["workspace_root"],
        "/rightful"
    );
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(status) FROM environments WHERE id = $1",
                office.environment
            )
            .await,
        "offline"
    );
    assert_eq!(
        office
            .value("SELECT progress FROM jobs WHERE id = $1", office.file_job)
            .await["percent"],
        80
    );
    assert_eq!(office.cursor(office.owner, &stream).await, Some(3));
}

#[tokio::test]
async fn a_harness_and_provider_take_reports_only_from_their_registered_machine() {
    let office = Office::new().await;
    let stream = Stream::Harness {
        provider_id: office.provider,
        agent_id: office.agent,
    };
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let reports = [
        Event::HarnessStatus(HarnessStatus {
            status: "online".to_owned(),
            detail: None,
            harness_version: Some("1.0".to_owned()),
            sessions_open: 2,
        }),
        Event::HarnessDescriptor(serde_json::from_value(json!({
            "kind":"example","adapter_version":1,
            "harness":{"name":"Example","version":"1","supported_range":"*","status":"supported"},
            "arrangements":[],"x_reported":"rightful"
        })).unwrap()),
    ];
    for (index, report) in reports.iter().enumerate() {
        office
            .refuse(&mut other, &stream, 41 + index as i64, report)
            .await;
    }
    assert_eq!(
        office
            .value("SELECT descriptor FROM agents WHERE id = $1", office.agent)
            .await,
        json!({})
    );
    let harnesses: i64 =
        sqlx::query_scalar("SELECT count(*) FROM harnesses WHERE provider_id = $1")
            .bind(office.provider)
            .fetch_one(&office.db.pool)
            .await
            .unwrap();
    assert_eq!(harnesses, 0);
    for (index, report) in reports.iter().enumerate() {
        owner
            .send(&office.app, &stream, index as i64 + 1, report)
            .await;
        owner.ack(&stream, index as i64 + 1).await;
    }
    assert_eq!(
        office
            .value("SELECT descriptor FROM agents WHERE id = $1", office.agent)
            .await,
        reports[1].body().unwrap()
    );
    let stream = Stream::Provider {
        provider_id: office.provider,
    };
    let report = Event::ProviderStatus(ProviderStatus {
        status: "exhausted".to_owned(),
        detail: None,
    });
    office.refuse(&mut other, &stream, 41, &report).await;
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(status) FROM providers WHERE id = $1",
                office.provider
            )
            .await,
        "ready"
    );
    owner.send(&office.app, &stream, 1, &report).await;
    owner.ack(&stream, 1).await;
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(status) FROM providers WHERE id = $1",
                office.provider
            )
            .await,
        "exhausted"
    );
}

#[tokio::test]
async fn a_managed_profile_reports_from_a_requested_colocated_harness_and_a_pinned_profile_cannot_move()
 {
    let office = Office::new().await;
    let stream = Stream::Harness {
        provider_id: office.provider,
        agent_id: office.agent,
    };
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let report = Event::HarnessStatus(HarnessStatus {
        status: "online".to_owned(),
        detail: None,
        harness_version: Some("1.0".to_owned()),
        sessions_open: 0,
    });
    sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,deadline_at) VALUES($1,$2,'agent.start',$3,'sent',$4)")
        .bind(new_id()).bind(office.other).bind(json!({"provider_id":office.provider,"agent_id":office.agent}))
        .bind(office.app.clock.now()+chrono::Duration::minutes(1)).execute(&office.db.pool).await.unwrap();
    office.refuse(&mut other, &stream, 41, &report).await;
    sqlx::query("UPDATE agents SET machine_id = NULL WHERE id = $1")
        .bind(office.agent)
        .execute(&office.db.pool)
        .await
        .unwrap();
    other.send(&office.app, &stream, 1, &report).await;
    other.ack(&stream, 1).await;
    assert_eq!(office.cursor(office.other, &stream).await, Some(1));
    let machines: Vec<Id> =
        sqlx::query_scalar("SELECT machine_id FROM harnesses WHERE provider_id = $1")
            .bind(office.provider)
            .fetch_all(&office.db.pool)
            .await
            .unwrap();
    assert_eq!(machines, vec![office.other]);
}

#[tokio::test]
async fn a_node_cannot_report_another_hosts_runtime_or_job_and_the_registered_node_can() {
    let office = Office::new().await;
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let runtime = Event::NodeRuntime(NodeRuntime {
        runtime_id: office.runtime,
        state: "absent".to_owned(),
        detail: None,
    });
    office.refuse(&mut other, &Stream::Node, 41, &runtime).await;
    office
        .refuse(
            &mut other,
            &Stream::Node,
            42,
            &job_progress(office.node_job),
        )
        .await;
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(state) FROM runtimes WHERE id = $1",
                office.runtime
            )
            .await,
        "running"
    );
    assert_eq!(
        office
            .value("SELECT progress FROM jobs WHERE id = $1", office.node_job)
            .await["percent"],
        0
    );
    owner
        .send(
            &office.app,
            &Stream::Node,
            1,
            &job_progress(office.node_job),
        )
        .await;
    owner.ack(&Stream::Node, 1).await;
    assert_eq!(
        office
            .value("SELECT progress FROM jobs WHERE id = $1", office.node_job)
            .await["percent"],
        80
    );
    owner.send(&office.app, &Stream::Node, 2, &runtime).await;
    owner.ack(&Stream::Node, 2).await;
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(state) FROM runtimes WHERE id = $1",
                office.runtime
            )
            .await,
        "failed"
    );
}

#[tokio::test]
async fn a_foreign_report_in_a_node_batch_applies_none_of_the_batch() {
    let office = Office::new().await;
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let own_load = json!({"cpu_percent":99.0,"memory_used_mb":1,"disk_used_gb":1,"disk_free_gb":1});
    other
        .frame(
            &office.app,
            UpFrame::Evb(Evb {
                stream: Stream::Node.to_string(),
                events: vec![
                    EvbEntry {
                        seq: 1,
                        at: office.app.clock.time(),
                        kind: "node.load".to_owned(),
                        body: own_load.clone(),
                    },
                    EvbEntry {
                        seq: 2,
                        at: office.app.clock.time(),
                        kind: "node.runtime".to_owned(),
                        body: json!({"runtime_id":office.runtime,"state":"absent","detail":null}),
                    },
                ],
            }),
        )
        .await;
    other.ack(&Stream::Node, 2).await;
    let load: Json = sqlx::query_scalar("SELECT load FROM hosts WHERE machine_id = $1")
        .bind(office.other)
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(load, json!({}));
    assert_eq!(office.cursor(office.other, &Stream::Node).await, None);
    other
        .send(
            &office.app,
            &Stream::Node,
            3,
            &Event::decode("node.load", own_load.clone()).unwrap(),
        )
        .await;
    other.ack(&Stream::Node, 3).await;
    let load: Json = sqlx::query_scalar("SELECT load FROM hosts WHERE machine_id = $1")
        .bind(office.other)
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(load, own_load);
    assert_eq!(office.cursor(office.other, &Stream::Node).await, Some(3));
}

#[tokio::test]
async fn discarding_a_foreign_runtime_does_not_block_the_nodes_next_own_report() {
    let office = Office::new().await;
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let foreign = Event::NodeRuntime(NodeRuntime {
        runtime_id: office.runtime,
        state: "absent".to_owned(),
        detail: None,
    });
    office.refuse(&mut other, &Stream::Node, 1, &foreign).await;
    let load = json!({"cpu_percent":22.0,"memory_used_mb":1,"disk_used_gb":1,"disk_free_gb":1});
    other
        .send(
            &office.app,
            &Stream::Node,
            2,
            &Event::decode("node.load", load.clone()).unwrap(),
        )
        .await;
    other.ack(&Stream::Node, 2).await;
    assert_eq!(office.cursor(office.other, &Stream::Node).await, Some(2));
    let recorded: Json = sqlx::query_scalar("SELECT load FROM hosts WHERE machine_id = $1")
        .bind(office.other)
        .fetch_one(&office.db.pool)
        .await
        .unwrap();
    assert_eq!(recorded, load);
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(state) FROM runtimes WHERE id = $1",
                office.runtime
            )
            .await,
        "running"
    );
}

#[tokio::test]
async fn a_foreign_environment_measurement_is_never_published_and_the_source_is_published() {
    let office = Office::new().await;
    let mut reports = office.app.app.link.reports();
    let other = Wire::new(&office.app.app.link, office.other).await;
    let owner = Wire::new(&office.app.app.link, office.owner).await;
    let eph = Eph {
        stream: Stream::Env {
            environment_id: office.environment,
        }
        .to_string(),
        kind: "procs.snapshot".to_owned(),
        at: office.app.clock.time(),
        body: json!({"as_of":office.app.clock.time(),"processes":[]}),
    };
    other.frame(&office.app, UpFrame::Eph(eph.clone())).await;
    // Ingress awaits the source check, so absence here does not depend on a short sleep.
    assert!(matches!(
        reports.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    owner.frame(&office.app, UpFrame::Eph(eph.clone())).await;
    let report = tokio::time::timeout(Duration::from_secs(30), reports.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.machine_id, office.owner);
    assert_eq!(report.eph, eph);
}

#[tokio::test]
async fn an_environment_stream_cannot_update_a_job_of_another_environment() {
    let office = Office::new().await;
    let stream = Stream::Env {
        environment_id: office.other_environment,
    };
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    office
        .refuse(&mut other, &stream, 41, &job_progress(office.file_job))
        .await;
    assert_eq!(
        office
            .value("SELECT progress FROM jobs WHERE id = $1", office.file_job)
            .await["percent"],
        0
    );
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let stream = Stream::Env {
        environment_id: office.environment,
    };
    owner
        .send(&office.app, &stream, 1, &job_progress(office.file_job))
        .await;
    owner.ack(&stream, 1).await;
    assert_eq!(
        office
            .value("SELECT progress FROM jobs WHERE id = $1", office.file_job)
            .await["percent"],
        80
    );
}

#[tokio::test]
async fn a_move_destination_reports_only_requested_work_and_its_facts_wait_for_commit() {
    let office = Office::new().await;
    let (candidate, job) = (new_id(), new_id());
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,machine_id,unit_name,environment_id,driver,template_id,spec_hash,state,successor_of)
                 SELECT $1,kind,(SELECT id FROM hosts WHERE machine_id = $2),$2,$3,environment_id,driver,template_id,spec_hash,'creating',id FROM runtimes WHERE id = $4")
        .bind(candidate).bind(office.other).bind(format!("candidate-{candidate}")).bind(office.runtime).execute(&office.db.pool).await.unwrap();
    sqlx::query("INSERT INTO jobs(id,kind,environment_id,state,plan) VALUES($1,'environment_move',$2,'running',$3)")
        .bind(job).bind(office.environment).bind(json!({"runtime":candidate})).execute(&office.db.pool).await.unwrap();
    sqlx::query("UPDATE environments SET status='moving' WHERE id=$1")
        .bind(office.environment)
        .execute(&office.db.pool)
        .await
        .unwrap();
    let stream = Stream::Env {
        environment_id: office.environment,
    };
    let mut destination = Wire::new(&office.app.app.link, office.other).await;
    office
        .refuse(&mut destination, &stream, 41, &facts("/candidate"))
        .await;
    assert_eq!(
        office
            .value("SELECT plan FROM jobs WHERE id = $1", job)
            .await
            .get("candidate_facts"),
        None
    );
    sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,deadline_at) VALUES($1,$2,'env.attach',$3,'sent',$4)")
        .bind(new_id()).bind(office.other).bind(json!({"environment_id":office.environment})).bind(office.app.clock.now() + chrono::Duration::minutes(1))
        .execute(&office.db.pool).await.unwrap();
    destination
        .send(
            &office.app,
            &stream,
            1,
            &Event::EnvStatus(EnvStatus {
                status: "running".to_owned(),
                detail: None,
                generation: 1,
            }),
        )
        .await;
    destination.ack(&stream, 1).await;
    assert_eq!(
        office
            .value(
                "SELECT to_jsonb(status) FROM environments WHERE id=$1",
                office.environment
            )
            .await,
        "moving"
    );
    assert_eq!(
        office.value("SELECT plan FROM jobs WHERE id=$1", job).await["candidate_status"]["status"],
        "running"
    );
    destination
        .send(&office.app, &stream, 2, &facts("/candidate"))
        .await;
    destination.ack(&stream, 2).await;
    assert_eq!(
        office
            .value("SELECT plan FROM jobs WHERE id = $1", job)
            .await["candidate_facts"]["workspace_root"],
        "/candidate"
    );
    assert_eq!(
        office
            .value(
                "SELECT facts FROM environments WHERE id = $1",
                office.environment
            )
            .await,
        json!({})
    );
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    owner.send(&office.app, &stream, 1, &facts("/source")).await;
    owner.ack(&stream, 1).await;
    assert_eq!(
        office
            .value(
                "SELECT facts FROM environments WHERE id = $1",
                office.environment
            )
            .await["workspace_root"],
        "/source"
    );
    // A requested attach does not grant authority over an unrelated job.
    destination
        .send(&office.app, &stream, 42, &job_progress(office.file_job))
        .await;
    destination.ack(&stream, 42).await;
    assert_eq!(office.cursor(office.other, &stream).await, Some(2));
    sqlx::query("UPDATE jobs SET state = 'failed' WHERE id = $1")
        .bind(job)
        .execute(&office.db.pool)
        .await
        .unwrap();
    destination
        .send(&office.app, &stream, 43, &facts("/stale"))
        .await;
    destination.ack(&stream, 43).await;
    assert_eq!(
        office
            .value("SELECT plan FROM jobs WHERE id = $1", job)
            .await["candidate_facts"]["workspace_root"],
        "/candidate"
    );
}

#[tokio::test]
async fn a_runtime_network_report_obeys_the_runtime_hosts_source_binding() {
    let office = Office::new().await;
    let mut other = Wire::new(&office.app.app.link, office.other).await;
    let mut owner = Wire::new(&office.app.app.link, office.owner).await;
    let report = Ev {
        stream: Stream::Node.to_string(),
        seq: 41,
        at: office.app.clock.time(),
        kind: "node.network".to_owned(),
        body: json!({"runtime_id":office.runtime,"network":{"requested":"restricted"}}),
    };
    other.frame(&office.app, UpFrame::Ev(report.clone())).await;
    other.ack(&Stream::Node, 41).await;
    assert_eq!(office.cursor(office.other, &Stream::Node).await, None);
    owner
        .frame(&office.app, UpFrame::Ev(Ev { seq: 1, ..report }))
        .await;
    owner.ack(&Stream::Node, 1).await;
    assert_eq!(office.cursor(office.owner, &Stream::Node).await, Some(1));
}
