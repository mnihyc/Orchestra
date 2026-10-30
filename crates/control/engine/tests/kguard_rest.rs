//! Direct engine document actions and guarded report application race access withdrawals in one order.
//! These prove trusted entry-point transactions, not native link delivery or stream-cursor replay.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "kguard_rest/commit_hold.rs"]
mod commit_hold;
mod common;
#[path = "kguard_rest/creation_probe.rs"]
mod creation_probe;
mod shared;

use orch_agent_contract::events::OrchTool;
use orch_db::queries::{access, resources, transaction_guard};
use orch_engine::tools::CallContext;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use shared::{Places, a_world_whose_agent_takes_tools, answer_to, entry, ok, share};
use std::time::Duration;

/// Repeated interleavings required to exercise both orders, rather than relying on one lucky schedule.
const ORDER_REPETITIONS: usize = 100;
/// Database lock observation has scheduling headroom on a loaded shared test machine.
const LOCK_HEADROOM: Duration = Duration::from_secs(60);
/// Observations yield between database reads; this is not an authority or expiry grace period.
const OBSERVATION_PAUSE: Duration = Duration::from_millis(10);
/// Setup plus the prescribed interleavings fit within this test's own rate budget.
const TOOL_CALL_BUDGET: i64 = 1000;
/// Each prescribed race has one competing commit. A stale attempt gets one
/// fresh transaction; another conflict is a test failure, not an unbounded retry.
const FENCE_ATTEMPTS: usize = 2;

fn start_call(
    w: &common::World,
    agent: Id,
    turn: Id,
    tool: OrchTool,
    arguments: Json,
) -> (
    String,
    tokio::task::JoinHandle<Result<(), orch_engine::error::EngineError>>,
) {
    let name = new_id().to_string();
    let call = CallContext {
        machine: w.machine_id,
        thread: agent,
        turn,
        call_ref: name.clone(),
        attempt: None,
        tool,
        arguments,
    };
    let engine = w.engine();
    (
        name,
        tokio::spawn(async move { engine.answer_tool_call(&call).await }),
    )
}

async fn waiting_on(w: &common::World, blocker: i32, phase: &str) -> i32 {
    tokio::time::timeout(LOCK_HEADROOM, async {
        loop {
            let waiting: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE datname=current_database()
                    AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[] ORDER BY pid LIMIT 1",
            )
            .bind(blocker)
            .fetch_optional(&mut *w.pool.acquire().await.unwrap())
            .await
            .unwrap();
            if let Some(pid) = waiting {
                return pid;
            }
            tokio::time::sleep(OBSERVATION_PAUSE).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("{phase}: no waiter reached backend {blocker} within {LOCK_HEADROOM:?}")
    })
}

/// Refusal must not install any part of the direct move. Hash synthetic material
/// rather than putting document bodies into a failed assertion's diagnostics.
async fn move_material(w: &common::World, document: Id, agent: Id, places: &[Id]) -> String {
    sqlx::query_scalar("SELECT md5(jsonb_build_object('document',to_jsonb(d),
        'blocks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM document_blocks b WHERE b.document_id=d.id),
        'history',(SELECT jsonb_agg(to_jsonb(v) ORDER BY v.version) FROM document_versions v WHERE v.document_id=d.id),
        'files',(SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM document_files f WHERE f.document_id=d.id),
        'moves',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM audit_log a WHERE a.object_id=d.id AND a.action='document.moved'),
        'receipts',(SELECT jsonb_agg(to_jsonb(o) ORDER BY o.request_ref) FROM document_tree_operations o WHERE o.actor_kind='agent' AND o.actor_id=$2),
        'scopes',(SELECT jsonb_agg(to_jsonb(s) ORDER BY s.node_id) FROM document_tree_scopes s WHERE s.node_id=ANY($3)))::text)
        FROM documents d WHERE d.id=$1")
        .bind(document).bind(agent).bind(places).fetch_one(w.pool.pool()).await.unwrap()
}

/// Read the same current agent reach as native creation, including independent document policy.
/// A surviving grant must fail this fixture's authority premise before it is called a commit-fence failure.
async fn creation_authority(w: &common::World, agent: Id, topic: Id) -> (bool, Json) {
    use orch_authz::reach::{Agent, AgentReach, DocumentPolicy, Reach, Things};
    use orch_settings::Place;
    use orch_settings_contract::registry::keys;
    let mut conn = w.pool.acquire().await.unwrap();
    let caller = Agent::of(&mut conn, agent).await.unwrap().unwrap();
    let policy = orch_settings::get::<String>(
        &mut conn,
        Place::Node(caller.topic),
        keys::SHARING_AGENT_REACH,
    )
    .await
    .unwrap()
    .as_deref()
    .and_then(AgentReach::parse)
    .unwrap_or_default();
    let documents = orch_settings::get::<String>(
        &mut conn,
        Place::Node(caller.topic),
        keys::SHARING_AGENT_DOCUMENTS,
    )
    .await
    .unwrap()
    .as_deref()
    .and_then(DocumentPolicy::parse)
    .unwrap_or_default();
    let grants =
        orch_authz::reach::grants(&mut conn, &caller, orch_types::Time::new(w.clock.now()))
            .await
            .unwrap();
    let sources:Vec<_>=grants.iter().filter(|grant|matches!(grant.permission,
        orch_access_contract::permissions::Permission::DocEdit|orch_access_contract::permissions::Permission::DocManage))
        .map(|grant|json!({"entry":grant.entry,"object":grant.page.id,"permission":grant.permission.as_str()})).collect();
    let same_topic = caller.topic == topic;
    let reach = Reach::load(
        &mut conn,
        caller,
        policy,
        orch_types::Time::new(w.clock.now()),
        &Things {
            nodes: vec![topic],
            documents: Vec::new(),
            people_chats: Vec::new(),
        },
    )
    .await
    .unwrap()
    .with_document_policy(documents);
    let standing = reach.documents_of(topic);
    (
        standing.writes(),
        json!({"same_topic":same_topic,"document_policy":format!("{documents:?}"),
        "read":standing.reads(),"write":format!("{:?}",standing.write),"manage":standing.manages(),"sources":sources}),
    )
}

/// A refused creation leaves all material at its destination and its structural scope unchanged.
async fn creation_material(w: &common::World, topic: Id) -> String {
    sqlx::query_scalar("SELECT md5(jsonb_build_object(
        'documents',(SELECT jsonb_agg(to_jsonb(d) ORDER BY d.id) FROM documents d WHERE d.node_id=$1),
        'blocks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM document_blocks b JOIN documents d ON d.id=b.document_id WHERE d.node_id=$1),
        'history',(SELECT jsonb_agg(to_jsonb(v) ORDER BY v.document_id,v.version) FROM document_versions v JOIN documents d ON d.id=v.document_id WHERE d.node_id=$1),
        'files',(SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM document_files f JOIN documents d ON d.id=f.document_id WHERE d.node_id=$1),
        'grants',(SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM permission_entries e JOIN documents d ON d.id=e.object_id
            WHERE e.object_kind='document' AND d.node_id=$1),
        'audit',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM audit_log a WHERE a.node_id=$1 AND a.action='document.created_by_agent'),
        'receipts',(SELECT jsonb_agg(to_jsonb(o) ORDER BY o.actor_id,o.request_ref) FROM document_tree_operations o
            WHERE o.arguments->'parent'=jsonb_build_object('kind','node_docs','id',$1::uuid)),
        'scope',(SELECT to_jsonb(s) FROM document_tree_scopes s WHERE s.node_id=$1))::text)")
        .bind(topic).fetch_one(w.pool.pool()).await.unwrap()
}

async fn withdraw(conn: &mut orch_db::Conn, entry: Id, at: chrono::DateTime<chrono::Utc>) {
    sqlx::query("UPDATE permission_entries SET removed_at = $2 WHERE id = $1")
        .bind(entry)
        .bind(at)
        .execute(&mut *conn)
        .await
        .unwrap();
    access::bump_epoch(conn).await.unwrap();
}

#[derive(Clone, Copy)]
enum Change {
    Machine(Id),
    Grant {
        entry: Id,
        remove_older_edits: Option<(Id, Id)>,
    },
}

fn start_change(
    w: &common::World,
    change: Change,
) -> (
    tokio::sync::oneshot::Receiver<i32>,
    tokio::task::JoinHandle<usize>,
) {
    let db = w.pool.clone();
    let at = w.clock.now();
    let (send, receive) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut send = Some(send);
        for attempt in 0..FENCE_ATTEMPTS {
            let mut tx = db.begin().await.unwrap();
            transaction_guard::lock_authority(&mut tx).await.unwrap();
            if let Some(send) = send.take() {
                let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                send.send(pid).unwrap();
            }
            match change {
                Change::Machine(machine) => {
                    assert!(
                        resources::revoke_machine(&mut tx, machine, at)
                            .await
                            .unwrap(),
                        "the prescribed revocation changes an active native machine"
                    );
                    access::bump_epoch(&mut tx).await.unwrap();
                }
                Change::Grant {
                    entry,
                    remove_older_edits,
                } => {
                    if let Some((topic, agent)) = remove_older_edits {
                        sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_id=$1 AND holder_id=$2
                            AND permission='doc.edit' AND removed_at IS NULL")
                            .bind(topic).bind(agent).bind(at).execute(&mut *tx).await.unwrap();
                    }
                    withdraw(&mut tx, entry, at).await;
                }
            }
            match tx.commit().await {
                Ok(()) => return attempt,
                Err(error) => {
                    let error = orch_db::DbError::from(error);
                    assert!(
                        attempt + 1 < FENCE_ATTEMPTS && matches!(&error, orch_db::DbError::Retry),
                        "the authority commit failed outside the single prescribed epoch retry: {error}"
                    );
                }
            }
        }
        unreachable!("each failed attempt either retries once or fails the proof")
    });
    (receive, task)
}

async fn waits_for(w: &common::World, waiter: i32, blocker: i32, phase: &str) {
    tokio::time::timeout(LOCK_HEADROOM,async {
        loop {
            let blocked:bool=sqlx::query_scalar("SELECT pg_blocking_pids($1) @> ARRAY[$2]::integer[]")
                .bind(waiter).bind(blocker).fetch_one(w.pool.pool()).await.unwrap();
            if blocked {return;}
            tokio::time::sleep(OBSERVATION_PAUSE).await;
        }
    }).await.unwrap_or_else(|_|panic!("{phase}: backend {waiter} did not wait on the actual winner {blocker} within {LOCK_HEADROOM:?}"));
}

fn start_report(
    w: &common::World,
    thread: Id,
    segment: Id,
    title: String,
    sequence: i64,
) -> tokio::task::JoinHandle<usize> {
    let engine = w.engine();
    let db = w.pool.clone();
    let machine = w.machine_id;
    let event =
        orch_link_contract::events::Event::SessionTitle(orch_link_contract::events::SessionTitle {
            title,
        });
    let delivered = orch_link::streams::DeliveredEvent {
        seq: sequence,
        at: orch_types::Time::new(w.clock.now()),
        kind: event.kind().as_str().to_owned(),
        body: event.body().unwrap(),
    };
    tokio::spawn(async move {
        for attempt in 0..FENCE_ATTEMPTS {
            let mut tx = db.begin().await.unwrap();
            engine
                .apply_stream_events(
                    &mut tx,
                    machine,
                    &orch_link_contract::streams::Stream::Session {
                        thread_id: thread,
                        segment_id: segment,
                    },
                    std::slice::from_ref(&delivered),
                )
                .await
                .unwrap();
            match tx.commit().await {
                Ok(()) => return attempt,
                Err(error) => {
                    let error = orch_db::DbError::from(error);
                    assert!(
                        attempt + 1 < FENCE_ATTEMPTS && matches!(&error, orch_db::DbError::Retry),
                        "the report commit failed outside the single prescribed epoch retry: {error}"
                    );
                }
            }
        }
        unreachable!("each failed attempt either retries once or fails the proof")
    })
}

#[tokio::test]
async fn machine_reports_and_revocation_in_both_orders_a_hundred_times_keep_only_reports_from_a_current_machine()
 {
    let w = common::World::new().await;
    let thread = w.thread("Automatic title").await;
    let segment = w.segment_of(thread).await;
    w.open_session_for_reports(thread, segment).await;
    commit_hold::install(&w).await;
    for round in 0..ORDER_REPETITIONS {
        sqlx::query("UPDATE machines SET status = 'active', online = true, offline_since = NULL WHERE id = $1")
            .bind(w.machine_id)
            .execute(w.pool.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE nodes SET name = 'Before report', title_source = 'auto' WHERE id = $1")
            .bind(thread)
            .execute(w.pool.pool())
            .await
            .unwrap();
        let hold = commit_hold::Hold::at(&w, commit_hold::Target::Machine(w.machine_id)).await;
        let (receive, revoke) = start_change(&w, Change::Machine(w.machine_id));
        let revoke_pid = receive.await.unwrap();
        assert_eq!(
            waiting_on(&w, hold.pid, "revocation first: hold its actual commit").await,
            revoke_pid,
            "revocation is held after the actual commit fence"
        );
        let sequence = i64::try_from(round).unwrap() + 1;
        let report = start_report(
            &w,
            thread,
            segment,
            format!("Refused report {round}"),
            sequence,
        );
        let _ = waiting_on(
            &w,
            revoke_pid,
            "revocation first: the report waits on that writer",
        )
        .await;
        hold.release().await;
        assert_eq!(revoke.await.unwrap(), 0);
        report.await.unwrap();
        let title: String = w
            .scalar("SELECT name FROM nodes WHERE id = $1", thread)
            .await;
        assert_eq!(title, "Before report");

        sqlx::query("UPDATE machines SET status = 'active', online = true, offline_since = NULL WHERE id = $1")
            .bind(w.machine_id)
            .execute(w.pool.pool())
            .await
            .unwrap();
        let mut object = w.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *object)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(thread)
            .execute(&mut *object)
            .await
            .unwrap();
        let hold = commit_hold::Hold::at(&w, commit_hold::Target::Report(thread)).await;
        let wanted = format!("Kept report {round}");
        let report = start_report(&w, thread, segment, wanted.clone(), sequence);
        let report_pid = waiting_on(&w, blocker, "report first: the original node row lock").await;
        object.rollback().await.unwrap();
        assert_eq!(
            waiting_on(&w, hold.pid, "report first: hold its actual commit").await,
            report_pid,
            "the same report reaches its actual commit fence"
        );
        let (receive, revoke) = start_change(&w, Change::Machine(w.machine_id));
        let revoke_pid = receive.await.unwrap();
        waits_for(
            &w,
            revoke_pid,
            report_pid,
            "report first: revocation waits for that exact commit",
        )
        .await;
        hold.release().await;
        assert_eq!(
            report.await.unwrap(),
            0,
            "the report which owns the commit fence is not displaced"
        );
        revoke.await.unwrap();
        let title: String = w
            .scalar("SELECT name FROM nodes WHERE id = $1", thread)
            .await;
        assert_eq!(
            title, wanted,
            "a report accepted first is applied before revocation"
        );
    }
}

#[tokio::test]
async fn creation_and_withdrawal_in_both_orders_a_hundred_times_refuse_only_the_later_creation() {
    let w = a_world_whose_agent_takes_tools().await;
    let agent = w.thread("Writer").await;
    // The fixture performs two creations per interleaving plus setup calls; rate limits are tested elsewhere.
    w.set(
        w.topic,
        "orch_tools.calls_per_minute",
        json!(TOOL_CALL_BUDGET),
    )
    .await;
    let topic = w.topic("Shared docs").await;
    share(&w, "node", topic, "live", "doc.view", "any", agent).await;
    let initial_grant = share(&w, "node", topic, "live", "doc.edit", "any", agent).await;
    let listed = ok(shared::call(
        &w,
        agent,
        orch_link_contract::commands::OrchTool::ListShared,
        json!({"path": format!("id:{topic}")}),
    )
    .await);
    let directory = entry(&listed, "docs")["path"].as_str().unwrap().to_owned();
    let turn = shared::prepare_call_turn(&w, agent).await;
    let mut initial = w.pool.begin().await.unwrap();
    transaction_guard::lock_authority(&mut initial)
        .await
        .unwrap();
    withdraw(&mut initial, initial_grant, w.clock.now()).await;
    initial.commit().await.unwrap();
    let (allowed, evidence) = creation_authority(&w, agent, topic).await;
    assert!(
        !allowed,
        "the initial withdrawal must leave no independent creation authority: {evidence}"
    );
    commit_hold::install(&w).await;
    creation_probe::install(&w).await;
    for round in 0..ORDER_REPETITIONS {
        let grant = share(&w, "node", topic, "live", "doc.edit", "any", agent).await;
        let (allowed, evidence) = creation_authority(&w, agent, topic).await;
        assert!(
            allowed,
            "round {round}: the exact active grant permits native creation: {evidence}"
        );
        let material = creation_material(&w, topic).await;
        let title = format!("Refused {round}");
        let invocation = creation_probe::prepare(
            &w,
            agent,
            turn,
            topic,
            &directory,
            &title,
            "Must not be written.",
        );
        let before = invocation.assert_fresh(&w, grant).await;
        let hold = commit_hold::Hold::at(&w, commit_hold::Target::Grant(grant)).await;
        let (receive, removal) = start_change(
            &w,
            Change::Grant {
                entry: grant,
                remove_older_edits: Some((topic, agent)),
            },
        );
        let removal_pid = receive.await.unwrap();
        assert_eq!(
            waiting_on(
                &w,
                hold.pid,
                "creation refused: withdrawal owns the commit fence"
            )
            .await,
            removal_pid
        );
        let action = invocation.start(&w);
        let waiter = waiting_on(
            &w,
            removal_pid,
            "creation refused: the stale action waits for withdrawal",
        )
        .await;
        hold.release().await;
        assert_eq!(removal.await.unwrap(), 0);
        let at_withdrawal = invocation.evidence(&w, grant).await;
        let (allowed, evidence) = creation_authority(&w, agent, topic).await;
        assert!(
            !allowed,
            "round {round}: a surviving authority makes this a fixture error, not a stale-create refusal: {evidence}"
        );
        action.await.unwrap().unwrap();
        let answer = answer_to(&w, agent, &invocation.call.call_ref).await;
        let after = invocation.evidence(&w, grant).await;
        invocation.assert_answer(&answer, &after);
        assert!(
            answer.error.is_some(),
            "round {round}: a fresh creation returned success after withdrawal left no current authority; observed_waiter={waiter}; withdrawal_backend={removal_pid}; reach={evidence}; before={before}; at_withdrawal={at_withdrawal}; after={after}"
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM documents WHERE node_id = $1 AND title = $2")
                .bind(topic)
                .bind(&title)
                .fetch_one(&mut *w.pool.acquire().await.unwrap())
                .await
                .unwrap();
        assert_eq!(count, 0, "withdrawal first leaves nothing written");
        assert_eq!(
            creation_material(&w, topic).await,
            material,
            "a refused creation changes no document, block, history, file, grant, audit, receipt or structural scope"
        );
        assert!(
            after["stored"]["document"].is_null()
                && after["stored"]["receipt"].is_null()
                && after["stored"]["producer"].is_null(),
            "a refused new identity has no committed effect or receipt: {after}"
        );

        let grant = share(&w, "node", topic, "live", "doc.edit", "any", agent).await;
        let title = format!("Kept {round}");
        let invocation = creation_probe::prepare(
            &w,
            agent,
            turn,
            topic,
            &directory,
            &title,
            "Written before withdrawal.",
        );
        invocation.assert_fresh(&w, grant).await;
        let mut object = w.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *object)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id = $1 FOR UPDATE")
            .bind(topic)
            .execute(&mut *object)
            .await
            .unwrap();
        let hold = commit_hold::Hold::at(
            &w,
            commit_hold::Target::Create {
                node: topic,
                title: &title,
            },
        )
        .await;
        let action = invocation.start(&w);
        let action_pid =
            waiting_on(&w, blocker, "creation first: the original parent row lock").await;
        object.rollback().await.unwrap();
        assert_eq!(
            waiting_on(&w, hold.pid, "creation first: hold its actual commit").await,
            action_pid,
            "the same action reaches its actual commit fence"
        );
        let (receive, removal) = start_change(
            &w,
            Change::Grant {
                entry: grant,
                remove_older_edits: None,
            },
        );
        let removal_pid = receive.await.unwrap();
        waits_for(
            &w,
            removal_pid,
            action_pid,
            "creation first: withdrawal waits for that exact commit",
        )
        .await;
        hold.release().await;
        action.await.unwrap().unwrap();
        removal.await.unwrap();
        let answer = answer_to(&w, agent, &invocation.call.call_ref).await;
        let committed = invocation.evidence(&w, grant).await;
        invocation.assert_answer(&answer, &committed);
        assert!(
            answer.error.is_none(),
            "creation first remains the accepted effect"
        );
        assert_eq!(
            committed["stored"]["document"]["id"],
            json!(invocation.document)
        );
        assert_eq!(committed["stored"]["receipt"]["document_matches"], true);
        assert_eq!(committed["stored"]["receipt"]["destination_matches"], true);
        assert_eq!(committed["stored"]["receipt"]["title_matches"], true);
        assert!(
            !committed["stored"]["producer"]["commit_reached_at"].is_null(),
            "the actual producer reached its commit observer"
        );
        assert_eq!(
            committed["stored"]["producer"]["pid"],
            json!(action_pid),
            "the winner's original waiter and committed producer are the same backend"
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM documents WHERE node_id = $1 AND title = $2")
                .bind(topic)
                .bind(title)
                .fetch_one(&mut *w.pool.acquire().await.unwrap())
                .await
                .unwrap();
        assert_eq!(count, 1, "creation first is kept exactly once");
        let material = creation_material(&w, topic).await;
        w.engine().answer_tool_call(&invocation.call).await.unwrap();
        let replay = answer_to(&w, agent, &invocation.call.call_ref).await;
        assert_eq!(
            replay, answer,
            "only the exact earlier call may retain its successful answer after withdrawal"
        );
        let replayed = invocation.evidence(&w, grant).await;
        assert_eq!(
            replayed["stored"]["document"],
            committed["stored"]["document"]
        );
        assert_eq!(
            replayed["stored"]["receipt"],
            committed["stored"]["receipt"]
        );
        assert_eq!(
            replayed["stored"]["producer"],
            committed["stored"]["producer"]
        );
        invocation.assert_answer(&replay, &replayed);
        assert_eq!(
            creation_material(&w, topic).await,
            material,
            "replay adds no document material, creation grant, audit, receipt or scope write"
        );
    }
}

#[tokio::test]
async fn moving_and_withdrawal_in_both_orders_a_hundred_times_keep_the_later_permissions_decision()
{
    let w = a_world_whose_agent_takes_tools().await;
    let agent = w.thread("Mover").await;
    w.set(
        w.topic,
        "orch_tools.calls_per_minute",
        json!(TOOL_CALL_BUDGET),
    )
    .await;
    let places = [w.topic("First docs").await, w.topic("Second docs").await];
    let mut directories = Vec::new();
    for place in places {
        for permission in ["doc.view", "doc.edit"] {
            share(&w, "node", place, "live", permission, "any", agent).await;
        }
        let listed = ok(shared::call(
            &w,
            agent,
            orch_link_contract::commands::OrchTool::ListShared,
            json!({"path":format!("id:{place}")}),
        )
        .await);
        directories.push(entry(&listed, "docs")["path"].as_str().unwrap().to_owned());
    }
    let document = w
        .document_beside(places[0], "Moving notes", &["The text stays."])
        .await;
    let turn = shared::prepare_call_turn(&w, agent).await;
    commit_hold::install(&w).await;
    for round in 0..ORDER_REPETITIONS {
        let source = places[round % places.len()];
        let destination = places[(round + 1) % places.len()];
        let to = &directories[(round + 1) % places.len()];
        let grant = share(&w, "node", source, "live", "doc.manage", "any", agent).await;
        let arguments = json!({"command":"move", "paths":[format!("id:{document}")], "to":to});
        let before = move_material(&w, document, agent, &places).await;
        // Move executes directly. The withdrawal owns the real commit fence
        // before its only call; there is no preview or synthetic authority lock.
        let hold = commit_hold::Hold::at(&w, commit_hold::Target::Grant(grant)).await;
        let (receive, removal) = start_change(
            &w,
            Change::Grant {
                entry: grant,
                remove_older_edits: None,
            },
        );
        let removal_pid = receive.await.unwrap();
        assert_eq!(
            waiting_on(
                &w,
                hold.pid,
                "move refused: withdrawal owns the commit fence"
            )
            .await,
            removal_pid
        );
        let (name, action) = start_call(&w, agent, turn, OrchTool::ManageShared, arguments.clone());
        let _ = waiting_on(
            &w,
            removal_pid,
            "move refused: the stale move waits for withdrawal",
        )
        .await;
        hold.release().await;
        assert_eq!(removal.await.unwrap(), 0);
        action.await.unwrap().unwrap();
        assert!(answer_to(&w, agent, &name).await.error.is_some());
        let node: Id = w
            .scalar("SELECT node_id FROM documents WHERE id = $1", document)
            .await;
        assert_eq!(
            node, source,
            "withdrawal first keeps the document where it was"
        );
        assert_eq!(
            move_material(&w, document, agent, &places).await,
            before,
            "a refused direct move changes no material, history, file, move audit, receipt or structural scope"
        );

        let grant = share(&w, "node", source, "live", "doc.manage", "any", agent).await;
        let mut object = w.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *object)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM documents WHERE id = $1 FOR UPDATE")
            .bind(document)
            .execute(&mut *object)
            .await
            .unwrap();
        let hold = commit_hold::Hold::at(&w, commit_hold::Target::Move(document)).await;
        let (name, action) = start_call(&w, agent, turn, OrchTool::ManageShared, arguments);
        let action_pid =
            waiting_on(&w, blocker, "move first: the original document row lock").await;
        object.rollback().await.unwrap();
        assert_eq!(
            waiting_on(&w, hold.pid, "move first: hold its actual commit").await,
            action_pid,
            "the direct move owns the production commit fence"
        );
        let (receive, removal) = start_change(
            &w,
            Change::Grant {
                entry: grant,
                remove_older_edits: None,
            },
        );
        let removal_pid = receive.await.unwrap();
        waits_for(
            &w,
            removal_pid,
            action_pid,
            "move first: withdrawal waits for that exact commit",
        )
        .await;
        hold.release().await;
        action.await.unwrap().unwrap();
        assert_eq!(
            removal.await.unwrap(),
            1,
            "the later authority change retries its stale epoch after the move commits"
        );
        let moved = ok(answer_to(&w, agent, &name).await);
        assert_eq!(
            moved["done"], true,
            "Move executes in the single direct call"
        );
        assert!(
            moved.get("confirm").is_none(),
            "only destructive deletion offers a confirmation"
        );
        let node: Id = w
            .scalar("SELECT node_id FROM documents WHERE id = $1", document)
            .await;
        assert_eq!(
            node, destination,
            "move first is kept before its permission is withdrawn"
        );
        let receipts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM document_tree_operations
            WHERE actor_kind='agent' AND actor_id=$1 AND request_ref=$2",
        )
        .bind(agent)
        .bind(&name)
        .fetch_one(w.pool.pool())
        .await
        .unwrap();
        assert_eq!(
            receipts, 1,
            "the successful direct move has exactly one committed receipt"
        );
        let moved: i64 = w
            .count(
                "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.moved'",
                document,
            )
            .await;
        assert_eq!(
            moved,
            i64::try_from(round + 1).unwrap(),
            "each successful ordering commits one move audit"
        );
    }
}
