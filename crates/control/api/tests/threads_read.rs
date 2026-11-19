//! A thread is read over HTTP: a sync that brings a client up to date, a stretch of the timeline, one item, a command's output as text, the outline, the catch-up, how far a person has read, their draft, and what was pinned; each refused to whoever may not read, or may not drive, the thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::personal::ThreadRead;
use orch_api_contract::personal::{Draft, ThreadReadBody};
use orch_api_contract::threads::{
    ContentPart, Delivery, ItemKind, OutlineKind, TOTAL_LENGTH_HEADER, ThreadCatchupGet,
    ThreadCatchupGetQuery, ThreadDraftDelete, ThreadDraftGet, ThreadDraftSet, ThreadDraftSetBody,
    ThreadItemGet, ThreadItemList, ThreadItemListQuery, ThreadItemOutputGet,
    ThreadItemOutputGetQuery, ThreadItemPin, ThreadItemUnpin, ThreadOutlineGet, ThreadSyncGet,
    ThreadSyncGetQuery,
};
use orch_types::{Id, new_id};
use places::missing_permissions;
use rig::{Scene, said};
use serde_json::json;

fn text(words: &str) -> Vec<ContentPart> {
    vec![ContentPart::Text {
        text: words.to_owned(),
    }]
}

#[tokio::test]
async fn goal_inputs_have_bounded_human_outline_titles_in_opening_and_older_pages() {
    use orch_api_contract::threads::{
        OUTLINE_TITLE_CHARS, ThreadOpeningGet, ThreadOpeningGetQuery, ThreadOutlineGetQuery,
    };
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Work on the redirect", "The redirect is fixed.")
        .await;
    let condition = format!(
        "npm test exits 0{}",
        " and all redirects stay local".repeat(10)
    );
    let facts = json!({"goal_id":new_id(), "condition":condition, "turn":1,
        "max_turns":30, "tokens":0, "not_yet":null, "plan":false});
    let (item, seq): (Id, i64) = sqlx::query_as(
        "UPDATE items SET payload=payload || jsonb_build_object('source','goal',
          'parts',jsonb_build_array(jsonb_build_object('type','text','text','<orch>\nInternal instructions\n</orch>')),
          'goal',$2::jsonb) WHERE thread_id=$1 AND kind='user_message' RETURNING id,seq_created"
    ).bind(thread).bind(facts).fetch_one(scene.rig.db.pool()).await.unwrap();
    let title: String = condition
        .chars()
        .take(usize::try_from(OUTLINE_TITLE_CHARS).unwrap())
        .collect();
    for reader in [&scene.ada, &scene.cai] {
        let page = scene
            .rig
            .world
            .app
            .call(ThreadOutlineGet, &[&thread])
            .token(&reader.token)
            .query(&ThreadOutlineGetQuery {
                before: Some(seq + 1),
                limit: Some(1),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        let line = page
            .entries
            .iter()
            .find(|entry| entry.item_id == item && entry.kind == OutlineKind::UserMessage)
            .unwrap();
        assert_eq!(line.title, title);
        assert_eq!(line.goal_condition.as_deref(), Some(title.as_str()));
        let opening = scene
            .rig
            .world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .query(&ThreadOpeningGetQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(
            opening
                .outline
                .entries
                .iter()
                .find(|entry| entry.item_id == item && entry.kind == OutlineKind::UserMessage),
            Some(line)
        );
    }
    // The outline projection must not fetch the internal instruction parts just to discard them later.
    let mut conn = scene.rig.db.pool().acquire().await.unwrap();
    let projected = orch_db::queries::threads::reading::outline_points(
        &mut conn,
        thread,
        Some(seq + 1),
        false,
        1,
        i64::from(OUTLINE_TITLE_CHARS),
    )
    .await
    .unwrap();
    assert_eq!(projected[0].payload["parts"], json!([]));
    assert_eq!(projected[0].payload["goal_condition"], json!(title));
}

#[tokio::test]
async fn paused_and_stopped_goals_survive_sync_and_opening() {
    use orch_api_contract::goals::{GoalStatus, ThreadGoalGet};
    use orch_api_contract::threads::{ThreadOpeningGet, ThreadOpeningGetQuery};

    let scene = Scene::new().await;
    for (status, stored, reason) in [
        (GoalStatus::Paused, "paused", "person"),
        (GoalStatus::Stopped, "stopped", "interrupted"),
    ] {
        let thread = scene.thread().await.base.id;
        let goal = new_id();
        // No running loop or live frame supplies this fixture: an initial HTTP read must recover it.
        sqlx::query(
            "INSERT INTO thread_goals
             (id, thread_id, objective, condition, set_by, set_at, status, reason,
              max_turns, turns_used, elapsed_ms, ended_at)
             VALUES ($1, $2, 'Greet the team three times', 'Three greetings are recorded',
                     $3, now(), $4, $5, 30, 7, 720000,
                     CASE WHEN $4 = 'stopped' THEN now() ELSE NULL END)",
        )
        .bind(goal)
        .bind(thread)
        .bind(scene.ada.id())
        .bind(stored)
        .bind(reason)
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
        let app = &scene.rig.world.app;
        let expected = app
            .call(ThreadGoalGet, &[&thread])
            .token(&scene.ada.token)
            .send()
            .await
            .ok()
            .expect("the stored goal is visible");
        assert_eq!(expected.id, goal);
        assert_eq!(expected.status, status);
        for reader in [&scene.ada, &scene.cai] {
            let sync = app
                .call(ThreadSyncGet, &[&thread])
                .token(&reader.token)
                .query(&ThreadSyncGetQuery::default())
                .send()
                .await
                .ok();
            assert_eq!(sync.thread.goal.as_ref(), Some(&expected));
            let opening = app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .query(&ThreadOpeningGetQuery::default())
                .send()
                .await
                .ok();
            assert_eq!(opening.sync.thread.goal.as_ref(), Some(&expected));
        }
    }
}

#[tokio::test]
async fn opening_carries_the_native_screen_projections_and_only_the_readers_own_draft() {
    use orch_api_contract::settings::{NodeSettingList, ThreadParameterGet};
    use orch_api_contract::threads::{
        Delivery, ThreadLimitsGet, ThreadLimitsGetQuery, ThreadOpeningGet, ThreadOpeningGetQuery,
        ThreadOutlineGetQuery,
    };
    let scene = Scene::new().await;
    let id = scene.thread().await.base.id;
    scene
        .exchange(id, 0, "First question", "First answer")
        .await;
    // Compare two ordinary requests only once the real startup receipts and terminal
    // follow-up have committed. Each request still owns an independent coherent cut.
    rig::eventually("the native opening fixture has finished its recorded work", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (
            SELECT 1 FROM turns t WHERE t.thread_id=$1 AND t.ordinal=1 AND t.state='completed'
              AND t.end_work IS NULL AND NOT t.files_end_due
              AND EXISTS (SELECT 1 FROM model_inputs m WHERE m.node_id=$1 AND m.turn_id=t.id
                  AND m.purpose='input' AND m.state='sent')
              AND NOT EXISTS (SELECT 1 FROM model_inputs m WHERE m.node_id=$1 AND m.state='prepared'))")
            .bind(id).fetch_one(scene.rig.db.pool()).await.unwrap()
    }).await;
    let app = &scene.rig.world.app;
    let saved = app
        .call(ThreadDraftSet, &[&id])
        .token(&scene.ada.token)
        .body(&ThreadDraftSetBody {
            native_action: None,
            content: text("Ada's unfinished question"),
            delivery: Delivery::Queue,
            review_comments: None,
        })
        .send()
        .await
        .ok();
    let response = app
        .call(ThreadOpeningGet, &[&id])
        .token(&scene.ada.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await;
    assert_eq!(
        response
            .headers
            .get(http::header::CACHE_CONTROL)
            .and_then(|header| header.to_str().ok()),
        Some("no-store")
    );
    let opening = response.ok();
    assert_eq!(opening.sync.seq, opening.sync.thread.seq);
    assert_eq!(opening.sync.seq, opening.outline.seq);
    assert!(
        opening
            .sync
            .items
            .iter()
            .all(|item| item.seq_updated <= opening.sync.seq)
    );
    assert!(
        opening
            .sync
            .turns
            .iter()
            .all(|turn| turn.seq_updated <= opening.sync.seq)
    );
    assert!(
        opening
            .sync
            .inputs
            .iter()
            .all(|input| input.seq_updated <= opening.sync.seq)
    );
    assert!(
        opening
            .sync
            .segments
            .iter()
            .all(|segment| segment.seq_updated <= opening.sync.seq)
    );
    assert_eq!(
        opening.sync,
        app.call(ThreadSyncGet, &[&id])
            .token(&scene.ada.token)
            .query(&ThreadSyncGetQuery::default())
            .send()
            .await
            .ok()
    );
    assert_eq!(opening.draft, Some(saved));
    assert_eq!(
        opening.parameters,
        Some(
            app.call(ThreadParameterGet, &[&id])
                .token(&scene.ada.token)
                .send()
                .await
                .ok()
        )
    );
    assert_eq!(
        opening.limits,
        Some(
            app.call(ThreadLimitsGet, &[&id])
                .token(&scene.ada.token)
                .query(&ThreadLimitsGetQuery::default())
                .send()
                .await
                .ok()
        )
    );
    assert_eq!(
        opening.settings,
        app.call(NodeSettingList, &[&id])
            .token(&scene.ada.token)
            .send()
            .await
            .ok()
    );
    assert_eq!(
        opening.outline,
        app.call(ThreadOutlineGet, &[&id])
            .token(&scene.ada.token)
            .query(&ThreadOutlineGetQuery::default())
            .send()
            .await
            .ok()
    );
    let other = app
        .call(ThreadOpeningGet, &[&id])
        .token(&scene.dan.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await
        .ok();
    assert!(
        other.draft.is_none(),
        "another interactor never receives Ada's draft"
    );
    let viewer = app
        .call(ThreadOpeningGet, &[&id])
        .token(&scene.cai.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(viewer.sync.thread.base.id, id);
    assert!(
        viewer.draft.is_none(),
        "viewing alone does not admit a private draft"
    );
    let denied = app
        .call(ThreadOpeningGet, &[&id])
        .token(&scene.bea.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&denied), ["thread.view".to_owned()]);
    assert_eq!(
        app.call(ThreadOpeningGet, &[&id])
            .query(&ThreadOpeningGetQuery::default())
            .send()
            .await
            .status,
        401
    );
    assert_eq!(
        app.call(ThreadOpeningGet, &[&id])
            .token(&scene.ada.token)
            .query(&ThreadOpeningGetQuery {
                limit: Some(0),
                ..Default::default()
            })
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn a_sandbox_block_exposes_the_actual_machine_limit_and_its_authorized_policy_act() {
    use orch_api_contract::environment::{EnvironmentStatus, SandboxLimitSource, SandboxMode};
    use orch_api_contract::threads::{ThreadOpeningGet, ThreadOpeningGetQuery};
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let mut conn = scene.rig.world.conn().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        scene.ada.id(),
        "world-box",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(machine)
        .bind(
            json!({"environment_descriptor":{"confinement":"none","lifecycle":"attach"},
            "policy":{"agent.max_sandbox":"workspace"}}),
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2,placement='connected',template_id=NULL,facts=$3 WHERE id=$1")
        .bind(scene.on.environment).bind(machine)
        .bind(json!({"executors":[{"kind":"orch-exec","available":true,"sandbox_modes":["open"],"tool_sets":["plain"]}]}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE threads SET state='blocked',blocked_reason='binding_invalid',blocked_detail=$2,executor='orch-exec' WHERE node_id=$1")
        .bind(thread).bind(orch_engine::sessions::SANDBOX_UNSUPPORTED).execute(&mut *conn).await.unwrap();
    drop(conn);
    assert_ne!(machine, scene.rig.machine_id);
    assert!(
        scene.rig.hub.session_of(machine).is_none(),
        "the policy fixture's machine is genuinely unattached"
    );
    // The running supervisor must finish following the new offline machine
    // before two independent HTTP snapshots can compare its complete reference.
    rig::eventually(
        "the unattached directory follows its offline machine",
        || async {
            let mut conn = scene.rig.world.conn().await;
            let state =
                orch_db::queries::environments::hosting::connected(&mut conn, scene.on.environment)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(state.machine_id, machine);
            assert!(!state.here, "the fixture must not invent an online machine");
            state.status == "offline"
        },
    )
    .await;
    let read = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    let readiness = read.blocked.as_ref().unwrap().sandbox.as_ref().unwrap();
    assert_eq!(readiness.environment.status, EnvironmentStatus::Offline);
    assert_eq!(readiness.machine.as_ref().unwrap().id, machine);
    assert!(!readiness.machine.as_ref().unwrap().online);
    assert_eq!(readiness.machine.as_ref().unwrap().name, "world-box");
    assert!(readiness.policy_manage);
    assert!(!readiness.sandbox.ready);
    assert_eq!(readiness.sandbox.source, SandboxLimitSource::MachinePolicy);
    assert_eq!(readiness.sandbox.widest, SandboxMode::Workspace);
    assert_eq!(readiness.sandbox.enforceable, vec![SandboxMode::Open]);
    assert_eq!(readiness.sandbox.template_limit, None);
    let opening = scene
        .rig
        .world
        .app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await
        .ok();
    // Opening a thread may find its block anew; a standalone sync read after it says the same.
    let after = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert_eq!(
        opening.sync.thread.blocked, after.blocked,
        "the bundled opening must retain the same authorized sandbox recovery as standalone sync"
    );
    sqlx::query("UPDATE threads SET blocked_detail='second_step_required' WHERE node_id=$1")
        .bind(thread)
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    let security = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert!(
        security.blocked.unwrap().sandbox.is_none(),
        "a sender's security step never offers sandbox recovery"
    );
}

#[tokio::test]
async fn request_urls_in_only_the_last_answer_are_suggestions_until_an_interactor_accepts_them() {
    use orch_api_contract::source::{ThreadLinksSet, ThreadLinksSetBody};
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(
            thread,
            0,
            "Show the result",
            "Review [the change](https://github.com/team/repo/pull/12).",
        )
        .await;
    let shown = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert!(shown.links.is_empty());
    assert_eq!(shown.suggested_links.len(), 1);
    assert_eq!(
        shown.suggested_links[0].url,
        "https://github.com/team/repo/pull/12"
    );
    let accepted = scene
        .rig
        .world
        .app
        .call(ThreadLinksSet, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadLinksSetBody {
            links: shown.suggested_links.clone(),
        })
        .send()
        .await
        .ok();
    assert_eq!(accepted, shown.suggested_links);
    scene
        .exchange(thread, 1, "Continue", "The next result has no request URL.")
        .await;
    let later = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert_eq!(later.links, accepted);
    assert!(
        later.suggested_links.is_empty(),
        "an older answer is not an automatic saved link"
    );
}

#[tokio::test]
async fn frozen_review_comments_remain_private_and_survive_a_text_save_from_another_device() {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .rig
        .lets_use(
            "provider",
            scene.on.provider,
            "provider.use",
            &scene.dan,
            json!({}),
        )
        .await;
    let comment = orch_api_contract::source::ReviewComment {
        id: new_id(),
        path: "src/main.rs".to_owned(),
        side: orch_api_contract::source::ReviewSide::New,
        from_line: 2,
        to_line: 3,
        diff_hash: orch_types::Hash::of(b"reviewed difference"),
        item_id: None,
        hunk: "@@ -1 +1,3 @@\n-old\n+new\n+reviewed\n".to_owned(),
        text: "Please check this range".to_owned(),
    };
    let saved = scene
        .rig
        .world
        .app
        .call(ThreadDraftSet, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadDraftSetBody {
            native_action: None,
            content: text("Review notes"),
            delivery: Delivery::Queue,
            review_comments: Some(vec![comment.clone()]),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        saved.review_comments.as_slice(),
        std::slice::from_ref(&comment)
    );
    scene
        .rig
        .world
        .app
        .call(ThreadDraftSet, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadDraftSetBody {
            native_action: None,
            content: text("Text from the other device"),
            delivery: Delivery::Queue,
            review_comments: None,
        })
        .send()
        .await
        .ok();
    let current = scene
        .rig
        .world
        .app
        .call(ThreadDraftGet, &[&thread])
        .token(&scene.ada.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(current.content, text("Text from the other device"));
    assert_eq!(current.review_comments, [comment]);
    assert!(
        scene
            .rig
            .world
            .app
            .call(ThreadDraftGet, &[&thread])
            .token(&scene.dan.token)
            .send()
            .await
            .ok()
            .is_none()
    );
}

#[tokio::test]
async fn accepted_thread_links_are_visible_in_sync_and_an_invalid_replacement_changes_nothing() {
    use orch_api_contract::source::{ThreadLink, ThreadLinksSet, ThreadLinksSetBody};
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let links = vec![ThreadLink {
        url: "https://github.com/team/project/pull/7".to_owned(),
        label: "Pull request".to_owned(),
    }];
    let shown = scene
        .rig
        .world
        .app
        .call(ThreadLinksSet, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadLinksSetBody {
            links: links.clone(),
        })
        .send()
        .await
        .ok();
    assert_eq!(shown, links);
    let invalid = ThreadLinksSetBody {
        links: vec![
            ThreadLink {
                url: "https://example.invalid/new".to_owned(),
                label: "New".to_owned(),
            },
            ThreadLink {
                url: "https://password@example.invalid/private".to_owned(),
                label: "Unsafe".to_owned(),
            },
        ],
    };
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadLinksSet, &[&thread])
        .token(&scene.ada.token)
        .body(&invalid)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
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
    assert_eq!(sync.thread.links, links);
    let changed = sync
        .items
        .iter()
        .filter(|item| {
            matches!(
                &item.payload,
                orch_api_contract::threads::ItemPayload::Marker(
                    orch_api_contract::threads::MarkerPayload::LinksChanged(_)
                )
            )
        })
        .count();
    assert_eq!(changed, 1);
    let forbidden = scene
        .rig
        .world
        .app
        .call(ThreadLinksSet, &[&thread])
        .token(&scene.cai.token)
        .body(&ThreadLinksSetBody { links: Vec::new() })
        .send()
        .await
        .problem();
    assert_eq!(forbidden.code, ErrorCode::AuthzDenied);
}

/// Puts a finished command into the timeline, as the engine would have after the harness ran one.
async fn command(s: &Scene, thread: Id, output: &str) -> Id {
    command_printing(
        s,
        thread,
        json!({ "inline": output, "total_len": output.len(), "truncated": false, "blob": null }),
    )
    .await
}

/// The same, with the output as the item keeps it: the end of it inline, and the whole in the store
/// of content where it was longer.
async fn command_printing(s: &Scene, thread: Id, output: serde_json::Value) -> Id {
    let mut conn = s.rig.db.acquire().await.unwrap();
    let seq = orch_db::sequences::next(&mut conn, thread)
        .await
        .unwrap()
        .unwrap();
    let id = new_id();
    let segment = s.rig.segment_of(thread).await;
    sqlx::query(
        "INSERT INTO items (id, thread_id, segment_id, stream, kind, status, payload, seq_created, seq_updated)
         VALUES ($1, $2, $3, 'main', 'tool_call', 'completed', $4, $5, $5)",
    )
    .bind(id)
    .bind(thread)
    .bind(segment)
    .bind(json!({
        "tool": {
            "kind": "command", "command": "cargo test", "cwd": "/workspace/web", "exit_code": 0,
            "output": output,
            "process_id": null, "background": false, "actions": []
        },
        "title": "Run the tests", "started_at": "2026-10-08T14:00:00.000Z", "duration_ms": 5,
        "raw": { "name": "Bash", "input": { "inline": "{}", "total_len": 2, "truncated": false, "blob": null }, "output": null, "is_error": false }
    }))
    .bind(seq)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_sync_brings_a_client_up_to_date_and_then_tells_only_what_changed() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let id = thread.base.id;
    s.exchange(id, 0, "Why does it loop?", "A stale cookie.")
        .await;
    let call = |since: Option<i64>, limit: Option<u32>| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadSyncGet, &[&id])
            .token(&s.ada.token)
            .query(&ThreadSyncGetQuery {
                since,
                limit,
                ..Default::default()
            });
        async move { call.send().await }
    };
    let first = call(None, None).await.ok();
    assert_eq!(
        said(&first.items),
        ["user: Why does it loop?", "agent: A stale cookie."]
    );
    assert_eq!(first.segments.len(), 1);
    assert!(
        first
            .items
            .iter()
            .any(|item| item.kind() == ItemKind::Marker)
    );
    let quiet = call(Some(first.seq), None).await.ok();
    assert!(quiet.items.is_empty() && quiet.turns.is_empty() && quiet.complete);
    assert_eq!(quiet.since, Some(first.seq));
    s.exchange(id, 1, "And then?", "Clear it.").await;
    let later = call(Some(first.seq), None).await.ok();
    assert_eq!(said(&later.items), ["user: And then?", "agent: Clear it."]);
    assert_eq!(later.turns.len(), 1, "only the turn that changed");

    // A limit cuts the items to the latest and says that older ones exist.
    let cut = call(None, Some(2)).await.ok();
    assert_eq!(cut.items.len(), 2);
    assert!(cut.has_older);
    let nothing = call(None, Some(0)).await.problem();
    assert_eq!(nothing.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn items_are_read_before_after_or_around_a_place_and_exactly_one_place_is_named() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "one", "first").await;
    s.exchange(id, 1, "two", "second").await;
    let list = |query: ThreadItemListQuery| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadItemList, &[&id])
            .token(&s.cai.token)
            .query(&query);
        async move { call.send().await }
    };
    let all = list(ThreadItemListQuery {
        after: Some(0),
        ..ThreadItemListQuery::default()
    })
    .await
    .ok();
    assert!(!all.has_older && !all.has_newer);
    let seqs: Vec<_> = all.items.iter().map(|item| item.seq_created).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "oldest first: {seqs:?}"
    );
    let last = all.items.last().unwrap();
    let before = list(ThreadItemListQuery {
        before: Some(last.seq_created),
        limit: Some(2),
        ..ThreadItemListQuery::default()
    })
    .await
    .ok();
    assert_eq!(before.items.len(), 2);
    assert!(before.has_older && before.has_newer);
    let around = list(ThreadItemListQuery {
        around: Some(all.items[2].id),
        limit: Some(3),
        ..ThreadItemListQuery::default()
    })
    .await
    .ok();
    assert!(around.items.iter().any(|item| item.id == all.items[2].id));
    let second_turn = list(ThreadItemListQuery {
        turn: Some(2),
        limit: Some(2),
        ..ThreadItemListQuery::default()
    })
    .await
    .ok();
    assert_eq!(
        second_turn.items.len(),
        2,
        "the limit counts every native item, including context markers"
    );
    assert!(
        second_turn.has_newer,
        "the next raw-item page retains the rest of the answer"
    );
    let mut has_newer = second_turn.has_newer;
    let mut complete_turn = second_turn.items;
    while has_newer {
        let after = complete_turn.last().unwrap().seq_created;
        let rest = list(ThreadItemListQuery {
            after: Some(after),
            limit: Some(2),
            ..ThreadItemListQuery::default()
        })
        .await
        .ok();
        assert!(!rest.items.is_empty());
        assert!(rest.items.len() <= 2);
        assert!(rest.items.iter().all(|item| item.seq_created > after));
        assert!(
            complete_turn
                .iter()
                .all(|item| rest.items.iter().all(|next| next.id != item.id))
        );
        assert!(rest.items.iter().all(|item| {
            rest.segments
                .iter()
                .any(|segment| segment.id == item.segment_id)
        }));
        assert!(
            rest.items
                .iter()
                .filter_map(|item| item.turn_id)
                .all(|turn_id| rest
                    .turns
                    .iter()
                    .any(|turn| turn.id == turn_id && turn.ordinal == 2))
        );
        has_newer = rest.has_newer;
        complete_turn.extend(rest.items);
        assert!(
            complete_turn.len() <= all.items.len(),
            "continuations cannot invent items beyond this finished fixture"
        );
    }
    assert_eq!(said(&complete_turn), ["user: two", "agent: second"]);
    assert!(
        complete_turn
            .iter()
            .all(|item| item.turn_id.is_some_and(|turn_id| all
                .turns
                .iter()
                .any(|turn| turn.id == turn_id && turn.ordinal == 2)))
    );
    let unknown_turn = list(ThreadItemListQuery {
        turn: Some(100),
        ..ThreadItemListQuery::default()
    })
    .await
    .problem();
    assert_eq!(unknown_turn.code, ErrorCode::NotFound);
    for wrong in [
        ThreadItemListQuery::default(),
        ThreadItemListQuery {
            turn: Some(2),
            around: Some(all.items[2].id),
            ..ThreadItemListQuery::default()
        },
        ThreadItemListQuery {
            before: Some(3),
            after: Some(1),
            ..ThreadItemListQuery::default()
        },
    ] {
        assert_eq!(list(wrong).await.problem().code, ErrorCode::RequestInvalid);
    }
}

#[tokio::test]
async fn one_item_is_read_and_an_item_of_another_thread_is_not_found() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let other = s.thread_by(&s.ada, "Other").await.base.id;
    s.exchange(id, 0, "one", "first").await;
    let items = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&id])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            after: Some(0),
            ..ThreadItemListQuery::default()
        })
        .send()
        .await
        .ok()
        .items;
    let read = |thread: Id, item: Id| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadItemGet, &[&thread, &item])
            .token(&s.ada.token);
        async move { call.send().await }
    };
    assert_eq!(read(id, items[0].id).await.ok().id, items[0].id);
    assert_eq!(
        read(other, items[0].id).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(read(id, new_id()).await.problem().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_commands_output_is_read_as_plain_text_in_pieces_with_its_whole_length_in_a_header() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let item = command(&s, id, "running 3 tests\nall passed\n").await;
    let router_call = |query: ThreadItemOutputGetQuery, who: &places::Who| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadItemOutputGet, &[&id, &item])
            .token(&who.token)
            .query(&query);
        async move { call.send_bytes().await }
    };
    let whole = router_call(ThreadItemOutputGetQuery::default(), &s.cai).await;
    assert_eq!(whole.status, 200);
    assert_eq!(
        std::str::from_utf8(&whole.raw).unwrap(),
        "running 3 tests\nall passed\n"
    );
    assert_eq!(
        whole
            .headers
            .get(TOTAL_LENGTH_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        "27"
    );
    let stranger = router_call(ThreadItemOutputGetQuery::default(), &s.bea).await;
    assert_eq!(stranger.problem().code, ErrorCode::AuthzDenied);
    let piece = router_call(
        ThreadItemOutputGetQuery {
            offset: Some(8),
            limit: Some(7),
            ..Default::default()
        },
        &s.cai,
    )
    .await;
    assert_eq!(std::str::from_utf8(&piece.raw).unwrap(), "3 tests");
    assert_eq!(
        piece
            .headers
            .get(TOTAL_LENGTH_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        "27",
        "the whole length is told however little is read"
    );
    // An item that is no command has no output.
    s.exchange(id, 0, "hi", "there").await;
    let message = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&id])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            after: Some(0),
            ..ThreadItemListQuery::default()
        })
        .send()
        .await
        .ok()
        .items
        .into_iter()
        .find(|item| item.kind() == ItemKind::AgentMessage)
        .unwrap();
    let none = s
        .rig
        .world
        .app
        .call(ThreadItemOutputGet, &[&id, &message.id])
        .token(&s.ada.token)
        .query(&ThreadItemOutputGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(none.problem().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_output_longer_than_the_item_keeps_is_read_in_pieces_from_the_store_of_content() {
    use orch_blob_contract::store::stream_of;
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let app = &s.rig.world.app.app;
    app.engine.attach_blobs(app.blobs.store());
    let whole: String = (0..2_000).map(|n| format!("line {n:04}\n")).collect();
    let hash = orch_types::Hash::of(whole.as_bytes());
    app.blobs
        .store()
        .put(
            &hash,
            Some(i64::try_from(whole.len()).unwrap()),
            stream_of(vec![bytes::Bytes::from(whole.clone())]),
        )
        .await
        .unwrap();
    let tail = &whole[whole.len() - 50..];
    let item = command_printing(
        &s,
        id,
        json!({ "inline": tail, "total_len": whole.len(), "truncated": true, "blob": hash }),
    )
    .await;
    let read = |query: ThreadItemOutputGetQuery| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadItemOutputGet, &[&id, &item])
            .token(&s.cai.token)
            .query(&query);
        async move { call.send_bytes().await }
    };
    let piece = read(ThreadItemOutputGetQuery {
        offset: Some(10_000),
        limit: Some(20),
        ..Default::default()
    })
    .await;
    assert_eq!(piece.status, 200);
    assert_eq!(
        std::str::from_utf8(&piece.raw).unwrap(),
        &whole[10_000..10_020],
        "the stretch asked for, from the middle of the whole"
    );
    assert_eq!(
        piece
            .headers
            .get(TOTAL_LENGTH_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        whole.len().to_string(),
        "and the length of the whole"
    );
    let end = read(ThreadItemOutputGetQuery {
        offset: Some(i64::try_from(whole.len()).unwrap() - 5),
        limit: Some(100),
        ..Default::default()
    })
    .await;
    assert_eq!(
        std::str::from_utf8(&end.raw).unwrap(),
        &whole[whole.len() - 5..],
        "a stretch that reaches past the end is cut at it"
    );
    let past = read(ThreadItemOutputGetQuery {
        offset: Some(i64::try_from(whole.len()).unwrap() + 100),
        limit: Some(10),
        ..Default::default()
    })
    .await;
    assert_eq!(past.status, 200);
    assert!(past.raw.is_empty(), "nothing lies past the end");
}

#[tokio::test]
async fn an_output_still_being_printed_that_is_longer_than_the_item_keeps_has_no_whole_to_read_yet()
{
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let item = command_printing(
        &s,
        id,
        json!({ "inline": "the end", "total_len": 90_000, "truncated": true, "blob": null }),
    )
    .await;
    let read = s
        .rig
        .world
        .app
        .call(ThreadItemOutputGet, &[&id, &item])
        .token(&s.cai.token)
        .query(&ThreadItemOutputGetQuery::default())
        .send_bytes()
        .await;
    assert!(!read.status.is_success(), "{}", read.status);
}

#[tokio::test]
async fn the_outline_names_turns_and_messages_once_without_internal_markers_or_duplicate_pins_and_catchup_counts_the_work()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "Plan the fix", "Here is the plan.").await;
    let items = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&id])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            after: Some(0),
            ..ThreadItemListQuery::default()
        })
        .send()
        .await
        .ok()
        .items;
    let reply = items
        .iter()
        .find(|item| item.kind() == ItemKind::AgentMessage)
        .unwrap();
    let pinned = s
        .rig
        .world
        .app
        .call(ThreadItemPin, &[&id, &reply.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        pinned.pinned.as_ref().map(|pin| pin.by.username.as_str()),
        Some("ada")
    );
    let outline = s
        .rig
        .world
        .app
        .call(ThreadOutlineGet, &[&id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    let outline = outline.entries;
    let kinds: Vec<_> = outline.iter().map(|line| line.kind).collect();
    assert!(kinds.contains(&OutlineKind::Turn), "{kinds:?}");
    assert!(
        kinds.iter().all(|kind| matches!(
            kind,
            OutlineKind::Turn
                | OutlineKind::UserMessage
                | OutlineKind::AgentMessage
                | OutlineKind::Milestone
                | OutlineKind::Approval
        )),
        "only conversation points, not internal history: {kinds:?}"
    );
    assert_eq!(
        outline
            .iter()
            .find(|line| line.kind == OutlineKind::Turn)
            .unwrap()
            .turn_ordinal,
        Some(1)
    );
    let said_line = outline
        .iter()
        .find(|line| line.kind == OutlineKind::UserMessage)
        .unwrap();
    assert_eq!(said_line.title, "Plan the fix");
    let reply_line = outline
        .iter()
        .find(|line| line.kind == OutlineKind::AgentMessage)
        .unwrap();
    assert_eq!(reply_line.item_id, reply.id);
    assert_eq!(reply_line.title, "Here is the plan.");
    assert_eq!(
        outline
            .iter()
            .filter(|line| line.item_id == reply.id)
            .count(),
        1,
        "pinning must not duplicate a message"
    );
    assert!(
        outline.windows(2).all(|pair| pair[0].seq <= pair[1].seq),
        "ascending"
    );
    // Removing a pin preserves the conversation point and never adds a second row.
    s.rig
        .world
        .app
        .call(ThreadItemUnpin, &[&id, &reply.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let outline = s
        .rig
        .world
        .app
        .call(ThreadOutlineGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        outline
            .entries
            .iter()
            .all(|line| line.kind != OutlineKind::Pin)
    );

    let caught_up = s
        .rig
        .world
        .app
        .call(ThreadCatchupGet, &[&id])
        .token(&s.cai.token)
        .query(&ThreadCatchupGetQuery {
            since: 0,
            inspect_deleted: None,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(caught_up.items >= 3, "{caught_up:?}");
    assert_eq!(caught_up.turns_completed, 1);
    assert_eq!(caught_up.turns_failed, 0);
    assert_eq!(caught_up.approvals_pending, 0);
    let from_now = s
        .rig
        .world
        .app
        .call(ThreadCatchupGet, &[&id])
        .token(&s.cai.token)
        .query(&ThreadCatchupGetQuery {
            since: caught_up.seq,
            inspect_deleted: None,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!((from_now.items, from_now.turns_completed), (0, 0));
}

#[tokio::test]
async fn how_far_a_person_has_read_only_goes_forward_and_is_their_own() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "one", "first").await;
    let read = |who: &places::Who, seq: i64| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadRead, &[&id])
            .token(&who.token)
            .body(&ThreadReadBody {
                seq,
                ..Default::default()
            });
        async move { call.send().await }
    };
    assert_eq!(read(&s.cai, 4).await.ok().last_read_seq, 4);
    assert_eq!(
        read(&s.cai, 2).await.ok().last_read_seq,
        4,
        "it never goes back"
    );
    assert_eq!(
        read(&s.ada, 1).await.ok().last_read_seq,
        1,
        "and is each person's own"
    );
    let synced = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&id])
        .token(&s.cai.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(synced.thread.my_last_read_seq, Some(4));
}

#[tokio::test]
async fn a_draft_is_kept_replaced_and_thrown_away_and_is_private_to_its_author() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.rig
        .lets_use("provider", s.on.provider, "provider.use", &s.dan, json!({}))
        .await;
    let get = |who: &places::Who| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadDraftGet, &[&id])
            .token(&who.token);
        async move { call.send().await.ok() }
    };
    assert_eq!(get(&s.ada).await, None);
    let set = |who: &places::Who, words: &str, delivery: Delivery| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadDraftSet, &[&id])
            .token(&who.token)
            .body(&ThreadDraftSetBody {
                native_action: None,
                content: text(words),
                delivery,
                review_comments: None,
            });
        async move { call.send().await }
    };
    let draft: Draft = set(&s.ada, "Please also", Delivery::Steer).await.ok();
    assert_eq!(draft.delivery, Delivery::Steer);
    set(&s.ada, "Please also run the tests", Delivery::Queue)
        .await
        .ok();
    let kept = get(&s.ada).await.expect("a draft");
    assert_eq!(kept.content, text("Please also run the tests"));
    assert_eq!(kept.delivery, Delivery::Queue);
    // Somebody else has none of theirs until they write one.
    assert_eq!(get(&s.dan).await, None);
    let gone = s
        .rig
        .world
        .app
        .call(ThreadDraftDelete, &[&id])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    assert_eq!(get(&s.ada).await, None);
}

#[tokio::test]
async fn a_stranger_reads_nothing_and_a_viewer_neither_drives_nor_pins_nor_keeps_a_draft() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "one", "first").await;
    let item = new_id();
    let denied = |problem: orch_api_contract::common::Problem, wanted: &str| {
        assert_eq!(problem.code, ErrorCode::AuthzDenied, "{problem:?}");
        assert_eq!(
            missing_permissions(&problem),
            [wanted.to_owned()],
            "{problem:?}"
        );
    };
    let app = &s.rig.world.app;
    let token = &s.bea.token;
    denied(
        app.call(ThreadSyncGet, &[&id])
            .token(token)
            .query(&ThreadSyncGetQuery::default())
            .send()
            .await
            .problem(),
        "thread.view",
    );
    denied(
        app.call(ThreadItemList, &[&id])
            .token(token)
            .query(&ThreadItemListQuery {
                after: Some(0),
                ..ThreadItemListQuery::default()
            })
            .send()
            .await
            .problem(),
        "thread.view",
    );
    denied(
        app.call(ThreadItemGet, &[&id, &item])
            .token(token)
            .send()
            .await
            .problem(),
        "thread.view",
    );
    denied(
        app.call(ThreadOutlineGet, &[&id])
            .token(token)
            .send()
            .await
            .problem(),
        "thread.view",
    );
    denied(
        app.call(ThreadCatchupGet, &[&id])
            .token(token)
            .query(&ThreadCatchupGetQuery {
                since: 0,
                inspect_deleted: None,
                ..Default::default()
            })
            .send()
            .await
            .problem(),
        "thread.view",
    );
    denied(
        app.call(ThreadRead, &[&id])
            .token(token)
            .body(&ThreadReadBody {
                seq: 1,
                ..Default::default()
            })
            .send()
            .await
            .problem(),
        "thread.view",
    );
    // Cai reads, and driving the thread is another permission.
    let token = &s.cai.token;
    denied(
        app.call(ThreadItemPin, &[&id, &item])
            .token(token)
            .send()
            .await
            .problem(),
        "thread.interact",
    );
    denied(
        app.call(ThreadItemUnpin, &[&id, &item])
            .token(token)
            .send()
            .await
            .problem(),
        "thread.interact",
    );
    denied(
        app.call(ThreadDraftGet, &[&id])
            .token(token)
            .send()
            .await
            .problem(),
        "thread.interact",
    );
    denied(
        app.call(ThreadDraftSet, &[&id])
            .token(token)
            .body(&ThreadDraftSetBody {
                native_action: None,
                content: text("x"),
                delivery: Delivery::Queue,
                review_comments: None,
            })
            .send()
            .await
            .problem(),
        "thread.interact",
    );
    denied(
        app.call(ThreadDraftDelete, &[&id])
            .token(token)
            .send()
            .await
            .problem(),
        "thread.interact",
    );
}
