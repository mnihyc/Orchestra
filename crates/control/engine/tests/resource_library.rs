//! Reported resource use survives replay, distinguishes versions and retains captured people and UTC windows.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;

#[tokio::test]
async fn old_and_recovered_history_never_borrow_the_open_turn_to_add_a_use() {
    use orch_link_contract::events::{EmittedItemKind, Event, TurnOutcome};
    fn began(turn: orch_types::Id, native: &str, payload: &orch_types::Json) -> Event {
        let Event::ItemStarted(mut item) = common::message_began(turn, native) else {
            panic!("the fixture starts an item")
        };
        item.kind = EmittedItemKind::ToolCall;
        item.payload = serde_json::from_value(payload.clone()).unwrap();
        Event::ItemStarted(item)
    }
    fn done(native: &str, payload: &orch_types::Json) -> Event {
        let Event::ItemCompleted(mut item) = common::message_done(native, "") else {
            panic!("the fixture completes an item")
        };
        item.payload = serde_json::from_value(payload.clone()).unwrap();
        Event::ItemCompleted(item)
    }
    let world = World::new().await;
    let thread = world.thread("History does not execute again").await;
    let server = new_id();
    let (old, _) = world.running_turn(thread, "first").await;
    let payload = serde_json::json!({"title":"lookup","tool":{"kind":"mcp","server":"tools","tool":"lookup","arguments":{},"result":{"ok":true},"error":null},"started_at":Time::new(world.clock.now()),"duration_ms":null,"raw":{"name":"mcp__tools__lookup","input":orch_api_contract::threads::OutputRef::of("{}"),"output":orch_api_contract::threads::OutputRef::of("{\"ok\":true}"),"is_error":false}});
    sqlx::query("UPDATE turns SET resource_delivery=$2 WHERE id=$1")
        .bind(old.turn_id)
        .bind(serde_json::json!([{"name":"tools","server_id":server}]))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .say(thread, began(old.turn_id, "original-call", &payload))
        .await;
    world.say(thread, done("original-call", &payload)).await;
    common::eventually("the reported original call",||async{world.count("SELECT COALESCE(max(uses),0) FROM resource_use_rollups WHERE subject_id=$1 AND day IS NULL",server).await==1}).await;
    world
        .say(
            thread,
            common::turn_ended(old.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_turn(old.turn_id, "completed").await;
    sqlx::query("UPDATE turns SET created_at=created_at-interval '120 days',started_at=started_at-interval '120 days',completed_at=completed_at-interval '120 days' WHERE id=$1").bind(old.turn_id).execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE items SET created_at=created_at-interval '120 days',updated_at=updated_at-interval '120 days' WHERE turn_id=$1").bind(old.turn_id).execute(world.pool.pool()).await.unwrap();
    let mut history = payload.clone();
    history["started_at"] =
        serde_json::to_value(Time::new(world.clock.now() - chrono::Duration::days(120))).unwrap();
    let (current, _) = world.running_turn(thread, "new work").await;
    sqlx::query("UPDATE turns SET resource_delivery=$2 WHERE id=$1")
        .bind(current.turn_id)
        .bind(serde_json::json!([{"name":"tools","server_id":server}]))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .say(thread, began(old.turn_id, "old-identified-call", &history))
        .await;
    world
        .say(thread, done("old-identified-call", &history))
        .await;
    // Adapter history has a stable synthetic turn identity, never the active turn it is replayed beside.
    let unowned = began(new_id(), "old-unmapped-history", &history);
    let segment = world.segment_of(thread).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let owned = common::message_began(current.turn_id, "owned-source-check");
    assert!(
        orch_db::queries::machine_reports::accepts(
            &mut conn,
            world.machine_id,
            &orch_link_contract::streams::Stream::Session {
                thread_id: thread,
                segment_id: segment
            },
            &[(owned.kind().as_str(), &owned.body().unwrap())]
        )
        .await
        .unwrap(),
        "the real open turn retains its own source authority"
    );
    let accepted = orch_db::queries::machine_reports::accepts(
        &mut conn,
        world.machine_id,
        &orch_link_contract::streams::Stream::Session {
            thread_id: thread,
            segment_id: segment,
        },
        &[(unowned.kind().as_str(), &unowned.body().unwrap())],
    )
    .await
    .unwrap();
    assert!(
        !accepted,
        "synthetic history cannot borrow the currently open turn's source authority"
    );
    drop(conn);
    world
        .say(thread, done("old-unmapped-history", &history))
        .await;
    world
        .say(thread, done("old-completion-without-start", &history))
        .await;
    world
        .say(thread, done("old-completion-without-start", &history))
        .await;
    world
        .say(
            thread,
            common::message_began(current.turn_id, "replay-barrier"),
        )
        .await;
    world
        .say(
            thread,
            common::message_done("replay-barrier", "History applied"),
        )
        .await;
    common::eventually("the ordered history barrier",||async{world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='replay-barrier' AND status='completed'",thread).await==1}).await;
    assert_eq!(world.count("SELECT COALESCE(max(uses),0) FROM resource_use_rollups WHERE subject_id=$1 AND day IS NULL",server).await,1);
    world
        .say(thread, began(current.turn_id, "new-call", &payload))
        .await;
    world.say(thread, done("new-call", &payload)).await;
    common::eventually("the new real call",||async{world.count("SELECT COALESCE(max(uses),0) FROM resource_use_rollups WHERE subject_id=$1 AND day IS NULL",server).await==2}).await;
    let mut denied = payload.clone();
    denied["tool"]["result"] = serde_json::Value::Null;
    denied["tool"]["error"] = serde_json::json!("Denied by policy");
    world
        .say(thread, began(current.turn_id, "denied-tool", &denied))
        .await;
    common::eventually("the linked tool item", || async {
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='denied-tool'",
                thread,
            )
            .await
            == 1
    })
    .await;
    let item: orch_types::Id =
        sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND native_ref='denied-tool'")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    sqlx::query("INSERT INTO approvals(id,kind,state,thread_id,turn_id,tool_item_id,native_ref,title,request,required_permission,decided_at) VALUES($1,'harness','denied',$2,$3,$4,'different-permission-request','Denied by policy','{}','thread.interact',$5)").bind(new_id()).bind(thread).bind(current.turn_id).bind(item).bind(world.clock.now()).execute(world.pool.pool()).await.unwrap();
    world.say(thread, done("denied-tool", &denied)).await;
    world
        .say(
            thread,
            common::message_began(current.turn_id, "denial-barrier"),
        )
        .await;
    world
        .say(
            thread,
            common::message_done("denial-barrier", "Denied call applied"),
        )
        .await;
    common::eventually("the ordered denial barrier",||async{world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='denial-barrier' AND status='completed'",thread).await==1}).await;
    assert_eq!(world.count("SELECT COALESCE(max(uses),0) FROM resource_use_rollups WHERE subject_id=$1 AND day IS NULL",server).await,2,"a linked local denial never reaches the use count");
}

use common::World;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::{ResourceUseKind, UseCoverage};
use orch_engine::resource_facts::{
    UseRequest,
    library::{RecordedUses, Report, record},
    reported_uses,
};
use orch_types::{Time, new_id};

#[tokio::test]
async fn replay_does_not_add_another_use_and_versions_share_exactly_one_resource_total() {
    let world = World::new().await;
    let thread = world.thread("Count only real open execution").await;
    let (started, _) = world.running_turn(thread, "work").await;
    let object = ObjectRef {
        kind: ObjectKind::Skill,
        id: new_id(),
    };
    let first = new_id();
    let second = new_id();
    let at = Time::new(world.clock.now());
    let old = Time::new(at.get() - chrono::Duration::days(30));
    let mut conn = world.pool.acquire().await.unwrap();
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    for (version, source, person, when) in [
        (first, "old", Some(world.ada), old),
        (first, "first", Some(world.ada), at),
        (first, "first", Some(world.ada), at),
        (second, "second", Some(world.ada), at),
    ] {
        record(
            &mut tx,
            &Report {
                turn: Some(started.turn_id),
                object,
                version: Some(version),
                kind: ResourceUseKind::SkillInvocation,
                source,
                person,
                at: when,
            },
        )
        .await
        .unwrap();
    }
    let requested = [None, Some(first), Some(second)].map(|version_id| UseRequest {
        object,
        version_id,
        kind: ResourceUseKind::SkillInvocation,
    });
    let counts = reported_uses(&mut tx, &requested, at, Some(&RecordedUses))
        .await
        .unwrap();
    assert_eq!(counts[0].1.all_time, Some(3));
    assert_eq!(counts[0].1.last_30_days, Some(2));
    assert_eq!(counts[0].1.people, Some(1));
    assert_eq!(counts[0].1.people_30_days, Some(1));
    assert_eq!(counts[0].1.last_used_at, Some(at));
    assert!(matches!(counts[0].1.coverage, UseCoverage::Partial { .. }));
    assert_eq!(counts[1].1.all_time, Some(2));
    assert_eq!(counts[2].1.all_time, Some(1));
    let ordered = orch_engine::resource_facts::library::lifetimes(&mut tx, &[object], at)
        .await
        .unwrap();
    assert_eq!(
        ordered,
        vec![(object, 3)],
        "inventory ordering reads the same aggregate without version duplication"
    );
    tx.rollback().await.unwrap();
    let rolled_back = reported_uses(&mut conn, &requested, at, Some(&RecordedUses))
        .await
        .unwrap();
    assert_eq!(
        rolled_back[0].1.all_time, None,
        "a rolled back delivery leaves no reported history"
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM resource_use_rollups WHERE subject_id=$1")
            .bind(object.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(rows, 0, "a rollback leaves no stored counter");
}

#[tokio::test]
async fn unknown_actor_is_not_claimed_as_zero_people_or_the_current_owner() {
    let world = World::new().await;
    let thread = world.thread("Count only real open execution").await;
    let (started, _) = world.running_turn(thread, "work").await;
    let at = Time::new(world.clock.now());
    let object = ObjectRef {
        kind: ObjectKind::McpServer,
        id: new_id(),
    };
    let mut conn = world.pool.acquire().await.unwrap();
    record(
        &mut conn,
        &Report {
            turn: Some(started.turn_id),
            object,
            version: None,
            kind: ResourceUseKind::McpCall,
            source: "native:call",
            person: None,
            at,
        },
    )
    .await
    .unwrap();
    let request = UseRequest {
        object,
        version_id: None,
        kind: ResourceUseKind::McpCall,
    };
    let counted = reported_uses(&mut conn, &[request], at, Some(&RecordedUses))
        .await
        .unwrap()
        .remove(0)
        .1;
    assert_eq!(counted.all_time, Some(1));
    assert_eq!(counted.people, None);
    assert_eq!(counted.people_30_days, None);
}

#[tokio::test]
async fn recreating_the_same_environment_refreshes_last_use_without_claiming_another_environment() {
    let world = World::new().await;
    let mut conn = world.pool.acquire().await.unwrap();
    let template = new_id();
    let host = new_id();
    let environment = new_id();
    let at = Time::new(world.clock.now());
    for when in [Time::new(at.get() - chrono::Duration::days(1)), at] {
        orch_engine::resource_facts::library::created(
            &mut conn,
            template,
            Some(host),
            environment,
            false,
            Some(world.ada),
            when,
        )
        .await
        .unwrap();
    }
    let requests =
        [(ObjectKind::Template, template), (ObjectKind::Host, host)].map(|(kind, id)| UseRequest {
            object: ObjectRef { kind, id },
            version_id: None,
            kind: ResourceUseKind::EnvironmentMade,
        });
    for (_, count) in reported_uses(&mut conn, &requests, at, Some(&RecordedUses))
        .await
        .unwrap()
    {
        assert_eq!(count.all_time, Some(1));
        assert_eq!(count.last_used_at, Some(at));
    }
}

#[tokio::test]
async fn late_completions_count_once_while_open_and_months_old_replay_cannot_recreate_pruned_receipts()
 {
    let world = World::new().await;
    let thread = world.thread("Late reported calls").await;
    let (turn, _) = world.running_turn(thread, "work").await;
    let object = ObjectRef {
        kind: ObjectKind::McpServer,
        id: new_id(),
    };
    let at = Time::new(world.clock.now());
    let mut conn = world.pool.acquire().await.unwrap();
    for _ in 0..2 {
        record(
            &mut conn,
            &Report {
                turn: Some(turn.turn_id),
                object,
                version: None,
                kind: ResourceUseKind::McpCall,
                source: "late-call",
                person: Some(world.ada),
                at,
            },
        )
        .await
        .unwrap();
    }
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM resource_use_receipts WHERE turn_id=$1")
            .bind(turn.turn_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        receipts, 1,
        "repeated native completion while open has one receipt"
    );
    drop(conn);
    world
        .say(
            thread,
            common::turn_ended(
                turn.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_turn(turn.turn_id, "completed").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM resource_use_receipts WHERE turn_id=$1")
            .bind(turn.turn_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        receipts, 0,
        "turn closure removes its replay fence atomically"
    );
    let months_later = Time::new(at.get() + chrono::Duration::days(120));
    for source in ["late-call", "history-imported-with-a-new-native-id"] {
        record(
            &mut conn,
            &Report {
                turn: Some(turn.turn_id),
                object,
                version: None,
                kind: ResourceUseKind::McpCall,
                source,
                person: Some(world.ada),
                at: months_later,
            },
        )
        .await
        .unwrap();
    }
    let request = UseRequest {
        object,
        version_id: None,
        kind: ResourceUseKind::McpCall,
    };
    let count = reported_uses(&mut conn, &[request], months_later, Some(&RecordedUses))
        .await
        .unwrap()
        .remove(0)
        .1;
    assert_eq!(count.all_time, Some(1));
    assert_eq!(count.last_30_days, Some(0));
    assert_eq!(count.last_used_at, Some(at));
}

#[tokio::test]
async fn a_tool_completion_uses_the_servers_frozen_identity_after_a_session_reload() {
    let world = World::new().await;
    let thread = world.thread("Frozen tool attribution").await;
    let (started, _) = world.running_turn(thread, "work").await;
    let segment = world.segment_of(thread).await;
    let before = new_id();
    let after = new_id();
    let command = new_id();
    let later = new_id();
    let at = Time::new(world.clock.now());
    let mut conn = world.pool.acquire().await.unwrap();
    for (id, server) in [(command, before), (later, after)] {
        sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,deadline_at) VALUES($1,$2,'session.open',$3,'succeeded',$4)").bind(id).bind(world.machine_id).bind(serde_json::json!({"mcp":[{"name":"tools","server_id":server}]})).bind(at.get()).execute(&mut *conn).await.unwrap();
    }
    sqlx::query("UPDATE turns SET resource_delivery=NULL WHERE id=$1")
        .bind(started.turn_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE segments SET delivered_resources=jsonb_build_object('command',$2::uuid) WHERE id=$1").bind(segment).bind(command).execute(&mut *conn).await.unwrap();
    orch_engine::resource_facts::library::capture_turn(&mut conn, started.turn_id)
        .await
        .unwrap();
    sqlx::query("UPDATE segments SET delivered_resources=jsonb_build_object('command',$2::uuid) WHERE id=$1").bind(segment).bind(later).execute(&mut *conn).await.unwrap();
    sqlx::query("DELETE FROM commands WHERE id=$1")
        .bind(command)
        .execute(&mut *conn)
        .await
        .unwrap();
    let payload = serde_json::json!({"tool":{"kind":"mcp","server":"tools","tool":"lookup","arguments":{},"result":{"ok":true},"error":null}});
    for _ in 0..2 {
        orch_engine::resource_facts::library::tool_item(
            &mut conn,
            segment,
            Some(started.turn_id),
            "native-call",
            &payload,
            at,
        )
        .await
        .unwrap();
    }
    let requests = [before, after].map(|id| UseRequest {
        object: ObjectRef {
            kind: ObjectKind::McpServer,
            id,
        },
        version_id: None,
        kind: ResourceUseKind::McpCall,
    });
    let counts = reported_uses(&mut conn, &requests, at, Some(&RecordedUses))
        .await
        .unwrap();
    assert_eq!(counts[0].1.all_time, Some(1));
    assert_eq!(counts[1].1.all_time, None);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM resource_use_rollups WHERE subject_id=$1",
                after
            )
            .await,
        0
    );
}

#[tokio::test]
async fn only_an_explicit_skill_report_counts_the_frozen_version_and_unrelated_plugin_names_do_not_match()
 {
    let world = World::new().await;
    let thread = world.thread("Reported skills").await;
    let (started, _) = world.running_turn(thread, "work").await;
    let segment = world.segment_of(thread).await;
    let skill = new_id();
    let first = new_id();
    let second = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    let at = Time::new(world.clock.now());
    sqlx::query("INSERT INTO skills(id,org_id,home_id,owner_id,name) VALUES($1,$2,$2,$3,'lookup')")
        .bind(skill)
        .bind(world.root)
        .bind(world.ada)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO blobs(hash,size) VALUES($1,0) ON CONFLICT DO NOTHING")
        .bind(vec![0u8; 32])
        .execute(&mut *conn)
        .await
        .unwrap();
    for (version, number) in [(first, 1), (second, 2)] {
        sqlx::query("INSERT INTO skill_versions(id,skill_id,number,name,description,archive,archive_size,files,size,origin,created_by) VALUES($1,$2,$3,'lookup','Lookup', $4,0,'[]',0,'{\"kind\":\"draft\",\"based_on\":null}',$5)").bind(version).bind(skill).bind(number).bind(vec![0u8;32]).bind(world.ada).execute(&mut *conn).await.unwrap();
    }
    sqlx::query("UPDATE turns SET skill_versions=ARRAY[$2::uuid] WHERE id=$1")
        .bind(started.turn_id)
        .bind(first)
        .execute(&mut *conn)
        .await
        .unwrap();
    let requests = [None, Some(first), Some(second)].map(|version_id| UseRequest {
        object: ObjectRef {
            kind: ObjectKind::Skill,
            id: skill,
        },
        version_id,
        kind: ResourceUseKind::SkillInvocation,
    });
    assert_eq!(
        reported_uses(&mut conn, &requests, at, Some(&RecordedUses))
            .await
            .unwrap()[0]
            .1
            .all_time,
        None,
        "delivery alone is not a recorded invocation"
    );
    for (native, name) in [
        ("ambiguous-plain", "lookup"),
        ("external", "another-plugin:lookup"),
        ("native-skill", "orch-skills:lookup"),
        ("native-skill", "orch-skills:lookup"),
    ] {
        let payload = serde_json::json!({"tool":{"kind":"other","name":"Skill","arguments":{"skill":name},"result":"Loaded skill"}});
        orch_engine::resource_facts::library::tool_item(
            &mut conn,
            segment,
            Some(started.turn_id),
            native,
            &payload,
            at,
        )
        .await
        .unwrap();
    }
    let counts = reported_uses(&mut conn, &requests, at, Some(&RecordedUses))
        .await
        .unwrap();
    assert_eq!(counts[0].1.all_time, Some(1));
    assert_eq!(counts[1].1.all_time, Some(1));
    assert_eq!(counts[2].1.all_time, None);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM resource_use_rollups WHERE version_id=$1")
            .bind(second)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(rows, 0, "the undelivered version receives no counter");
}
