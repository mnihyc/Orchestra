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
        let (out, mut receive) = mpsc::unbounded_channel();
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
        let Some(Outbound::Frame(initial)) = receive.recv().await else {
            panic!("the primary has its initial tool control");
        };
        assert!(
            matches!(*initial,BackplaneFrame::Down(Down {machine_id,frame:DownFrame::ToolSessionControl(_)}) if machine_id==machine)
        );
        Self {
            gateway,
            machine,
            session,
            out: receive,
        }
    }

    pub(super) async fn send(&self, app: &TestApp, stream: &Stream, seq: i64, event: &Event) {
        let mut frame = Ev::of(stream, seq, app.clock.time(), event).unwrap();
        frame.native_opening_id = match event {
            Event::SessionOpened(body) => Some(body.opening_id),
            Event::SessionLost(body) => Some(body.opening_id),
            _ => None,
        };
        app.app
            .link
            .up(
                &self.gateway,
                self.machine,
                self.session,
                UpFrame::Ev(frame),
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
    person: Id,
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
        threads::insert_segment(
            &mut conn,
            &threads::NewSegment {
                id: segment,
                thread_id: thread,
                agent_id: new_id(),
                provider_id: new_id(),
                environment_id: new_id(),
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
            person,
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

async fn tool_office() -> (Office, Id, Id) {
    use orch_db::queries::resources::access_fixtures as fixtures;
    let office = Office::new(true).await;
    let mut conn = office.db.pool.acquire().await.unwrap();
    let home: Id = sqlx::query_scalar("SELECT topic_id FROM threads WHERE node_id=$1")
        .bind(office.thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let environment = fixtures::insert_environment(
        &mut conn,
        office.person,
        Some(home),
        "Tool environment",
        "connected",
    )
    .await
    .unwrap();
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(office.person),
        Some(home),
        "Tool provider",
    )
    .await
    .unwrap();
    let agent = fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(office.person),
        Some(home),
        "Tool agent",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2,status='running',generation=1 WHERE id=$1")
        .bind(environment)
        .bind(office.owner)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE segments SET environment_id=$2,env_generation=1,provider_id=$3,agent_id=$4 WHERE id=$1").bind(office.segment).bind(environment).bind(provider).bind(agent).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2,provider_id=$3,agent_id=$4 WHERE node_id=$1")
        .bind(office.thread)
        .bind(environment)
        .bind(provider)
        .bind(agent)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    (office, environment, provider)
}

async fn admitted_tool_question(
    office: &Office,
    conn: &mut Conn,
    body: &Json,
    wanted: &orch_link_contract::commands::TicketWanted,
) -> orch_link_contract::tool_sessions::ToolSessionQuery {
    use orch_db::queries::link::{NewCommand, insert_command};
    let command = new_id();
    insert_command(
        conn,
        &NewCommand {
            native_target: None,
            id: command,
            machine_id: office.owner,
            kind: "session.open",
            body,
            serial_key: None,
            deadline_at: office.app.clock.now() + chrono::Duration::minutes(2),
        },
        office.app.clock.now(),
    )
    .await
    .unwrap();
    let facts = orch_db::queries::session_dependencies::command_tool_session(conn, command)
        .await
        .unwrap()
        .unwrap();
    let scope = Tickets::command_scope(&facts, wanted, office.owner).unwrap();
    let ticket = office
        .app
        .app
        .link
        .tickets()
        .for_session_request(wanted, office.owner, &scope)
        .unwrap();
    let claims = orch_link_contract::tunnels::Ticket::parse(&ticket)
        .unwrap()
        .claims()
        .clone();
    orch_link_contract::tool_sessions::ToolSessionQuery {
        n: new_id(),
        ticket,
        claims: Box::new(claims),
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn a_signed_tool_ticket_ends_with_its_actual_opening_and_cannot_join_a_replacement() {
    use orch_db::queries::link::{NewCommand, insert_command};
    use orch_link_contract::{
        commands::{TicketSlot, TicketWanted},
        tool_sessions::ToolSessionQuery,
        tunnels::{Ticket, TunnelKind},
    };
    let (office, environment, _provider) = tool_office().await;
    let link = &office.app.app.link;
    let mut conn = office.db.pool.acquire().await.unwrap();
    let wanted=TicketWanted {kind:TunnelKind::Exec,more:json!({"service":"exec/orch","dst":office.owner,"thread_id":office.thread,"environment_id":environment,"environment_generation":1}).as_object().unwrap().clone()};
    let mut slot = TicketSlot::ask(TunnelKind::Exec);
    if let TicketSlot::Asked(ask) = &mut slot {
        ask.ticket = wanted.clone();
    }
    let body = json!({"thread_id":office.thread,"segment_id":office.segment,"exec":{"environment_id":environment,"generation":1,"ticket":slot},"mcp":[],"skills":[]});
    let first = new_id();
    let next = new_id();
    let mut original = None;
    for command in [first, next] {
        insert_command(
            &mut conn,
            &NewCommand {
                native_target: None,
                id: command,
                machine_id: office.owner,
                kind: "session.open",
                body: &body,
                serial_key: None,
                deadline_at: office.app.clock.now() + chrono::Duration::minutes(2),
            },
            office.app.clock.now(),
        )
        .await
        .unwrap();
        let facts =
            orch_db::queries::session_dependencies::command_tool_session(&mut conn, command)
                .await
                .unwrap()
                .unwrap();
        let scope = Tickets::command_scope(&facts, &wanted, office.owner).unwrap();
        let ticket = link
            .tickets()
            .for_session_request(&wanted, office.owner, &scope)
            .unwrap();
        let claims = Ticket::parse(&ticket).unwrap().claims().clone();
        let query = ToolSessionQuery {
            n: new_id(),
            ticket,
            claims: Box::new(claims),
        };
        assert!(
            link.authorize_tool_session(&query).await.allowed,
            "the valid native session remains usable"
        );
        if let Some(old) = &original {
            assert!(
                !link.authorize_tool_session(old).await.allowed,
                "an older ticket cannot join this replacement"
            );
        }
        original = Some(query);
    }
    let close = json!({"thread_id":office.thread,"segment_id":office.segment});
    insert_command(
        &mut conn,
        &NewCommand {
            native_target: None,
            id: new_id(),
            machine_id: office.owner,
            kind: "session.close.fenced",
            body: &close,
            serial_key: None,
            deadline_at: office.app.clock.now() + chrono::Duration::minutes(2),
        },
        office.app.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        !link
            .authorize_tool_session(original.as_ref().unwrap())
            .await
            .allowed,
        "close refuses the signed ticket before a remote reply"
    );
    let mut builtin = wanted.clone();
    builtin.kind = TunnelKind::Mcp;
    builtin.more.insert("service".into(), json!("mcp/exec"));
    let mut slot = TicketSlot::ask(TunnelKind::Mcp);
    if let TicketSlot::Asked(ask) = &mut slot {
        ask.ticket = builtin.clone();
    }
    let body = json!({"thread_id":office.thread,"segment_id":office.segment,"exec":{"environment_id":environment,"generation":1,"ticket":slot},"mcp":[],"skills":[]});
    let opening = new_id();
    insert_command(
        &mut conn,
        &NewCommand {
            native_target: None,
            id: opening,
            machine_id: office.owner,
            kind: "session.open",
            body: &body,
            serial_key: None,
            deadline_at: office.app.clock.now() + chrono::Duration::minutes(2),
        },
        office.app.clock.now(),
    )
    .await
    .unwrap();
    let facts = orch_db::queries::session_dependencies::command_tool_session(&mut conn, opening)
        .await
        .unwrap()
        .unwrap();
    let scope = Tickets::command_scope(&facts, &builtin, office.owner).unwrap();
    assert_eq!(scope.member, "exec");
    let ticket = link
        .tickets()
        .for_session_request(&builtin, office.owner, &scope)
        .unwrap();
    let claims = Ticket::parse(&ticket).unwrap().claims().clone();
    assert!(
        link.authorize_tool_session(&ToolSessionQuery {
            n: new_id(),
            ticket,
            claims: Box::new(claims)
        })
        .await
        .allowed,
        "the builtin executor stays usable over its native MCP transport without an external server UUID"
    );
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_signed_tool_ticket_ends_with_its_actual_opening_and_cannot_join_a_replacement()
-> futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(
        self::a_signed_tool_ticket_ends_with_its_actual_opening_and_cannot_join_a_replacement(),
    )
}

fn raw() -> Event {
    Event::RawFrame(RawFrame {
        n: 1,
        direction: "in".to_owned(),
        payload: json!({"message": "private session output"}),
    })
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

#[cfg(any(test, feature = "test-support"))]
async fn a_tool_ticket_names_the_current_server_and_credential_keeper_even_after_a_stale_command_is_queued()
 {
    use orch_link_contract::commands::TicketSlot;
    use orch_link_contract::tunnels::{Service, TunnelKind};
    let (office, environment, provider) = tool_office().await;
    let link = &office.app.app.link;
    let mut conn = office.db.pool.acquire().await.unwrap();
    let server = new_id();
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',jsonb_build_object('mcp.add',true)) WHERE id=$1").bind(office.owner).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO mcp_servers(id,org_id,home_id,owner_id,environment_id,name,spec) SELECT $1,org_id,home_node_id,owner_id,id,'files',$3 FROM environments WHERE id=$2")
        .bind(server).bind(environment).bind(json!({"transport":"stdio","command":["true"]})).execute(&mut *conn).await.unwrap();
    let mut wanted = crate::tickets::request(
        TunnelKind::Mcp,
        &Service::Mcp {
            server: "files".into(),
        },
        office.owner,
        Some(office.thread),
    );
    wanted
        .more
        .insert("environment_id".into(), json!(environment));
    wanted
        .more
        .insert("environment_generation".into(), json!(1));
    let mut slot = TicketSlot::ask(TunnelKind::Mcp);
    if let TicketSlot::Asked(ask) = &mut slot {
        ask.ticket = wanted.clone();
    }
    let body = json!({"thread_id":office.thread,"segment_id":office.segment,"mcp":[{"server_id":server,"name":"files","ticket":slot}],"skills":[]});
    let current = admitted_tool_question(&office, &mut conn, &body, &wanted).await;
    assert!(
        link.authorize_tool_session(&current).await.allowed,
        "the current own server is usable"
    );
    let snapshot = link
        .tool_session_snapshot(office.segment)
        .await
        .unwrap()
        .unwrap();
    assert!(
        snapshot
            .members
            .iter()
            .any(|member| member.member == current.claims.tool_session.as_ref().unwrap().member)
    );
    sqlx::query("UPDATE mcp_servers SET name='other' WHERE id=$1")
        .bind(server)
        .execute(&mut *conn)
        .await
        .unwrap();
    let stale = admitted_tool_question(&office, &mut conn, &body, &wanted).await;
    assert!(
        !link.authorize_tool_session(&stale).await.allowed,
        "a command queued after rename cannot borrow the old service name for another server"
    );
    let snapshot = link
        .tool_session_snapshot(office.segment)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !snapshot
            .members
            .iter()
            .any(|member| member.member == stale.claims.tool_session.as_ref().unwrap().member),
        "the same stale member cannot be revalidated on a combined host by its local control snapshot"
    );
    sqlx::query("UPDATE mcp_servers SET name='files' WHERE id=$1")
        .bind(server)
        .execute(&mut *conn)
        .await
        .unwrap();
    let restored = admitted_tool_question(&office, &mut conn, &body, &wanted).await;
    assert!(link.authorize_tool_session(&restored).await.allowed);
    sqlx::query("UPDATE mcp_servers SET name='exec' WHERE id=$1")
        .bind(server)
        .execute(&mut *conn)
        .await
        .unwrap();
    let catalogue = orch_engine::mcp::resolve(&mut conn, environment, office.app.clock.time())
        .await
        .unwrap();
    let reserved = catalogue
        .iter()
        .find(|candidate| candidate.row.id == server)
        .unwrap();
    assert_eq!(
        reserved.reason_code,
        Some(orch_api_contract::environment::McpReasonCode::NameConflict),
        "a colliding discovered definition cannot be shown as a usable builtin executor"
    );
    sqlx::query("UPDATE mcp_servers SET name='files' WHERE id=$1")
        .bind(server)
        .execute(&mut *conn)
        .await
        .unwrap();
    let lease = new_id();
    assert!(
        threads::open_lease(
            &mut conn,
            &threads::NewLease {
                id: lease,
                provider_id: provider,
                thread_id: office.thread,
                mode: "forwarded",
                form: None,
                allowed_models: &[],
                allowance: None,
                at: office.app.clock.now()
            }
        )
        .await
        .unwrap()
        .is_some()
    );
    sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
        .bind(provider)
        .bind(office.owner)
        .execute(&mut *conn)
        .await
        .unwrap();
    let wanted = crate::tickets::request(
        TunnelKind::Llm,
        &Service::Llm { lease_id: lease },
        office.owner,
        Some(office.thread),
    );
    let mut slot = TicketSlot::ask(TunnelKind::Llm);
    if let TicketSlot::Asked(ask) = &mut slot {
        ask.ticket = wanted.clone();
    }
    let body = json!({"thread_id":office.thread,"segment_id":office.segment,"provider":{"path":"forwarded","provider_id":provider,"protocol":"openai-responses","ticket":slot},"mcp":[],"skills":[]});
    let current = admitted_tool_question(&office, &mut conn, &body, &wanted).await;
    assert!(
        link.authorize_tool_session(&current).await.allowed,
        "the actual credential keeper is usable"
    );
    sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
        .bind(provider)
        .bind(office.other)
        .execute(&mut *conn)
        .await
        .unwrap();
    let stale = admitted_tool_question(&office, &mut conn, &body, &wanted).await;
    assert!(
        !link.authorize_tool_session(&stale).await.allowed,
        "a fresh command cannot re-admit a ticket for the previous credential keeper"
    );
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_tool_ticket_names_the_current_server_and_credential_keeper_even_after_a_stale_command_is_queued()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(a_tool_ticket_names_the_current_server_and_credential_keeper_even_after_a_stale_command_is_queued())
}

#[cfg(any(test, feature = "test-support"))]
async fn an_admitted_provisional_opening_works_before_history_publication_and_its_removal_never_revives_the_predecessor()
 {
    use orch_link_contract::commands::{TicketSlot, TicketWanted};
    use orch_link_contract::tunnels::TunnelKind;
    let (office, environment, provider) = tool_office().await;
    let link = &office.app.app.link;
    let mut conn = office.db.pool.acquire().await.unwrap();
    let wanted=TicketWanted {kind:TunnelKind::Exec,more:json!({"service":"exec/orch","dst":office.owner,"thread_id":office.thread,"environment_id":environment,"environment_generation":1}).as_object().unwrap().clone()};
    let mut slot = TicketSlot::ask(TunnelKind::Exec);
    if let TicketSlot::Asked(ask) = &mut slot {
        ask.ticket = wanted.clone();
    }
    let mut body = json!({"thread_id":office.thread,"segment_id":office.segment,"exec":{"environment_id":environment,"generation":1,"ticket":slot},"mcp":[],"skills":[]});
    let previous = admitted_tool_question(&office, &mut conn, &body, &wanted).await;
    assert!(link.authorize_tool_session(&previous).await.allowed);
    let source = threads::segment(&mut conn, office.segment)
        .await
        .unwrap()
        .unwrap();
    let provisional = new_id();
    threads::insert_segment(
        &mut conn,
        &threads::NewSegment {
            id: provisional,
            thread_id: office.thread,
            agent_id: source.agent_id,
            provider_id: provider,
            environment_id: environment,
            env_generation: 1,
            arrangement: "separated",
            executor: Some("orch"),
            tool_set: None,
            credential_path: "native",
            native_session_ref: None,
            started_by: Some(office.person),
            started_at: office.app.clock.now(),
        },
        2,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE segments SET machine_id=$2 WHERE id=$1")
        .bind(provisional)
        .bind(office.owner)
        .execute(&mut *conn)
        .await
        .unwrap();
    body["segment_id"] = json!(provisional);
    let current = admitted_tool_question(&office, &mut conn, &body, &wanted).await;
    assert_eq!(
        threads::thread(&mut conn, office.thread)
            .await
            .unwrap()
            .unwrap()
            .current_segment_id,
        Some(office.segment),
        "the native opening precedes publication of its history selection"
    );
    assert!(
        link.authorize_tool_session(&current).await.allowed,
        "a valid admitted fork/import can use its own tools before publication"
    );
    assert!(!link.authorize_tool_session(&previous).await.allowed);
    sqlx::query("DELETE FROM segments WHERE id=$1")
        .bind(provisional)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !link.authorize_tool_session(&previous).await.allowed,
        "deleting an unpublished successor never revives its predecessor's old ticket"
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_admitted_provisional_opening_works_before_history_publication_and_its_removal_never_revives_the_predecessor()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(an_admitted_provisional_opening_works_before_history_publication_and_its_removal_never_revives_the_predecessor())
}

#[cfg(any(test, feature = "test-support"))]
async fn a_delayed_close_keeps_its_captured_absence_through_the_engine_dispatcher() {
    use crate::thread_engine::DispatchCommands;
    use orch_engine::commands::{Commands, CommandsError, Outgoing};
    use orch_link_contract::commands::{NativeTarget, SerialKey, SessionClose};
    use orch_link_contract::native_target::SessionTarget;

    let office = Office::new(true).await;
    let link = &office.app.app.link;
    let absent = NativeTarget::Session(SessionTarget::absent(office.thread));
    let close = Outgoing::new(
        new_id(),
        office.owner,
        SerialKey::Thread(office.thread),
        &SessionClose {
            thread_id: office.thread,
        },
    )
    .unwrap()
    .with_native_target(absent.clone());
    let mut conn = office.db.pool.acquire().await.unwrap();
    let opening = new_id();
    let body = json!({"thread_id":office.thread,"segment_id":office.segment,"mcp":[]});
    orch_db::queries::link::insert_command(
        &mut conn,
        &orch_db::queries::link::NewCommand {
            native_target: None,
            id: opening,
            machine_id: office.owner,
            kind: "session.open",
            body: &body,
            serial_key: None,
            deadline_at: office.app.clock.now() + chrono::Duration::minutes(2),
        },
        office.app.clock.now(),
    )
    .await
    .unwrap();
    let current = orch_db::queries::session_dependencies::target(
        &mut conn,
        office.thread,
        Some(office.segment),
    )
    .await
    .unwrap();
    assert_ne!(current, absent);
    drop(conn);

    let commands = DispatchCommands::new(link.dispatcher());
    commands.issue(close.clone()).await.unwrap();
    commands.issue(close.clone()).await.unwrap();
    let row = link.dispatcher().status(close.id).await.unwrap().unwrap();
    let entry = link.entry(office.owner).await.unwrap();
    let delivered = link
        .prepare(&entry, None, crate::dispatch::cmd_of(&row))
        .await
        .unwrap();
    assert_eq!(
        delivered.native_target,
        Some(absent.clone()),
        "a command without tickets still carries its original known absence"
    );
    let mut changed = close.clone();
    changed.native_target = Some(current);
    assert!(matches!(
        commands.issue(changed).await,
        Err(CommandsError::AuthorityMismatch)
    ));
    let mut conn = office.db.pool.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::session_dependencies::command_native_target(&mut conn, close.id)
            .await
            .unwrap(),
        Some(absent)
    );
    let live: bool = sqlx::query_scalar(
        "SELECT (delivered_resources#>>'{tool_admission,live}')::boolean FROM segments WHERE id=$1",
    )
    .bind(office.segment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(live, "the delayed close never withdraws the later opening");
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_delayed_close_keeps_its_captured_absence_through_the_engine_dispatcher()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(a_delayed_close_keeps_its_captured_absence_through_the_engine_dispatcher())
}

#[cfg(any(test, feature = "test-support"))]
async fn a_terminal_report_withdraws_later_mutable_observations_in_the_same_batch() {
    let office = Office::new(true).await;
    let mut conn = office.db.pool.acquire().await.unwrap();
    threads::leave_title_to_automation(&mut conn, office.thread)
        .await
        .unwrap();
    orch_db::queries::settings::put(
        &mut conn,
        Some(office.thread),
        orch_settings_contract::registry::keys::AUTO_TITLE,
        &orch_db::queries::settings::Stored {
            value: &json!("harness"),
            locked: false,
            by: Some(office.person),
            at: office.app.clock.now(),
        },
        None,
    )
    .await
    .unwrap();
    let opening = new_id();
    let body = json!({"thread_id":office.thread,"segment_id":office.segment,"mcp":[]});
    orch_db::queries::link::insert_command(
        &mut conn,
        &orch_db::queries::link::NewCommand {
            native_target: None,
            id: opening,
            machine_id: office.owner,
            kind: "session.open",
            body: &body,
            serial_key: None,
            deadline_at: office.app.clock.now() + chrono::Duration::minutes(2),
        },
        office.app.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let events = [
        Event::SessionTitle(orch_link_contract::events::SessionTitle {
            title: "Before the source ended".into(),
        }),
        Event::SessionLost(orch_link_contract::events::SessionLost {
            opening_id: opening,
            expected: true,
            reason: "The physical session ended".into(),
        }),
        Event::SessionTitle(orch_link_contract::events::SessionTitle {
            title: "A title after the source ended".into(),
        }),
    ];
    let reports: Vec<_> = events
        .iter()
        .zip(1_i64..)
        .map(|(event, seq)| orch_link::streams::DeliveredEvent {
            native_opening_id: Some(opening),
            seq,
            at: office.app.clock.time(),
            kind: event.kind().as_str().into(),
            body: event.body().unwrap(),
        })
        .collect();
    let mut tx = office.db.pool.begin().await.unwrap();
    office
        .app
        .app
        .engine
        .apply_stream_events(&mut tx, office.owner, &office.stream(), &reports)
        .await
        .unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1")
        .bind(office.thread)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(name, "Before the source ended");
    assert_eq!(
        orch_db::queries::session_dependencies::current_opening(&mut tx, office.segment)
            .await
            .unwrap(),
        None
    );
    tx.commit().await.unwrap();
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_terminal_report_withdraws_later_mutable_observations_in_the_same_batch()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(a_terminal_report_withdraws_later_mutable_observations_in_the_same_batch())
}
