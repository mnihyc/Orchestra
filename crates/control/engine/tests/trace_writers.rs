//! Real database proofs of committed approval, Stop, cancellation, provisioning and frozen agent responsibility.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod shared;

use common::{World, command_asked, queued};
use orch_agent_contract::events::OrchTool;
use orch_api_contract::approvals::{ApprovalDecision, HarnessOptionDecision};
use orch_authz::Caller;
use orch_db::audit::{Origin, scope_origin};
use orch_link_contract::commands::{
    CommandError, CommandKind as K, ErrorCode, OrchTool as WireTool,
};
use orch_types::{Id, new_id};
use serde_json::json;

#[tokio::test]
async fn an_actual_agent_document_create_and_edit_keep_the_document_place_original_person_and_one_replayed_source_act()
 {
    let world = shared::a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Document audit trace").await;
    let topic = world.topic("Agent documents").await;
    for permission in ["doc.view", "doc.edit"] {
        shared::share(&world, "node", topic, "live", permission, "any", thread).await;
    }
    let (turn, _) = world
        .running_turn(thread, "Make and change a shared document")
        .await;
    shared::the_agent_takes_tools_again(&world).await;
    let listing_call = new_id().to_string();
    world
        .say(
            thread,
            orch_link_contract::events::Event::OrchToolCall(
                orch_link_contract::events::OrchToolCall {
                    turn_id: turn.turn_id,
                    call_ref: listing_call.clone(),
                    attempt: None,
                    tool: WireTool::ListShared,
                    arguments: json!({"path":format!("id:{topic}")}),
                },
            ),
        )
        .await;
    let listing = shared::ok(shared::answer_to(&world, thread, &listing_call).await);
    let docs = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "docs")
        .unwrap();
    let path = format!("{}Agent evidence.md", docs["path"].as_str().unwrap());
    let call = new_id().to_string();
    let event =
        orch_link_contract::events::Event::OrchToolCall(orch_link_contract::events::OrchToolCall {
            turn_id: turn.turn_id,
            call_ref: call.clone(),
            attempt: None,
            tool: WireTool::EditDocument,
            arguments: json!({"command":"create","path":path,"markdown":"private document body"}),
        });
    world.say(thread, event.clone()).await;
    let result = shared::ok(shared::answer_to(&world, thread, &call).await);
    let document: Id = serde_json::from_value(result["id"].clone()).unwrap();
    world.say(thread, event).await;
    let call = new_id().to_string();
    world.say(thread,orch_link_contract::events::Event::OrchToolCall(orch_link_contract::events::OrchToolCall {
        turn_id:turn.turn_id,call_ref:call.clone(), attempt: None,tool:WireTool::EditDocument,
        arguments:json!({"command":"rewrite","path":format!("id:{document}"),"markdown":"a second private document body","seq":result["seq"]}),
    })).await;
    shared::ok(shared::answer_to(&world, thread, &call).await);
    let rows:Vec<(String,Id,Id,Vec<Id>,Id,serde_json::Value)>=sqlx::query_as("SELECT action,node_id,org_id,scope_node_ids,for_user_id,details FROM audit_log WHERE object_id=$1 ORDER BY at,id").bind(document).fetch_all(world.pool.pool()).await.unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().any(|r| r.0 == "document.created_by_agent"));
    assert!(rows.iter().any(|r| r.0 == "document.edited_by_agent"));
    for row in rows {
        assert_eq!((row.1, row.2, row.4), (topic, world.root, world.ada));
        assert!(row.3.contains(&world.root) && row.3.contains(&topic));
        assert!(!row.5.to_string().contains("private document body"));
    }
}

async fn world() -> World {
    World::new().await
}

fn origin() -> Origin {
    Origin {
        as_admin: None,
        ip: Some("203.0.113.7".parse().unwrap()),
        user_agent: Some("trace-test".to_owned()),
        request_id: Some("trace-request".to_owned()),
    }
}

async fn count(world: &World, action: &str, object: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action=$1 AND object_id=$2")
        .bind(action)
        .bind(object)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

async fn veto(world: &World, action: &str) {
    sqlx::raw_sql("CREATE FUNCTION trace_veto() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF EXISTS(SELECT 1 FROM user_state WHERE key='help' AND value->>'trace_veto'=NEW.action) THEN RAISE EXCEPTION 'trace commit veto'; END IF; RETURN NEW; END $$; CREATE TRIGGER trace_veto BEFORE INSERT ON audit_log FOR EACH ROW EXECUTE FUNCTION trace_veto();").execute(world.pool.pool()).await.unwrap();
    sqlx::query("INSERT INTO user_state(user_id,key,value) VALUES($1,'help',jsonb_build_object('trace_veto',$2::text)) ON CONFLICT(user_id,key) DO UPDATE SET value=EXCLUDED.value")
        .bind(world.ada)
        .bind(action)
        .execute(world.pool.pool())
        .await
        .unwrap();
}
async fn allow(world: &World) {
    sqlx::raw_sql("DROP TRIGGER trace_veto ON audit_log; DROP FUNCTION trace_veto(); DELETE FROM user_state WHERE key='help' AND value ? 'trace_veto';").execute(world.pool.pool()).await.unwrap();
}

#[tokio::test]
async fn approval_decided_is_one_factual_commit_and_a_failed_audit_leaves_it_pending() {
    let world = world().await;
    let thread = world.thread("Approval trace").await;
    let (turn, _) = world.running_turn(thread, "request a decision").await;
    world
        .say(thread, command_asked(turn.turn_id, "trace-approval"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    let decision = ApprovalDecision::Option(HarnessOptionDecision {
        option_id: "allow_once".to_owned(),
        note: None,
    });
    veto(&world, "approval.decided").await;
    assert!(
        world
            .engine()
            .decide_approval(&Caller::session(world.ada), approval, &decision)
            .await
            .is_err()
    );
    let state: String = world
        .scalar("SELECT state FROM approvals WHERE id=$1", approval)
        .await;
    assert_eq!(state, "pending");
    assert_eq!(count(&world, "approval.decided", thread).await, 0);
    allow(&world).await;
    scope_origin(
        origin(),
        world
            .engine()
            .decide_approval(&Caller::session(world.ada), approval, &decision),
    )
    .await
    .unwrap();
    assert!(
        world
            .engine()
            .decide_approval(&Caller::session(world.ada), approval, &decision)
            .await
            .is_err()
    );
    let rows:Vec<(Id,Vec<Id>,Option<String>,String)>=sqlx::query_as("SELECT actor_id,scope_node_ids,request_id,details::text FROM audit_log WHERE action='approval.decided' AND object_id=$1")
        .bind(thread).fetch_all(world.pool.pool()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, world.ada);
    assert!(rows[0].1.contains(&world.root));
    assert_eq!(rows[0].2.as_deref(), Some("trace-request"));
    assert!(!rows[0].3.contains("request a decision"));
}

#[tokio::test]
async fn turn_interrupted_records_only_the_first_accepted_stop_and_rolls_back_with_its_audit() {
    let world = world().await;
    let thread = world.thread("Stop trace").await;
    let (turn, _) = world.running_turn(thread, "an active turn").await;
    veto(&world, "turn.interrupted").await;
    assert!(
        world
            .engine()
            .interrupt(&Caller::session(world.ada), thread, turn.turn_id)
            .await
            .is_err()
    );
    let stopped: Option<Id> = world
        .scalar("SELECT interrupted_by FROM turns WHERE id=$1", turn.turn_id)
        .await;
    assert_eq!(stopped, None);
    assert_eq!(count(&world, "turn.interrupted", thread).await, 0);
    allow(&world).await;
    scope_origin(
        origin(),
        world
            .engine()
            .interrupt(&Caller::session(world.ada), thread, turn.turn_id),
    )
    .await
    .unwrap();
    let _ = world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, turn.turn_id)
        .await;
    assert_eq!(count(&world, "turn.interrupted", thread).await, 1);
    let requester: Id = world
        .scalar(
            "SELECT actor_id FROM audit_log WHERE object_id=$1 AND action='turn.interrupted'",
            thread,
        )
        .await;
    assert_eq!(requester, world.ada);
}

#[tokio::test]
async fn input_cancelled_records_another_persons_input_once_and_never_the_authors_own_cancel() {
    let world = world().await;
    let thread = world.thread("Cancellation trace").await;
    let _ = world
        .running_turn(thread, "keep the following input queued")
        .await;
    let other = world.person("bee").await;
    world.give(other, "developer").await;
    let input = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("private queued text"),
        )
        .await
        .unwrap();
    veto(&world, "input.cancelled").await;
    assert!(
        world
            .engine()
            .cancel_input(&Caller::session(other), thread, input.input.id)
            .await
            .is_err()
    );
    assert_eq!(world.input_state(input.input.id).await.0, "pending");
    allow(&world).await;
    world
        .engine()
        .cancel_input(&Caller::session(other), thread, input.input.id)
        .await
        .unwrap();
    world
        .engine()
        .cancel_input(&Caller::session(other), thread, input.input.id)
        .await
        .unwrap();
    let own = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("another private text"),
        )
        .await
        .unwrap();
    world
        .engine()
        .cancel_input(&Caller::session(world.ada), thread, own.input.id)
        .await
        .unwrap();
    assert_eq!(count(&world, "input.cancelled", thread).await, 1);
    let (actor,details):(Id,String)=sqlx::query_as("SELECT actor_id,details::text FROM audit_log WHERE action='input.cancelled' AND object_id=$1").bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(actor, other);
    assert!(!details.contains("private queued text"));
}

#[tokio::test]
async fn verified_provisioning_and_start_are_recorded_once_and_failed_creation_claims_neither() {
    let world = world().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "trace-host", 16_384).await;
    let template = hosting::template(&world).await;
    let made = scope_origin(origin(), hosting::make(&world, template, "Trace files")).await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let _unit = hosting::enroll(&world, runtime).await;
    let (state, _, error) = hosting::wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(count(&world, "environment.provisioned", made.id).await, 1);
    assert_eq!(count(&world, "environment.started", made.id).await, 1);
    world
        .engine()
        .start_environment(made.id, Some(world.ada))
        .await
        .unwrap();
    assert_eq!(count(&world, "environment.started", made.id).await, 1);
    host.refuse(
        K::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Internal, "no capacity for the candidate"),
    );
    let failed = hosting::make(&world, template, "Trace failed files").await;
    let (state, _, _) = hosting::wait_over(&world, failed.job.id).await;
    assert_eq!(state, "failed");
    assert_eq!(count(&world, "environment.provisioned", failed.id).await, 0);
    assert_eq!(count(&world, "environment.started", failed.id).await, 0);
    let request:Option<String>=world.scalar("SELECT request_id FROM audit_log WHERE action='environment.provisioned' AND object_id=$1",made.id).await;
    assert_eq!(request.as_deref(), Some("trace-request"));
}

#[tokio::test]
async fn an_agents_tool_keeps_the_input_author_after_a_steer_and_an_owner_change() {
    let world = world().await;
    shared::the_agent_takes_tools_again(&world).await;
    let thread = world.thread("Responsibility trace").await;
    let (turn, _) = world
        .running_turn(thread, "start for the original person")
        .await;
    let other = world.person("bee").await;
    world.give(other, "developer").await;
    let mut steer = queued("a different person's steer");
    steer.delivery = orch_api_contract::threads::Delivery::Steer;
    let _ = world
        .engine()
        .submit_input(&Caller::session(other), thread, steer)
        .await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    sqlx::query("UPDATE nodes SET owner_id=$2 WHERE id=$1")
        .bind(thread)
        .bind(other)
        .execute(&mut *tx)
        .await
        .unwrap();
    orch_db::queries::access::testing::name_owner(
        &mut tx,
        thread,
        ("user", other),
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world
        .engine()
        .answer_tool_call(&orch_engine::tools::CallContext {
            machine: world.machine_id,
            thread,
            turn: turn.turn_id,
            call_ref: new_id().to_string(),
            attempt: None,
            tool: OrchTool::SearchShared,
            arguments: json!({"query":"trace needle"}),
        })
        .await
        .unwrap();
    let responsible:Option<Id>=world.scalar("SELECT for_user_id FROM audit_log WHERE action='share.searched_by_agent' AND actor_id=$1",thread).await;
    assert_eq!(responsible, Some(world.ada));
    let frozen: Option<Id> = world
        .scalar(
            "SELECT audit_for_user_id FROM turns WHERE id=$1",
            turn.turn_id,
        )
        .await;
    assert_eq!(frozen, Some(world.ada));
}

#[tokio::test]
async fn automatic_approval_is_one_committed_decision_even_if_its_native_request_is_replayed() {
    let world = world().await;
    let thread = world.thread("Automatic trace").await;
    world
        .set(
            thread,
            orch_settings_contract::registry::keys::APPROVAL_MODE,
            json!("on_request"),
        )
        .await;
    let descriptor: serde_json::Value = world
        .scalar("SELECT descriptor FROM agents WHERE id=$1", world.agent)
        .await;
    let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor).unwrap();
    assert!(
        descriptor
            .approval_modes
            .contains(&orch_agent_contract::descriptor::ApprovalMode::OnRequest)
    );
    assert!(
        !descriptor
            .approval_modes
            .contains(&orch_agent_contract::descriptor::ApprovalMode::Auto),
        "this audit proof uses an explicit thread rule, not an unsupported native reviewer"
    );
    let (turn, _) = world
        .running_turn(thread, "run under the selected thread rule")
        .await;
    let request = |reference: &str| {
        orch_link_contract::events::Event::ApprovalRequested(
        orch_link_contract::events::ApprovalRequested {
            turn_id: turn.turn_id, request_ref: reference.to_owned(), title: "Run the tests".to_owned(),
            request: serde_json::from_value(json!({
                "request_kind":"command",
                "detail":{"command":"cargo test -p web","cwd":"/workspace/web","reason":null},
                "options":[
                    {"id":"allow_once","label":"Allow once","style":"allow","scope":"once","rule":null},
                    {"id":"allow_thread","label":"Allow for this thread","style":"allow","scope":"thread",
                     "rule":{"type":"command_prefix","argv":["cargo","test"],"cwd":"/workspace/web"}},
                    {"id":"deny","label":"Deny","style":"deny","scope":null,"rule":null}
                ]
            })).unwrap(),
        })
    };
    // Only a person-approved matcher authorizes a later automatic decision. Approval mode alone never does.
    world.say(thread, request("automatic-rule-seed")).await;
    world.wait_state(thread, "waiting_approval").await;
    let seed: Id = sqlx::query_scalar(
        "SELECT id FROM approvals WHERE thread_id=$1 AND native_ref='automatic-rule-seed'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    world
        .engine()
        .decide_approval(
            &Caller::session(world.ada),
            seed,
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "allow_thread".to_owned(),
                note: None,
            }),
        )
        .await
        .unwrap();
    let rule: Id = world
        .scalar(
            "SELECT id FROM thread_approval_rules WHERE thread_id=$1 AND revoked_at IS NULL",
            thread,
        )
        .await;
    world.wait_ran(K::ApprovalResolve, 1).await;
    world.wait_state(thread, "running").await;
    world.say(thread, request("automatic-trace")).await;
    world.wait_ran(K::ApprovalResolve, 2).await;
    let stream = orch_link_contract::streams::Stream::Session {
        thread_id: thread,
        segment_id: world.segment_of(thread).await,
    };
    let before = world.commands.acknowledged(world.machine_id, &stream);
    world.say(thread, request("automatic-trace")).await;
    common::eventually(
        "the exact replay is committed before audit and native-command counts are checked",
        || {
            let committed = world.commands.acknowledged(world.machine_id, &stream) > before;
            async move { committed }
        },
    )
    .await;
    assert_eq!(
        world.seen.of_kind(K::ApprovalResolve).len(),
        2,
        "one human seed and one rule decision are delivered; replay creates neither another decision nor delivery"
    );
    assert_eq!(
        count(&world, "approval.decided", thread).await,
        2,
        "the human seed and the automatic rule each commit exactly once"
    );
    let (rows,actors,scoped):(i64,bool,bool)=sqlx::query_as("SELECT count(*),bool_and(actor_kind='system'),bool_and($2=ANY(scope_node_ids)) FROM audit_log WHERE action='approval.decided' AND object_id=$1 AND actor_kind='system'").bind(thread).bind(world.root).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!((rows, actors, scoped), (1, true, true));
    let text: String = world
        .scalar(
            "SELECT details::text FROM audit_log WHERE action='approval.decided' AND object_id=$1 AND actor_kind='system'",
            thread,
        )
        .await;
    assert!(!text.contains("run under"));
    let details: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(details["via"], "rule");
    assert_eq!(details["rule"], json!(rule));
    assert_eq!(details["decision"], "approved");
    let automatic: Id = sqlx::query_scalar("SELECT id FROM approvals WHERE thread_id=$1 AND native_ref='automatic-trace' AND state='approved' AND decided_by IS NULL")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(details["approval"], json!(automatic));
}

#[tokio::test]
async fn service_and_authorless_turns_freeze_audit_responsibility_without_attributing_authorless_usage()
 {
    use orch_db::queries::threads::{self, NewInput, Place};
    use orch_link_contract::events::{Event, TurnOutcome, UsageUpdated};
    let world = world().await;
    shared::the_agent_takes_tools_again(&world).await;
    let service = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::identity::create_service_account(
        &mut conn,
        service,
        world.root,
        "trace_service",
        "Trace service",
        world.ada,
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    world.give(service, "developer").await;
    for (name, author, responsible) in [
        ("Service trace", Some(service), service),
        ("Authorless trace", None, world.ada),
    ] {
        let thread = world.thread(name).await;
        let input = new_id();
        let content =
            json!([{"type":"text","text":"trace input whose content must stay out of audit"}]);
        let authority = author.map(|_| {
            json!({"credentials":[{"type":"token","token_id":null,"scopes":["*"],"node_id":null}]})
        });
        let mut tx = world.pool.begin().await.unwrap();
        orch_db::queries::transaction_guard::lock_authority(&mut tx)
            .await
            .unwrap();
        threads::lock(&mut tx, thread).await.unwrap();
        threads::insert_input(
            &mut tx,
            &NewInput {
                plan: false,
                id: input,
                thread_id: thread,
                author_id: author,
                submission_authority: authority.as_ref(),
                run_as: None,
                run_as_authority: None,
                source: if author.is_some() { "user" } else { "system" },
                content: &content,
                delivery: "queue",
                state: "pending",
                turn_id: None,
                client_id: None,
                limit_bypass: None,
                place: Place::End,
                at: world.clock.now(),
            },
            1,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        world.engine().wake(thread);
        let turn = world.turn_start(thread, 0).await;
        world.say(thread, common::turn_began(turn.turn_id)).await;
        shared::the_agent_takes_tools_again(&world).await;
        world
            .engine()
            .answer_tool_call(&orch_engine::tools::CallContext {
                machine: world.machine_id,
                thread,
                turn: turn.turn_id,
                call_ref: new_id().to_string(),
                attempt: None,
                tool: OrchTool::SearchShared,
                arguments: json!({"query":"trace"}),
            })
            .await
            .unwrap();
        let frozen: Option<Id> = world
            .scalar(
                "SELECT audit_for_user_id FROM turns WHERE id=$1",
                turn.turn_id,
            )
            .await;
        assert_eq!(frozen, Some(responsible));
        let filter = orch_db::queries::trace::Filter {
            actor: Some(responsible),
            include_agents: true,
            action: Some("share.searched_by_agent".to_owned()),
            node: Some(world.root),
            ..Default::default()
        };
        let mut conn = world.pool.acquire().await.unwrap();
        let rows = orch_db::queries::trace::page(&mut conn, &filter, None, 10, true)
            .await
            .unwrap();
        assert!(
            rows.iter()
                .any(|r| r.base.actor_id == Some(thread) && r.for_user_id == Some(responsible))
        );
        let direct = orch_db::queries::trace::page(
            &mut conn,
            &orch_db::queries::trace::Filter {
                include_agents: false,
                ..filter
            },
            None,
            10,
            true,
        )
        .await
        .unwrap();
        assert!(direct.is_empty());
        drop(conn);
        let usage = json!({"input_tokens":12,"cached_input_tokens":0,"output_tokens":3,"reasoning_tokens":0,"total_tokens":15});
        world
            .say(
                thread,
                Event::UsageUpdated(UsageUpdated {
                    turn_id: turn.turn_id,
                    turn: serde_json::from_value(usage.clone()).unwrap(),
                    thread: serde_json::from_value(usage).unwrap(),
                    context_window: None,
                    context_used: None,
                    report: None,
                }),
            )
            .await;
        world
            .say(
                thread,
                common::turn_ended(turn.turn_id, TurnOutcome::Completed),
            )
            .await;
        let ledger = world.wait_ledger(turn.turn_id, 1).await;
        assert_eq!(ledger[0].user_id, author);
        let mut tx = world.pool.begin().await.unwrap();
        orch_db::queries::transaction_guard::lock_authority(&mut tx)
            .await
            .unwrap();
        orch_db::queries::hierarchy::delete_subtree(&mut tx, thread, world.ada, world.clock.now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let retained:Option<Id>=world.scalar("SELECT for_user_id FROM audit_log WHERE action='share.searched_by_agent' AND actor_id=$1",thread).await;
        assert_eq!(retained, Some(responsible));
    }
}
