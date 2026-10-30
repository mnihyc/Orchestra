//! Thread icons agree across reader projections and clear only when that reader sees the completed turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::PageQuery;
use orch_api_contract::documents::{InlineRefKind, ReferencePreviewGet, ReferencePreviewGetQuery};
use orch_api_contract::everything::{EverythingList, EverythingListQuery, EverythingRecent};
use orch_api_contract::nodes::{
    Node, NodeChildList, NodeChildListQuery, NodeGet, OrgTreeGet, OrgTreeGetQuery,
};
use orch_api_contract::personal::{ThreadRead, ThreadReadBody};
use orch_api_contract::resources::ProviderThreadList;
use orch_api_contract::search::{
    NameSearchList, NameSearchListQuery, SearchFound, SearchList, SearchListQuery,
    SearchStreamEvent, SearchStreamList,
};
use orch_api_contract::threads::{
    ThreadDisplay, ThreadDisplayState as State, ThreadSyncGet, ThreadSyncGetQuery,
};
use orch_db::queries::source::ThreadDisplayFacts;
use orch_engine::views::thread_display;
use serde_json::{Value, json};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn a_threads_icon_gives_people_then_blockers_then_failed_work_priority_over_other_work() {
    let mut facts = ThreadDisplayFacts {
        state: "idle".into(),
        unread_completion: true,
        ..Default::default()
    };
    assert_eq!(thread_display(&facts).state, State::Finished);
    facts.queue_held = true;
    facts.held_inputs = 2;
    assert_eq!(thread_display(&facts).state, State::Held);
    facts.queued_inputs = 1;
    assert_eq!(thread_display(&facts).state, State::Queued);
    facts.state = "starting".into();
    assert_eq!(thread_display(&facts).state, State::Running);
    facts.failed = true;
    assert_eq!(thread_display(&facts).state, State::Failed);
    facts.blocked_reason = Some("provider_unavailable".into());
    assert_eq!(thread_display(&facts).state, State::Blocked);
    facts.waiting_inputs = 1;
    assert_eq!(thread_display(&facts).state, State::WaitingApproval);
    facts.waiting_inputs = 0;
    facts.approvals = 1;
    assert_eq!(thread_display(&facts).state, State::WaitingApproval);
    facts.archived = true;
    assert_eq!(thread_display(&facts).state, State::Archived);
    let quiet = ThreadDisplayFacts {
        state: "idle".into(),
        ..Default::default()
    };
    assert_eq!(thread_display(&quiet).state, State::Idle);
}

#[test]
fn an_expired_request_no_longer_draws_a_human_wait_while_its_turn_settles() {
    let mut facts = ThreadDisplayFacts {
        state: "waiting_approval".into(),
        approvals: 1,
        ..Default::default()
    };
    assert_eq!(thread_display(&facts).state, State::WaitingApproval);
    facts.approvals = 0;
    assert_eq!(thread_display(&facts).state, State::Running);
}

#[test]
fn automatic_recovery_uses_queued_running_and_failed_without_overriding_people_or_locks() {
    let mut facts = ThreadDisplayFacts {
        state: "blocked".into(),
        blocked_reason: Some("error".into()),
        blocked_detail: Some("call_recovery".into()),
        recovery_state: Some("scheduled".into()),
        recovery_attempt: 2,
        recovery_step_type: Some("retry".into()),
        recovery_step_times: Some(3),
        recovery_failure_kind: Some("provider_busy".into()),
        failed: true,
        ..Default::default()
    };
    for (recovery, expected) in [
        ("scheduled", State::Queued),
        ("running", State::Running),
        ("stopped", State::Failed),
        ("waiting", State::Failed),
    ] {
        facts.recovery_state = Some(recovery.into());
        let display = thread_display(&facts);
        assert_eq!(display.state, expected);
        assert_eq!(display.recovery.unwrap().attempts, Some(3));
    }
    facts.recovery_attempt = i64::from(u32::MAX);
    facts.recovery_step_times = Some(i64::from(u32::MAX));
    let details = thread_display(&facts).recovery.unwrap();
    assert_eq!(
        (details.attempt, details.attempts),
        (i64::from(u32::MAX), Some(i64::from(u32::MAX)))
    );
    facts.approvals = 1;
    let display = thread_display(&facts);
    assert_eq!(display.state, State::WaitingApproval);
    assert!(display.recovery.is_none());
    facts.approvals = 0;
    facts.recovery_has_lock = true;
    assert_eq!(thread_display(&facts).state, State::Blocked);
    assert!(thread_display(&facts).recovery.is_none());
    facts.recovery_has_lock = false;
    for detail in [
        "provider_switched_off",
        "model_switched_off",
        "request_cancelled",
        "returning",
    ] {
        facts.recovery_stop_reason = Some(detail.into());
        assert_eq!(thread_display(&facts).state, State::Blocked);
        assert!(thread_display(&facts).recovery.is_none());
    }
    facts.recovery_stop_reason = Some("private vendor diagnostic".into());
    facts.recovery_failure_kind = Some("private extension diagnostic".into());
    let display = thread_display(&facts);
    assert_eq!(display.state, State::Failed);
    let serialized = serde_json::to_string(&display).unwrap();
    assert!(!serialized.contains("private"));
    facts.archived = true;
    assert_eq!(thread_display(&facts).state, State::Archived);
    assert!(thread_display(&facts).recovery.is_none());
}

#[tokio::test]
async fn recovery_facts_join_each_existing_thread_read_without_vendor_text_or_candidate_parameters()
-> Proof {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    let pool = scene.rig.db.pool();
    sqlx::query("UPDATE threads SET state='blocked',blocked_reason='error',blocked_detail='call_recovery' WHERE node_id=$1")
        .bind(thread).execute(pool).await?;
    let due = scene.rig.world.now() + chrono::Duration::seconds(55);
    sqlx::query("INSERT INTO model_call_recovery(node_id,purpose,call_key,reference,failure,rule,cursor,action,state,due_at,context,updated_at) VALUES($1,'agent',$1,$2,$3,$4,'{}',$5,'scheduled',$6,$7,$8)")
        .bind(thread).bind(orch_types::new_id())
        .bind(json!({"code":"provider_busy","message":"private vendor text","said":"private vendor text","retryable":true}))
        .bind(json!({"id":"busy","steps":[{"type":"failover","candidate":{"params":{"credential":"private candidate"}}}]}))
        .bind(json!({"attempt":2,"step":{"type":"retry","times":3}}))
        .bind(due).bind(json!({"effects":false,"candidate":{"params":{"credential":"private context"}}}))
        .bind(scene.rig.world.now()).execute(pool).await?;
    for (recovery, state) in [
        ("scheduled", State::Queued),
        ("running", State::Running),
        ("stopped", State::Failed),
    ] {
        sqlx::query("UPDATE model_call_recovery SET state=$2,context=context||$3,action=action||$4 WHERE node_id=$1 AND purpose='agent'")
            .bind(thread).bind(recovery)
            .bind(if recovery=="stopped" {json!({"detail":"repeat_limit"})} else {json!({})})
            .bind(if recovery=="stopped" {json!({"count":4,"window_secs":300})} else {json!({})})
            .execute(pool).await?;
        let sync = scene
            .rig
            .world
            .app
            .call(ThreadSyncGet, &[&thread])
            .token(&scene.cai.token)
            .query(&ThreadSyncGetQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(sync.thread.display.state, state);
        let details = sync.thread.display.recovery.as_ref().unwrap();
        assert_eq!(details.attempt, 2);
        assert_eq!(details.attempts, Some(3));
        assert_eq!(details.due_at, Some(orch_types::Time::new(due)));
        if recovery == "stopped" {
            assert_eq!(
                details.stop_reason,
                Some(orch_api_contract::threads::ThreadRecoveryStop::RepeatLimit)
            );
            assert_eq!((details.count, details.window_secs), (Some(4), Some(300)));
        }
        let serialized = serde_json::to_value(&sync.thread.display)?;
        assert!(!serialized.to_string().contains("private"));
        assert!(!serialized.to_string().contains("params"));
        assert!(!serialized.to_string().contains("said"));
        let tree = scene
            .rig
            .world
            .app
            .call(OrgTreeGet, &[&scene.tree.org.base.id])
            .token(&scene.cai.token)
            .query(&OrgTreeGetQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(
            tree_display(serde_json::to_value(tree)?, thread),
            serialized
        );
    }
    sqlx::query("UPDATE model_call_recovery SET state='completed' WHERE node_id=$1")
        .bind(thread)
        .execute(pool)
        .await?;
    let sync = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(sync.thread.display.state, State::Blocked);
    assert!(sync.thread.display.recovery.is_none());
    Ok(())
}

fn tree_display(tree: Value, thread: orch_types::Id) -> Value {
    tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == thread.to_string())
        .unwrap()["thread"]["display"]
        .clone()
}

async fn assert_search_display(
    scene: &rig::Scene,
    token: &str,
    thread: orch_types::Id,
    expected: Option<&ThreadDisplay>,
) -> Proof {
    for text in ["State answer", "Fix login"] {
        let query: SearchListQuery = serde_json::from_value(json!({
            "q": text, "sort": "relevance", "immediate": true
        }))?;
        let page = scene
            .rig
            .world
            .app
            .call(SearchList, &[])
            .token(token)
            .query(&query)
            .send()
            .await
            .ok();
        let stream = scene
            .rig
            .world
            .app
            .call(SearchStreamList, &[])
            .token(token)
            .query(&query)
            .send_bytes()
            .await;
        assert_eq!(stream.status, 200, "{text}");
        let streamed = stream
            .raw
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice::<SearchStreamEvent>)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|event| match event {
                SearchStreamEvent::Hit { hit, .. } => Some(*hit),
                _ => None,
            })
            .collect();
        for (source, hits) in [("json", page.items), ("stream", streamed)] {
            let refs: Vec<_> = hits
                .iter()
                .filter_map(|hit| match &hit.found {
                    SearchFound::Item { thread, .. } => Some(thread),
                    SearchFound::Name { node, .. } => Some(node),
                    _ => None,
                })
                .filter(|node| node.id == thread)
                .collect();
            if let Some(expected) = expected {
                let name: String = sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1")
                    .bind(thread)
                    .fetch_one(scene.rig.db.pool())
                    .await?;
                assert!(
                    !refs.is_empty(),
                    "the thread must be found by {text} in {source}; current name {name}; hits {hits:#?}"
                );
                for node in refs {
                    assert_eq!(node.thread_display.as_deref(), Some(expected), "{text}");
                }
            } else {
                assert!(
                    refs.is_empty(),
                    "an unreadable thread has no search payload"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn completed_work_has_one_reader_specific_icon_on_lists_references_and_the_thread() -> Proof {
    let scene = rig::Scene::new().await;
    let made = scene.thread().await;
    let thread = made.base.id;
    scene
        .exchange(thread, 0, "Check the state", "State answer")
        .await;
    rig::eventually("the completed turn's follow-up settles before comparing read snapshots", || async {
        sqlx::query_scalar::<_, bool>("SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1 AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))")
            .bind(thread).fetch_one(scene.rig.db.pool()).await.unwrap_or(false)
    }).await;

    let app = &scene.rig.world.app;
    let token = &scene.cai.token;
    let sync = app
        .call(ThreadSyncGet, &[&thread])
        .token(token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(sync.thread.display.state, State::Finished);
    assert!(sync.thread.display.finished_at.is_some());
    let expected = serde_json::to_value(&sync.thread.display)?;
    let tree = app
        .call(OrgTreeGet, &[&scene.tree.org.base.id])
        .token(token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(tree_display(serde_json::to_value(tree)?, thread), expected);
    let node = app.call(NodeGet, &[&thread]).token(token).send().await.ok();
    let Node::Thread(node) = node else {
        panic!("a thread read returns a thread")
    };
    assert_eq!(node.display, sync.thread.display);
    assert_eq!(
        node.base.path.last().unwrap().thread_display.as_deref(),
        Some(&node.display)
    );
    let children = app
        .call(NodeChildList, &[&scene.tree.topic.base.id])
        .token(token)
        .query(&NodeChildListQuery::default())
        .send()
        .await
        .ok();
    let Node::Thread(child) = children
        .items
        .into_iter()
        .find_map(|row| match row {
            orch_api_contract::common::Listed::Visible(node) => Some(node.content),
            _ => None,
        })
        .unwrap()
    else {
        panic!("the topic holds the thread")
    };
    assert_eq!(child.display, sync.thread.display);
    let found = app
        .call(NameSearchList, &[])
        .token(token)
        .query(&NameSearchListQuery {
            q: Some("Fix login".into()),
            kinds: Some("thread".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        found.items[0].thread_display.as_ref(),
        Some(&sync.thread.display)
    );
    let all = app
        .call(EverythingList, &[])
        .token(token)
        .query(&EverythingListQuery {
            q: Some("Fix login".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        all.items
            .iter()
            .find(|row| row.id == thread.to_string())
            .unwrap()
            .thread_display
            .as_ref(),
        Some(&sync.thread.display)
    );
    let recent = app
        .call(EverythingList, &[])
        .token(token)
        .query(&EverythingListQuery {
            recent: Some(EverythingRecent::Active),
            kinds: Some("thread".into()),
            q: Some("Fix login".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        recent
            .items
            .iter()
            .find(|row| row.id == thread.to_string())
            .unwrap()
            .thread_display
            .as_ref(),
        Some(&sync.thread.display)
    );
    let bound = app
        .call(ProviderThreadList, &[&scene.on.provider])
        .token(&scene.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        bound.items[0].thread.thread_display.as_ref().unwrap().state,
        State::Finished
    );
    let preview = app
        .call(ReferencePreviewGet, &[])
        .token(token)
        .query(&ReferencePreviewGetQuery {
            kind: InlineRefKind::Node,
            id: thread,
            block: None,
            turn: None,
            message: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(preview.thread_display.as_ref(), Some(&sync.thread.display));
    assert_search_display(&scene, token, thread, Some(&sync.thread.display)).await?;
    let read = app
        .call(ThreadRead, &[&thread])
        .token(token)
        .body(&ThreadReadBody {
            seq: sync.seq,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(read.thread_display.unwrap().state, State::Idle);
    let mine = app
        .call(ThreadSyncGet, &[&thread])
        .token(token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(mine.thread.display.state, State::Idle);
    assert_search_display(&scene, token, thread, Some(&mine.thread.display)).await?;
    let other = app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.eve.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(other.thread.display.state, State::Finished);
    assert_search_display(
        &scene,
        &scene.eve.token,
        thread,
        Some(&other.thread.display),
    )
    .await?;
    assert_search_display(&scene, &scene.bea.token, thread, None).await?;
    // No View means no preview or icon facts, even though this caller knows the identity.
    let denied = app
        .call(ReferencePreviewGet, &[])
        .token(&scene.bea.token)
        .query(&ReferencePreviewGetQuery {
            kind: InlineRefKind::Node,
            id: thread,
            block: None,
            turn: None,
            message: None,
        })
        .send()
        .await;
    assert_eq!(denied.status, 404);
    assert!(
        !serde_json::to_value(denied.problem())?
            .to_string()
            .contains("finished_at")
    );
    Ok(())
}

#[tokio::test]
async fn queue_holds_and_waiting_people_are_counted_from_the_same_active_continuation() -> Proof {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    let pool = scene.rig.db.pool();
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(thread)
        .execute(pool)
        .await?;
    let mut conn = pool.acquire().await?;
    let facts = orch_db::queries::source::thread_display_facts(
        &mut conn,
        &[thread],
        Some(scene.cai.id()),
        orch_types::Time::new(scene.rig.world.now()),
    )
    .await?;
    assert_eq!(thread_display(&facts[0]).state, State::Held);
    drop(conn);
    sqlx::query("INSERT INTO thread_inputs(id,thread_id,source,content,delivery,state,position,created_at,seq_created,seq_updated,queue_held,state_detail) VALUES($1,$2,'system','[]','queue','pending',1,$3,1,1,false,'model_use_required')")
        .bind(orch_types::new_id()).bind(thread).bind(scene.rig.world.now()).execute(pool).await?;
    let mut conn = pool.acquire().await?;
    let facts = orch_db::queries::source::thread_display_facts(
        &mut conn,
        &[thread],
        Some(scene.cai.id()),
        orch_types::Time::new(scene.rig.world.now()),
    )
    .await?;
    assert_eq!(facts[0].waiting_inputs, 1);
    assert_eq!(thread_display(&facts[0]).state, State::WaitingApproval);
    assert_eq!(
        serde_json::to_value(thread_display(&facts[0]))?["waiting_inputs"],
        json!(1)
    );
    Ok(())
}

#[tokio::test]
async fn a_topics_contained_work_includes_readable_sides_without_changing_its_main_thread_count()
-> Proof {
    let scene = rig::Scene::new().await;
    let main = scene.thread().await.base.id;
    let side = orch_types::new_id();
    let pool = scene.rig.db.pool();
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by) SELECT $2,'thread',id,org_id,path||$2::uuid,'A held aside',owner_id,created_by FROM nodes WHERE id=$1")
        .bind(main).bind(side).execute(pool).await?;
    sqlx::query("INSERT INTO threads (node_id,topic_id,environment_id,workspace_dir,agent_id,provider_id,arrangement,side_of,side_reading,queue_held) SELECT $2,topic_id,environment_id,workspace_dir,agent_id,provider_id,arrangement,node_id,'read_only',true FROM threads WHERE node_id=$1")
        .bind(main).bind(side).execute(pool).await?;
    scene
        .rig
        .lets_use("node", side, "access.own", &scene.ada, json!({}))
        .await;
    let app = &scene.rig.world.app;
    let topic_id = scene.tree.topic.base.id;
    let topic = app
        .call(NodeGet, &[&topic_id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let Node::Topic(topic) = topic else {
        panic!("a topic read returns a topic")
    };
    assert_eq!(topic.thread_display_state, Some(State::Held));
    assert_eq!(topic.thread_count, 1);
    assert_eq!(topic.work_attention.failed, 0);
    let tree = app
        .call(OrgTreeGet, &[&scene.tree.org.base.id])
        .token(&scene.ada.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        tree_display(serde_json::to_value(tree)?, side)["state"],
        "held"
    );
    sqlx::query("UPDATE nodes SET archived_at=$2,archived_by=$3 WHERE id=$1")
        .bind(side)
        .bind(scene.rig.world.now())
        .bind(scene.ada.id())
        .execute(pool)
        .await?;
    let topic = app
        .call(NodeGet, &[&topic_id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let Node::Topic(topic) = topic else {
        panic!("a topic read returns a topic")
    };
    assert_eq!(topic.thread_display_state, None);
    assert_eq!(topic.thread_count, 1);
    Ok(())
}

#[tokio::test]
async fn a_thousand_contained_threads_keep_metadata_pages_bounded_and_include_the_last_readable_side()
-> Proof {
    let scene = rig::Scene::new().await;
    let main = scene.thread().await.base.id;
    let pool = scene.rig.db.pool();
    // SQL builds the large fixture without a request or insertion loop per thread.
    sqlx::query("INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
        SELECT md5('scale-thread-'||n)::uuid,'thread',CASE WHEN n%2=0 THEN seed.id ELSE topic.id END,
          seed.org_id,(CASE WHEN n%2=0 THEN seed.path ELSE topic.path END)||md5('scale-thread-'||n)::uuid,
          'Scale cursor '||n,seed.owner_id,seed.created_by FROM nodes seed JOIN nodes topic ON topic.id=seed.parent_id
        CROSS JOIN generate_series(1,1000)n WHERE seed.id=$1")
        .bind(main).execute(pool).await?;
    sqlx::query("INSERT INTO threads(node_id,topic_id,environment_id,workspace_dir,agent_id,provider_id,arrangement,side_of,side_reading)
        SELECT md5('scale-thread-'||n)::uuid,seed.topic_id,seed.environment_id,seed.workspace_dir,seed.agent_id,
          seed.provider_id,seed.arrangement,CASE WHEN n%2=0 THEN seed.node_id ELSE NULL END,
          CASE WHEN n%2=0 THEN 'read_only' ELSE NULL END FROM threads seed CROSS JOIN generate_series(1,1000)n WHERE seed.node_id=$1")
        .bind(main).execute(pool).await?;
    let held: orch_types::Id = sqlx::query_scalar(
        "UPDATE threads SET queue_held=true WHERE node_id=(SELECT node_id FROM threads
        WHERE side_of=$1 ORDER BY node_id DESC LIMIT 1) RETURNING node_id",
    )
    .bind(main)
    .fetch_one(pool)
    .await?;
    scene
        .rig
        .lets_use("node", held, "access.own", &scene.ada, json!({}))
        .await;
    let topic = scene.tree.topic.base.id;
    let batch = usize::try_from(orch_api_contract::common::MAX_LIMIT)?;
    let total = 1_001_usize;
    let mut conn = pool.acquire().await?;
    let mut after = None;
    let mut seen = std::collections::HashSet::new();
    let mut pages = 0;
    let mut held_page = 0;
    loop {
        let rows = orch_db::queries::source::topic_threads_page(
            &mut conn,
            &[topic],
            after,
            i64::try_from(batch)?,
        )
        .await?;
        assert!(!rows.is_empty());
        assert!(rows.len() <= batch);
        pages += 1;
        for (place, thread, _) in &rows {
            assert_eq!(*place, topic);
            assert!(seen.insert(*thread), "a cursor cannot repeat metadata");
            if *thread == held {
                held_page = pages;
            }
        }
        after = rows.last().map(|(place, thread, _)| (*place, *thread));
        if rows.len() < batch {
            break;
        }
    }
    assert_eq!(seen.len(), total);
    assert_eq!(pages, total.div_ceil(batch));
    assert!(
        held_page > 1,
        "the held side must be found beyond the first metadata page"
    );
    drop(conn);
    let node = scene
        .rig
        .world
        .app
        .call(NodeGet, &[&topic])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let Node::Topic(node) = node else {
        panic!("a topic read returns its topic")
    };
    assert_eq!(node.thread_count, 501);
    assert_eq!(node.thread_display_state, Some(State::Held));
    assert_eq!(
        (
            node.work_attention.failed,
            node.work_attention.uncommitted,
            node.work_attention.unpushed
        ),
        (0, 0, 0)
    );
    Ok(())
}
