//! Native sides keep parent/history authority, original receipts and exact deferred source commands.
use super::*;
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};
#[derive(Debug, PartialEq, serde::Deserialize)]
struct SideReceipt {
    projection: String,
    parent_id: Id,
    thread_id: Id,
    input_id: Id,
    seq: orch_types::Seq,
}
const OP: &str = "ThreadSideCreate";
fn grants(thread: Id) -> Vec<Json> {
    vec![
        rule(OP, "thread", thread, "self", "read"),
        rule(OP, "thread", thread, "self", "write"),
    ]
}
async fn start(w: &Forks, parent: Id, secret: &str, client: &str) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ThreadSideCreate, &[&parent])
        .token(secret)
        .body(&ThreadSideCreateBody {
            content: vec![ContentPart::Text {
                text: "A scoped aside".into(),
            }],
            client_id: client.into(),
            to_turn: None,
        })
        .send_bytes()
        .await
}
fn receipt(reply: &RawReply) -> Proof<SideReceipt> {
    assert_eq!(
        reply.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw)?;
    assert_eq!(value["projection"], "receipt");
    assert!(value.get("thread").is_none() && value.get("input").is_none());
    Ok(serde_json::from_value(value)?)
}
#[tokio::test(flavor = "multi_thread")]
async fn side_create_binds_its_actual_parent_and_preserves_native_history_mode_receipt_and_replay()
-> Proof {
    let w = Forks::new().await;
    let parent = w.thread_with_history("Scoped parent").await;
    w.s.rig.world.app.app.engine.stop();
    let world = &w.s.rig.world;
    let other = w.s.thread_by(&w.s.ada, "Outside side scope").await.base.id;
    for rules in [
        vec![rule(OP, "thread", parent, "self", "read")],
        vec![rule(OP, "thread", parent, "self", "write")],
        grants(other),
        vec![
            rule(
                "ThreadPromptVersionCreate",
                "thread",
                parent,
                "self",
                "read",
            ),
            rule(
                "ThreadPromptVersionCreate",
                "thread",
                parent,
                "self",
                "write",
            ),
        ],
    ] {
        let token = key(world, w.s.ada.id(), rules).await?;
        let denied = start(&w, parent, &token.secret, "refused-side").await;
        assert_eq!(denied.status.as_u16(), 403);
    }
    assert_eq!(
        w.count("SELECT count(*) FROM threads WHERE side_of IS NOT NULL")
            .await,
        0
    );
    let token = key(world, w.s.ada.id(), grants(parent)).await?;
    let made = receipt(&start(&w, parent, &token.secret, "scoped-side").await)?;
    assert_eq!(made.parent_id, parent);
    let replay = receipt(&start(&w, parent, &token.secret, "scoped-side").await)?;
    assert_eq!(made, replay);
    let child: (Option<Id>, Option<i32>, Option<String>) =
        sqlx::query_as("SELECT side_of,side_from_turn,side_reading FROM threads WHERE node_id=$1")
            .bind(made.thread_id)
            .fetch_one(&world.db.pool)
            .await?;
    assert_eq!(child.0, Some(parent));
    assert_eq!(child.1, Some(1));
    assert_eq!(child.2.as_deref(), Some("read_only"));
    let row = orch_db::queries::threads::input(&mut *world.conn().await, made.input_id)
        .await?
        .ok_or("side input missing")?;
    assert_eq!(row.thread_id, made.thread_id);
    assert_eq!(
        row.content,
        json!([{"type":"text", "text":"A scoped aside"}])
    );
    let cut = orch_db::queries::thread_history_cuts::cut(&mut *world.conn().await, made.thread_id)
        .await?
        .ok_or("native side cut missing")?;
    assert_eq!(
        (cut.thread_id, cut.owner_thread_id, cut.last_turn),
        (parent, Some(made.thread_id), 1)
    );
    assert!(cut.artifacts["side_authorization"]["operation_origin"].is_object());
    assert_eq!(
        w.count("SELECT count(*) FROM threads WHERE side_of IS NOT NULL")
            .await,
        1
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn side_create_rechecks_after_its_actual_parent_lock_before_the_first_native_insert() -> Proof
{
    for expired in [false, true] {
        let w = Forks::new().await;
        let parent = w.thread_with_history("Waiting side parent").await;
        w.s.rig.world.app.app.engine.stop();
        let world = &w.s.rig.world;
        let token = key(world, w.s.ada.id(), grants(parent)).await?;
        let mut locked = world.db.pool.begin().await?;
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(parent)
            .fetch_one(&mut *locked)
            .await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = world.app.clone();
        let secret = token.secret.clone();
        let pending = tokio::spawn(async move {
            let request =
                app.call(ThreadSideCreate, &[&parent])
                    .token(&secret)
                    .body(&ThreadSideCreateBody {
                        content: vec![ContentPart::Text {
                            text: "After parent wait".into(),
                        }],
                        client_id: "side-wait".into(),
                        to_turn: None,
                    });
            request.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(!pending.is_finished());
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(blocker).fetch_one(&world.db.pool).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        if expired {
            world.advance(chrono::Duration::hours(2));
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if expired {
            assert_eq!(reply.status.as_u16(), 401);
            assert_eq!(
                w.count("SELECT count(*) FROM threads WHERE side_of IS NOT NULL")
                    .await,
                0
            );
        } else {
            receipt(&reply)?;
        }
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn side_history_native_record_retains_the_accepted_operation_through_queue_and_delivery()
-> Proof {
    let w = Forks::new().await;
    let parent = w.thread_with_history("Deferred side source").await;
    let world = &w.s.rig.world;
    let token = key(world, w.s.ada.id(), grants(parent)).await?;
    w.s.rig.machine.handler.hold(CommandKind::AgentReadSession);
    let made = receipt(&start(&w, parent, &token.secret, "native-side-origin").await)?;
    let id = orch_engine::ids::derived("side.native-record", &[made.thread_id.as_bytes()]);
    tokio::time::timeout(rig::PATIENCE, w.s.rig.machine.handler.started(id)).await?;
    let authority =
        w.s.rig
            .commands
            .issued_authority(id)
            .ok_or("actual side source command lacks retained operation")?;
    assert_eq!(serde_json::to_value(&authority)?["kind"], "side_history");
    let mut conn = world.db.pool.begin().await?;
    let row = orch_db::queries::link::command(&mut conn, id)
        .await?
        .ok_or("actual native source command missing")?;
    let clock = world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    assert!(
        authority
            .current_callers(&mut conn, &row, clock.as_ref(), &access)
            .await?
            .is_some()
    );
    conn.rollback().await?;
    narrow(
        world,
        &token,
        vec![rule(OP, "thread", parent, "self", "read")],
    )
    .await?;
    let mut conn = world.db.pool.begin().await?;
    assert!(
        authority
            .current_callers(&mut conn, &row, clock.as_ref(), &access)
            .await?
            .is_none()
    );
    let stored = orch_db::queries::link::command_with_authority(&mut conn, id)
        .await?
        .ok_or("retained side command missing")?;
    assert_eq!(stored.authority, Some(serde_json::to_value(&authority)?));
    conn.rollback().await?;
    w.s.rig.machine.handler.release();
    Ok(())
}
