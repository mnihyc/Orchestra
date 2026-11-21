//! Bounded history windows retain every older anchor and the exact turn/segment references it needs.

mod places;
mod rig;
mod support;
mod thread_history_scope;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{
    OutlineKind, THREAD_SYNC_DEFAULT_LIMIT, THREAD_SYNC_ENDED_TURNS, ThreadHistoryListQuery,
    ThreadItemList, ThreadItemListQuery, ThreadOpeningGet, ThreadOpeningGetQuery, ThreadOutlineGet,
    ThreadOutlineGetQuery, ThreadSegmentList, ThreadSyncGet, ThreadSyncGetQuery, ThreadTurnList,
};
use orch_db::queries::threads::{self, InputChange, NewInput, NewItem, NewSegment, NewTurn, Place};
use orch_types::{Id, Seq, Time, new_id};
use rig::Scene;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const SEGMENTS: usize = 24; // More than the retained twenty-ended-turn initial window.
const MESSAGES: usize = 12; // Together exceed the default two-hundred-point outline page.

struct Anchor {
    item: Id,
    seq: Seq,
    turn: Id,
    segment: Id,
}

struct History {
    scene: Scene,
    thread: Id,
    anchors: Vec<Anchor>,
    empty_turns: BTreeSet<Id>,
    points: BTreeSet<Id>,
}

async fn seed() -> Proof<History> {
    seed_history(SEGMENTS, MESSAGES).await
}

async fn seed_history(segments: usize, messages: usize) -> Proof<History> {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    // Synthetic historical rows use native persistence helpers in one committed fixture, with
    // no queued work or real harness rounds. The separate native-link stress suite owns those.
    let mut tx = scene.rig.db.begin().await?;
    let at = scene.rig.world.now();
    // Binding the real thread already made segment 1; it is part of this history,
    // not an extra empty segment ahead of the twenty-four the fixture promises.
    let first_segment = threads::thread(&mut tx, thread)
        .await?
        .ok_or("thread absent")?
        .current_segment_id
        .ok_or("initial bound segment absent")?;
    let mut anchors = Vec::new();
    let mut empty_turns = BTreeSet::new();
    let mut points = BTreeSet::new();
    let params = json!({});
    let mut last_segment = None;
    for ordinal in 1..=segments {
        let segment = if ordinal == 1 {
            first_segment
        } else {
            let segment = new_id();
            let seq = orch_db::sequences::next(&mut tx, thread)
                .await?
                .ok_or("thread absent")?;
            threads::insert_segment(
                &mut tx,
                &NewSegment {
                    id: segment,
                    thread_id: thread,
                    agent_id: scene.on.agent,
                    provider_id: scene.on.provider,
                    environment_id: scene.on.environment,
                    env_generation: 1,
                    arrangement: "separated",
                    executor: Some("orch-exec"),
                    tool_set: Some("plain"),
                    credential_path: "native",
                    native_session_ref: None,
                    started_by: Some(scene.ada.id()),
                    started_at: at,
                },
                seq,
            )
            .await?;
            segment
        };
        let input = new_id();
        let content = json!([{"type":"text","text":format!("Historical question {ordinal}")}]);
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("thread absent")?;
        threads::insert_input(
            &mut tx,
            &NewInput {
                plan: false,
                id: input,
                thread_id: thread,
                author_id: Some(scene.ada.id()),
                submission_authority: Some(
                    &serde_json::json!({"credentials":[{"type":"session","as_admin":false}]}),
                ),
                run_as: None,
                run_as_authority: None,
                source: "user",
                content: &content,
                delivery: "queue",
                state: "pending",
                turn_id: None,
                client_id: None,
                limit_bypass: None,
                place: Place::End,
                at,
            },
            seq,
        )
        .await?
        .ok_or("input absent")?;
        let turn = new_id();
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("thread absent")?;
        threads::insert_turn(
            &mut tx,
            &NewTurn {
                id: turn,
                thread_id: thread,
                segment_id: segment,
                input_id: input,
                model: rig::MODEL,
                effort: Some("low"),
                params: &params,
                lease_id: None,
                limit_bypass: None,
                at,
            },
            seq,
        )
        .await?;
        threads::change_input(
            &mut tx,
            input,
            &["pending"],
            &InputChange {
                state: "delivered",
                turn_id: Some(turn),
                delivered_at: Some(at),
                ..Default::default()
            },
            seq,
        )
        .await?;
        if ordinal % 7 == 0 {
            empty_turns.insert(turn);
        } else {
            if ordinal % 2 == 0 {
                let command = new_id();
                let output = "large-command-body-must-not-enter-outline".repeat(256);
                let payload = json!({"title":"Inspect historical output","started_at":Time::new(at),"duration_ms":1,
                    "tool":{"kind":"command","command":"synthetic-read","cwd":"/workspace","exit_code":0,
                        "output":{"inline":output,"total_len":output.len(),"truncated":false,"blob":null},
                        "background":false,"process_id":null,"actions":[]},
                    "raw":{"name":"SyntheticShell","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},
                        "output":null,"is_error":false}});
                let seq = orch_db::sequences::next(&mut tx, thread)
                    .await?
                    .ok_or("thread absent")?;
                threads::insert_item(
                    &mut tx,
                    &NewItem {
                        id: command,
                        thread_id: thread,
                        segment_id: segment,
                        turn_id: Some(turn),
                        stream: "main",
                        parent_item_id: None,
                        kind: "tool_call",
                        status: "completed",
                        author_id: None,
                        payload: &payload,
                        native_ref: None,
                        search_text: None,
                        at,
                    },
                    seq,
                )
                .await?
                .ok_or("command absent")?;
                points.insert(command); // First non-marker item remains the numbered turn's anchor.
            }
            for message in 0..messages {
                let item = new_id();
                let text = format!(" \n\n Historical answer {ordinal}-{message}\nExtra details");
                let payload = json!({"text":text,"text_len":text.len()});
                let seq = orch_db::sequences::next(&mut tx, thread)
                    .await?
                    .ok_or("thread absent")?;
                threads::insert_item(
                    &mut tx,
                    &NewItem {
                        id: item,
                        thread_id: thread,
                        segment_id: segment,
                        turn_id: Some(turn),
                        stream: "main",
                        parent_item_id: None,
                        kind: "agent_message",
                        status: "completed",
                        author_id: None,
                        payload: &payload,
                        native_ref: None,
                        search_text: None,
                        at,
                    },
                    seq,
                )
                .await?
                .ok_or("message absent")?;
                anchors.push(Anchor {
                    item,
                    seq,
                    turn,
                    segment,
                });
                points.insert(item);
            }
        }
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("thread absent")?;
        assert!(threads::finish_turn(&mut tx, turn, "completed", None, at, seq).await?);
        if ordinal < segments {
            assert!(threads::end_segment(&mut tx, segment, "switched", at, seq).await?);
        }
        last_segment = Some(segment);
    }
    let seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread absent")?;
    threads::set_current(&mut tx, thread, last_segment, None, seq).await?;
    tx.commit().await?;
    Ok(History {
        scene,
        thread,
        anchors,
        empty_turns,
        points,
    })
}

// Two segments and one message per turn are sufficient to force a second metadata page.
// The existing twenty-four-segment controls above keep their original scale and constants.
async fn scoped_read<E>(endpoint: E) -> Proof
where
    E: orch_api_contract::routes::Endpoint<Query = ThreadHistoryListQuery, Body = ()> + Copy,
{
    use thread_history_scope::{key, ok, request, rule};
    let history = seed_history(2, 1).await?;
    let world = &history.scene.rig.world;
    let operation = format!("{}.read", E::ROUTE.name);
    let selected = key(
        world,
        history.scene.cai.id(),
        vec![rule(&operation, "thread", history.thread, "self", "read")],
    )
    .await?;
    let query = ThreadHistoryListQuery {
        limit: Some(1),
        ..Default::default()
    };
    let browser = ok(&request(
        world,
        endpoint,
        history.thread,
        &history.scene.cai.token,
        &query,
    )
    .await)?;
    let scoped = ok(&request(world, endpoint, history.thread, &selected.secret, &query).await)?;
    assert_eq!(
        scoped, browser,
        "the key must use the ordinary history projection"
    );
    let segments = scoped["segments"].as_array().ok_or("segments absent")?;
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0]["agent"]["id"], json!(history.scene.on.agent));
    assert_eq!(
        segments[0]["provider"]["id"],
        json!(history.scene.on.provider)
    );
    assert_eq!(
        segments[0]["environment"]["id"],
        json!(history.scene.on.environment)
    );
    assert_eq!(segments[0]["ordinal"], 2);
    if E::ROUTE.name == "ThreadTurnList" {
        let turns = scoped["turns"].as_array().ok_or("turns absent")?;
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["segment_id"], segments[0]["id"]);
        assert_eq!(turns[0]["ordinal"], 2);
    }
    let before = serde_json::from_value(scoped["next_before"].clone())?;
    let older = ok(&request(
        world,
        endpoint,
        history.thread,
        &selected.secret,
        &ThreadHistoryListQuery {
            before,
            limit: Some(1),
            ..Default::default()
        },
    )
    .await)?;
    assert_eq!(older["segments"][0]["ordinal"], 1);
    assert!(older["next_before"].is_null());
    // Intrinsic history references give no direct permission over those other resources.
    for reply in [
        world
            .app
            .call(
                orch_api_contract::environment::EnvironmentGet,
                &[&history.scene.on.environment],
            )
            .token(&selected.secret)
            .send_bytes()
            .await,
        world
            .app
            .call(
                orch_api_contract::resources::ProviderGet,
                &[&history.scene.on.provider],
            )
            .token(&selected.secret)
            .send_bytes()
            .await,
    ] {
        thread_history_scope::denied(&reply);
    }
    Ok(())
}

async fn scoped_inspection<E>(endpoint: E) -> Proof
where
    E: orch_api_contract::routes::Endpoint<Query = ThreadHistoryListQuery, Body = ()> + Copy,
{
    use thread_history_scope::{denied, key, ok, request, rule};
    let history = seed_history(2, 1).await?;
    let world = &history.scene.rig.world;
    world
        .app
        .call(orch_api_contract::ownership::NodeDelete, &[&history.thread])
        .token(&history.scene.ada.token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let query = ThreadHistoryListQuery {
        inspect_deleted: Some(true),
        ..Default::default()
    };
    let inspect = format!("{}.inspect_deleted", E::ROUTE.name);
    let selected = key(
        world,
        history.scene.ada.id(),
        vec![rule(&inspect, "thread", history.thread, "self", "manage")],
    )
    .await?;
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM items WHERE thread_id=$1),(SELECT count(*) FROM turns WHERE thread_id=$1),(SELECT count(*) FROM thread_inputs WHERE thread_id=$1)")
        .bind(history.thread).fetch_one(&world.db.pool).await?;
    let browser = ok(&request(
        world,
        endpoint,
        history.thread,
        &history.scene.ada.token,
        &query,
    )
    .await)?;
    let scoped_reply = request(world, endpoint, history.thread, &selected.secret, &query).await;
    let scoped = ok(&scoped_reply)?;
    assert_eq!(scoped, browser);
    assert_eq!(
        scoped["segments"]
            .as_array()
            .ok_or("segments absent")?
            .len(),
        2
    );
    // Metadata pages preserve their payload shape and carry inspection in the existing header.
    let inspection: serde_json::Value = serde_json::from_str(
        scoped_reply
            .headers
            .get("orch-inspection")
            .ok_or("inspection header absent")?
            .to_str()?,
    )?;
    assert_eq!(inspection["readonly"], true);
    assert_eq!(
        scoped_reply
            .headers
            .get(http::header::CACHE_CONTROL)
            .ok_or("cache control absent")?,
        "no-store"
    );
    let after: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM items WHERE thread_id=$1),(SELECT count(*) FROM turns WHERE thread_id=$1),(SELECT count(*) FROM thread_inputs WHERE thread_id=$1)")
        .bind(history.thread).fetch_one(&world.db.pool).await?;
    assert_eq!(
        after, counts,
        "inspection must not submit or change history"
    );
    let reads: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1",
    )
    .bind(history.thread)
    .fetch_one(&world.db.pool)
    .await?;
    assert_eq!(reads, 1, "ordinary daily inspection audit remains shared");
    let read = key(
        world,
        history.scene.ada.id(),
        vec![rule(
            &format!("{}.read", E::ROUTE.name),
            "thread",
            history.thread,
            "self",
            "read",
        )],
    )
    .await?;
    denied(&request(world, endpoint, history.thread, &read.secret, &query).await);
    let viewer = key(
        world,
        history.scene.cai.id(),
        vec![rule(&inspect, "thread", history.thread, "self", "manage")],
    )
    .await?;
    denied(&request(world, endpoint, history.thread, &viewer.secret, &query).await);
    // A Manage inspection selection cannot stand in for the distinct ordinary Read variant.
    denied(
        &request(
            world,
            endpoint,
            history.thread,
            &selected.secret,
            &ThreadHistoryListQuery::default(),
        )
        .await,
    );
    Ok(())
}

async fn scoped_bounds<E>(endpoint: E) -> Proof
where
    E: orch_api_contract::routes::Endpoint<Query = ThreadHistoryListQuery, Body = ()> + Copy,
{
    use thread_history_scope::{denied, key, request, rule};
    let history = seed_history(2, 1).await?;
    let world = &history.scene.rig.world;
    let operation = format!("{}.read", E::ROUTE.name);
    let sibling = history
        .scene
        .thread_by(&history.scene.ada, "Unrelated history")
        .await
        .base
        .id;
    for selected_rule in [
        rule(
            "ThreadOutlineGet.read",
            "thread",
            history.thread,
            "self",
            "read",
        ),
        rule(&operation, "thread", history.thread, "self", "write"),
        rule(&operation, "thread", sibling, "self", "read"),
        rule(&operation, "document", history.thread, "self", "read"),
    ] {
        let selected = key(world, history.scene.ada.id(), vec![selected_rule]).await?;
        denied(
            &request(
                world,
                endpoint,
                history.thread,
                &selected.secret,
                &ThreadHistoryListQuery::default(),
            )
            .await,
        );
    }
    let powerless = key(
        world,
        history.scene.bea.id(),
        vec![rule(&operation, "thread", history.thread, "self", "read")],
    )
    .await?;
    let reply = request(
        world,
        endpoint,
        history.thread,
        &powerless.secret,
        &ThreadHistoryListQuery::default(),
    )
    .await;
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    let selected = key(
        world,
        history.scene.ada.id(),
        vec![rule(&operation, "thread", history.thread, "self", "read")],
    )
    .await?;
    // Another thread's continuation is not usable through this admitted thread.
    let foreign: Id =
        sqlx::query_scalar("SELECT active_continuation_id FROM threads WHERE node_id=$1")
            .bind(sibling)
            .fetch_one(&world.db.pool)
            .await?;
    let foreign = request(
        world,
        endpoint,
        history.thread,
        &selected.secret,
        &ThreadHistoryListQuery {
            continuation_id: Some(foreign),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(foreign.problem().code, ErrorCode::NotFound);
    Ok(())
}

async fn scoped_current<E>(endpoint: E) -> Proof
where
    E: orch_api_contract::routes::Endpoint<Query = ThreadHistoryListQuery, Body = ()> + Copy,
{
    use thread_history_scope::{denied, key, narrow, ok, request, rule};
    let history = seed_history(2, 1).await?;
    let world = &history.scene.rig.world;
    let operation = format!("{}.read", E::ROUTE.name);
    let original = vec![rule(&operation, "thread", history.thread, "self", "read")];
    let selected = key(world, history.scene.ada.id(), original.clone()).await?;
    let page = ok(&request(
        world,
        endpoint,
        history.thread,
        &selected.secret,
        &ThreadHistoryListQuery {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await)?;
    let query = ThreadHistoryListQuery {
        before: serde_json::from_value(page["next_before"].clone())?,
        limit: Some(1),
        ..Default::default()
    };
    narrow(
        world,
        &selected,
        vec![rule(
            "ThreadOutlineGet.read",
            "thread",
            history.thread,
            "self",
            "read",
        )],
    )
    .await?;
    denied(&request(world, endpoint, history.thread, &selected.secret, &query).await);
    for revoked in [false, true] {
        let selected = key(world, history.scene.ada.id(), original.clone()).await?;
        if revoked {
            assert!(
                orch_db::queries::identity::revoke_api_token(
                    &mut *world.conn().await,
                    selected.id,
                    history.scene.ada.id(),
                    world.now()
                )
                .await?
            );
        } else {
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(selected.id)
                .bind(world.now())
                .execute(&world.db.pool)
                .await?;
        }
        assert_eq!(
            request(world, endpoint, history.thread, &selected.secret, &query)
                .await
                .status,
            http::StatusCode::UNAUTHORIZED
        );
    }
    let source = key(
        world,
        history.scene.ada.id(),
        vec![rule(
            &operation,
            "topic",
            history.scene.tree.topic.base.id,
            "subtree",
            "read",
        )],
    )
    .await?;
    ok(&request(world, endpoint, history.thread, &source.secret, &query).await)?;
    let destination = places::make_topic(
        world,
        &history.scene.ada,
        history.scene.tree.project.base.id,
        "Moved history",
    )
    .await;
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::move_under(
            &mut conn,
            history.thread,
            destination.base.id,
            world.now(),
        )
        .await?;
        orch_authz::epoch::bump(&mut conn).await?;
    }
    denied(&request(world, endpoint, history.thread, &source.secret, &query).await);
    let exact = key(world, history.scene.ada.id(), original).await?;
    ok(&request(world, endpoint, history.thread, &exact.secret, &query).await)?;
    Ok(())
}

macro_rules! scoped_history_controls {
    ($read:ident, $inspect:ident, $bounds:ident, $current:ident, $endpoint:expr) => {
        #[tokio::test]
        async fn $read() -> Proof {
            scoped_read($endpoint).await
        }
        #[tokio::test]
        async fn $inspect() -> Proof {
            scoped_inspection($endpoint).await
        }
        #[tokio::test]
        async fn $bounds() -> Proof {
            scoped_bounds($endpoint).await
        }
        #[tokio::test]
        async fn $current() -> Proof {
            scoped_current($endpoint).await
        }
    };
}

scoped_history_controls!(
    scoped_segment_read_matches_browser_and_keeps_exact_references,
    scoped_segment_inspection_uses_current_restoration_authority,
    scoped_segment_read_refuses_other_operations_targets_and_accounts,
    scoped_segment_pagination_rechecks_current_key_and_placement,
    ThreadSegmentList
);
scoped_history_controls!(
    scoped_turn_read_matches_browser_and_keeps_exact_references,
    scoped_turn_inspection_uses_current_restoration_authority,
    scoped_turn_read_refuses_other_operations_targets_and_accounts,
    scoped_turn_pagination_rechecks_current_key_and_placement,
    ThreadTurnList
);

#[tokio::test]
async fn opening_is_a_window_and_old_item_pages_supply_their_own_historical_references() -> Proof {
    let history = seed().await?;
    let app = &history.scene.rig.world.app;
    let opening = app
        .call(ThreadOpeningGet, &[&history.thread])
        .token(&history.scene.ada.token)
        .query(&ThreadOpeningGetQuery {
            limit: Some(5),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(serde_json::to_value(&opening)?.get("outline").is_none());
    let outline = app
        .call(ThreadOutlineGet, &[&history.thread])
        .token(&history.scene.ada.token)
        .query(&ThreadOutlineGetQuery {
            limit: Some(5),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(outline.seq >= opening.sync.seq);
    assert!(opening.sync.items.len() <= 10); // Existing union: five overall plus five main.
    assert_eq!(
        opening.sync.turns.len(),
        usize::try_from(THREAD_SYNC_ENDED_TURNS)?
    );
    assert_eq!(opening.sync.segments.len(), opening.sync.turns.len());
    assert!(outline.next_before.is_some());
    assert!(
        outline
            .entries
            .iter()
            .map(|entry| entry.item_id)
            .collect::<BTreeSet<_>>()
            .len()
            <= 5
    );
    let oldest = history.anchors.first().ok_or("old anchor absent")?;
    assert!(!opening.sync.turns.iter().any(|turn| turn.id == oldest.turn));
    assert!(
        !opening
            .sync
            .segments
            .iter()
            .any(|segment| segment.id == oldest.segment)
    );
    let page = app
        .call(ThreadItemList, &[&history.thread])
        .token(&history.scene.cai.token)
        .query(&ThreadItemListQuery {
            around: Some(oldest.item),
            limit: Some(3),
            stream: Some("main".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(page.items.iter().any(|item| item.id == oldest.item));
    assert!(
        page.turns
            .iter()
            .any(|turn| turn.id == oldest.turn && turn.ordinal == 1)
    );
    assert!(
        page.segments
            .iter()
            .any(|segment| segment.id == oldest.segment && segment.ordinal == 1)
    );
    assert!(page.items.iter().all(|item| {
        page.segments
            .iter()
            .any(|segment| segment.id == item.segment_id)
    }));
    assert!(
        page.items
            .iter()
            .filter_map(|item| item.turn_id)
            .all(|id| page.turns.iter().any(|turn| turn.id == id))
    );
    let mut retained: BTreeMap<_, _> = opening
        .sync
        .turns
        .iter()
        .map(|turn| (turn.id, turn.clone()))
        .collect();
    retained.extend(page.turns.into_iter().map(|turn| (turn.id, turn)));
    assert!(retained.contains_key(&oldest.turn));
    assert!(
        opening
            .sync
            .turns
            .iter()
            .all(|turn| retained.contains_key(&turn.id))
    );
    Ok(())
}

#[tokio::test]
async fn outline_continuations_and_anchors_reach_every_point_without_payload_or_turn_header_duplication()
-> Proof {
    let history = seed().await?;
    let app = &history.scene.rig.world.app;
    let latest = app
        .call(ThreadOutlineGet, &[&history.thread])
        .token(&history.scene.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(
        latest
            .entries
            .iter()
            .map(|entry| entry.item_id)
            .collect::<BTreeSet<_>>()
            .len(),
        usize::try_from(THREAD_SYNC_DEFAULT_LIMIT)?
    );
    assert!(latest.next_before.is_some());
    let mut before = None;
    let mut seen = BTreeSet::new();
    let mut turn_headers = BTreeSet::new();
    let mut messages = BTreeSet::new();
    loop {
        let page = app
            .call(ThreadOutlineGet, &[&history.thread])
            .token(&history.scene.cai.token)
            .query(&ThreadOutlineGetQuery {
                before,
                limit: Some(11),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        let ids: BTreeSet<_> = page.entries.iter().map(|entry| entry.item_id).collect();
        assert!(ids.len() <= 11);
        assert!(
            page.entries
                .windows(2)
                .all(|pair| pair[0].seq <= pair[1].seq)
        );
        for id in ids {
            assert!(
                seen.insert(id),
                "a source point must never straddle two pages"
            );
        }
        for entry in &page.entries {
            assert!(before.is_none_or(|cursor| entry.seq < cursor));
            assert!(entry.title.chars().count() <= 80);
            assert!(!entry.title.contains("large-command-body"));
            if entry.kind == OutlineKind::Turn {
                assert!(turn_headers.insert(entry.turn_ordinal.ok_or("turn ordinal absent")?));
            } else {
                assert_eq!(
                    entry.kind,
                    OutlineKind::AgentMessage,
                    "a command is not an announced milestone"
                );
                assert!(messages.insert(entry.item_id));
                assert!(
                    entry.title.starts_with("Historical answer "),
                    "leading blank lines do not erase the native title"
                );
            }
        }
        before = page.next_before;
        if before.is_none() {
            break;
        }
    }
    assert_eq!(seen, history.points);
    assert_eq!(
        messages,
        history
            .anchors
            .iter()
            .map(|anchor| anchor.item)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(turn_headers.len(), SEGMENTS - history.empty_turns.len());
    let oldest = history.anchors.first().ok_or("old anchor absent")?;
    for query in [
        ThreadOutlineGetQuery {
            around: Some(oldest.item),
            limit: Some(7),
            ..Default::default()
        },
        ThreadOutlineGetQuery {
            turn: Some(1),
            limit: Some(7),
            ..Default::default()
        },
    ] {
        let page = app
            .call(ThreadOutlineGet, &[&history.thread])
            .token(&history.scene.cai.token)
            .query(&query)
            .send()
            .await
            .ok();
        assert!(
            page.entries
                .iter()
                .any(|entry| entry.item_id == oldest.item)
        );
        assert!(page.entries.iter().any(|entry| entry.seq <= oldest.seq));
    }
    Ok(())
}

#[tokio::test]
async fn explicit_metadata_pages_include_empty_turns_and_all_older_segments_without_broadening_access()
-> Proof {
    let history = seed().await?;
    let app = &history.scene.rig.world.app;
    let mut before = None;
    let mut seen_turns = BTreeSet::new();
    loop {
        let page = app
            .call(ThreadTurnList, &[&history.thread])
            .token(&history.scene.cai.token)
            .query(&ThreadHistoryListQuery {
                before,
                limit: Some(5),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(page.turns.len() <= 5);
        assert!(
            page.turns
                .windows(2)
                .all(|pair| pair[0].ordinal < pair[1].ordinal)
        );
        for turn in &page.turns {
            assert!(seen_turns.insert(turn.id));
            assert!(
                page.segments
                    .iter()
                    .any(|segment| segment.id == turn.segment_id)
            );
            assert!(turn.seq_updated <= page.seq);
        }
        before = page.next_before;
        if before.is_none() {
            break;
        }
    }
    assert_eq!(seen_turns.len(), SEGMENTS);
    assert!(history.empty_turns.is_subset(&seen_turns));
    before = None;
    let mut seen_segments = BTreeSet::new();
    loop {
        let page = app
            .call(ThreadSegmentList, &[&history.thread])
            .token(&history.scene.cai.token)
            .query(&ThreadHistoryListQuery {
                before,
                limit: Some(5),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(page.segments.len() <= 5);
        assert!(
            page.segments
                .windows(2)
                .all(|pair| pair[0].ordinal < pair[1].ordinal)
        );
        for segment in &page.segments {
            assert!(seen_segments.insert(segment.id));
        }
        before = page.next_before;
        if before.is_none() {
            break;
        }
    }
    assert_eq!(seen_segments.len(), SEGMENTS);
    assert_eq!(
        app.call(ThreadTurnList, &[&history.thread])
            .token(&history.scene.bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        app.call(ThreadSegmentList, &[&history.thread])
            .token(&history.scene.bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        app.call(ThreadOutlineGet, &[&history.thread])
            .token(&history.scene.bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    for query in [
        ThreadHistoryListQuery {
            before: Some(0),
            ..Default::default()
        },
        ThreadHistoryListQuery {
            limit: Some(0),
            ..Default::default()
        },
    ] {
        assert_eq!(
            app.call(ThreadTurnList, &[&history.thread])
                .token(&history.scene.ada.token)
                .query(&query)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::RequestInvalid
        );
    }
    let sync = app
        .call(ThreadSyncGet, &[&history.thread])
        .token(&history.scene.ada.token)
        .query(&ThreadSyncGetQuery {
            since: Some(i64::MAX),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        !sync.complete,
        "a cursor beyond this cut cannot claim a complete incremental read"
    );
    Ok(())
}

// Seven content readers share real history fixtures and retain each endpoint's native wire shape.
#[derive(Clone, Copy)]
enum ContentReadCase {
    Sync,
    Items,
    Item,
    Output,
    Outline,
    Catchup,
    Blob,
}

impl ContentReadCase {
    fn operation(self) -> &'static str {
        use orch_api_contract::routes::Endpoint as _;
        use orch_api_contract::threads::*;
        match self {
            Self::Sync => ThreadSyncGet::ROUTE.name,
            Self::Items => ThreadItemList::ROUTE.name,
            Self::Item => ThreadItemGet::ROUTE.name,
            Self::Output => ThreadItemOutputGet::ROUTE.name,
            Self::Outline => ThreadOutlineGet::ROUTE.name,
            Self::Catchup => ThreadCatchupGet::ROUTE.name,
            Self::Blob => ThreadBlobGet::ROUTE.name,
        }
    }

    async fn request(
        self,
        history: &History,
        token: &str,
        inspect: bool,
        foreign_thread: Option<Id>,
        range: bool,
    ) -> Proof<orch_api_core::testing::RawReply> {
        use orch_api_contract::threads::*;
        let world = &history.scene.rig.world;
        let thread = foreign_thread.unwrap_or(history.thread);
        let item = history
            .anchors
            .first()
            .ok_or("historical message absent")?
            .item;
        let command: Id = sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind='tool_call' ORDER BY seq_created LIMIT 1")
            .bind(history.thread).fetch_one(&world.db.pool).await?;
        let revision: i64 =
            sqlx::query_scalar("SELECT execution_revision FROM threads WHERE node_id=$1")
                .bind(history.thread)
                .fetch_one(&world.db.pool)
                .await?;
        let base = format!("inspect_deleted={inspect}&projection_revision={revision}");
        let app = &world.app;
        let reply = match self {
            Self::Sync => {
                app.call(ThreadSyncGet, &[&thread])
                    .token(token)
                    .raw_query(&base)
                    .send_bytes()
                    .await
            }
            Self::Items => {
                app.call(ThreadItemList, &[&thread])
                    .token(token)
                    .raw_query(&format!("{base}&around={item}"))
                    .send_bytes()
                    .await
            }
            Self::Item => {
                app.call(ThreadItemGet, &[&thread, &item])
                    .token(token)
                    .raw_query(&base)
                    .send_bytes()
                    .await
            }
            Self::Output => {
                app.call(ThreadItemOutputGet, &[&thread, &command])
                    .token(token)
                    .raw_query(&format!("{base}&offset=2&limit=17"))
                    .send_bytes()
                    .await
            }
            Self::Outline => {
                app.call(ThreadOutlineGet, &[&thread])
                    .token(token)
                    .raw_query(&base)
                    .send_bytes()
                    .await
            }
            Self::Catchup => {
                app.call(ThreadCatchupGet, &[&thread])
                    .token(token)
                    .raw_query(&format!("{base}&since=0"))
                    .send_bytes()
                    .await
            }
            Self::Blob => {
                let hash = orch_types::Hash::of(CONTENT_SCOPE_BLOB);
                let call = app
                    .call(ThreadBlobGet, &[&thread, &hash])
                    .token(token)
                    .raw_query(&format!("inspect_deleted={inspect}"));
                if range {
                    call.header("range", "bytes=2-7").send_bytes().await
                } else {
                    call.send_bytes().await
                }
            }
        };
        Ok(reply)
    }
}

const CONTENT_SCOPE_BLOB: &[u8] = b"Scoped thread image original body";

async fn content_scope_fixture() -> Proof<History> {
    use orch_api_contract::threads::{ContentPart, ImageMediaType, UploadCreate};
    let history = seed_history(2, 1).await?;
    let world = &history.scene.rig.world;
    // The existing image uploader accepts the declared media type; this cohort reads original
    // bytes, without image decoding. Preview format correctness has its own upload controls.
    let stored = world
        .app
        .call(UploadCreate, &[])
        .token(&history.scene.ada.token)
        .header("content-type", "image/png")
        .bytes(CONTENT_SCOPE_BLOB)
        .send()
        .await
        .ok();
    let parts = vec![ContentPart::Image {
        blob: stored.blob,
        media_type: ImageMediaType::Png,
        name: "scoped-history.png".into(),
        path: None,
    }];
    let changed = sqlx::query("UPDATE thread_inputs SET content=$2 WHERE id=(SELECT input_id FROM turns WHERE thread_id=$1 ORDER BY ordinal LIMIT 1)")
        .bind(history.thread).bind(serde_json::to_value(parts)?).execute(&world.db.pool).await?;
    assert_eq!(changed.rows_affected(), 1);
    Ok(history)
}

async fn scoped_content_positive(case: ContentReadCase, inspect: bool) -> Proof {
    use thread_history_scope::{denied, key, rule};
    let history = content_scope_fixture().await?;
    let world = &history.scene.rig.world;
    let sibling = history
        .scene
        .thread_by(&history.scene.ada, "Scoped content sibling")
        .await
        .base
        .id;
    if inspect {
        world
            .app
            .call(orch_api_contract::ownership::NodeDelete, &[&history.thread])
            .token(&history.scene.ada.token)
            .raw_body(&json!({}))
            .send()
            .await
            .ok();
    }
    let action = if inspect { "manage" } else { "read" };
    let variant = if inspect { "inspect_deleted" } else { "read" };
    let operation = format!("{}.{variant}", case.operation());
    let user = if inspect {
        &history.scene.ada
    } else {
        &history.scene.cai
    };
    let selected = key(
        world,
        user.id(),
        vec![rule(&operation, "thread", history.thread, "self", action)],
    )
    .await?;
    let ordinary = case
        .request(&history, &user.token, inspect, None, false)
        .await?;
    assert_eq!(
        ordinary.status,
        http::StatusCode::OK,
        "ordinary {} premise: {}",
        operation,
        String::from_utf8_lossy(&ordinary.raw)
    );
    match case {
        ContentReadCase::Blob => assert_eq!(ordinary.raw.as_ref(), CONTENT_SCOPE_BLOB),
        ContentReadCase::Output => {
            assert_eq!(
                ordinary.raw.as_ref(),
                &b"large-command-body-must-not-enter-outline".repeat(256)[2..19]
            );
            assert!(
                ordinary
                    .headers
                    .contains_key(orch_api_contract::threads::TOTAL_LENGTH_HEADER)
            );
        }
        ContentReadCase::Catchup => {
            let body: serde_json::Value = serde_json::from_slice(&ordinary.raw)?;
            assert!(body["items"].as_i64().ok_or("catchup item count absent")? >= 2);
        }
        _ => assert!(
            String::from_utf8_lossy(&ordinary.raw).contains("Historical answer"),
            "actual historical content premise absent"
        ),
    }
    if inspect {
        let banner: serde_json::Value = serde_json::from_str(
            ordinary
                .headers
                .get("orch-inspection")
                .ok_or("inspection header absent")?
                .to_str()?,
        )?;
        assert_eq!(banner["readonly"], true);
    }
    let scoped = case
        .request(&history, &selected.secret, inspect, None, false)
        .await?;
    assert_eq!(
        scoped.status,
        ordinary.status,
        "{} actual key: {}",
        operation,
        String::from_utf8_lossy(&scoped.raw)
    );
    assert_eq!(
        scoped.raw, ordinary.raw,
        "the same producer must preserve {} output",
        operation
    );
    for header in [
        http::header::CONTENT_TYPE.as_str(),
        orch_api_contract::threads::TOTAL_LENGTH_HEADER,
        "etag",
        "cache-control",
        "content-disposition",
        "orch-inspection",
    ] {
        assert_eq!(
            scoped.headers.get(header),
            ordinary.headers.get(header),
            "same {} header {header}",
            operation
        );
    }
    if matches!(case, ContentReadCase::Blob) {
        let partial = case
            .request(&history, &selected.secret, inspect, None, true)
            .await?;
        assert_eq!(partial.status, http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.raw.as_ref(), &CONTENT_SCOPE_BLOB[2..8]);
        assert!(partial.headers.contains_key(http::header::CONTENT_RANGE));
    }
    let wrong = key(
        world,
        user.id(),
        vec![rule(
            "ThreadRawList",
            "thread",
            history.thread,
            "self",
            "manage",
        )],
    )
    .await?;
    denied(
        &case
            .request(&history, &wrong.secret, inspect, None, false)
            .await?,
    );
    let wrong_action = key(
        world,
        user.id(),
        vec![rule(
            &operation,
            "thread",
            history.thread,
            "self",
            if inspect { "read" } else { "write" },
        )],
    )
    .await?;
    denied(
        &case
            .request(&history, &wrong_action.secret, inspect, None, false)
            .await?,
    );
    let wrong_target = key(
        world,
        user.id(),
        vec![rule(&operation, "thread", sibling, "self", action)],
    )
    .await?;
    denied(
        &case
            .request(&history, &wrong_target.secret, inspect, None, false)
            .await?,
    );
    let wrong_kind = key(
        world,
        user.id(),
        vec![rule(&operation, "document", history.thread, "self", action)],
    )
    .await?;
    denied(
        &case
            .request(&history, &wrong_kind.secret, inspect, None, false)
            .await?,
    );
    let powerless = key(
        world,
        if inspect {
            history.scene.cai.id()
        } else {
            history.scene.bea.id()
        },
        vec![rule(&operation, "thread", history.thread, "self", action)],
    )
    .await?;
    let reply = case
        .request(&history, &powerless.secret, inspect, None, false)
        .await?;
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    Ok(())
}

macro_rules! content_scope_controls {
    ($read:ident, $inspect:ident, $case:expr) => {
        #[tokio::test]
        async fn $read() -> Proof {
            scoped_content_positive($case, false).await
        }
        #[tokio::test]
        async fn $inspect() -> Proof {
            scoped_content_positive($case, true).await
        }
    };
}
content_scope_controls!(
    scoped_content_sync_read,
    scoped_content_sync_inspect,
    ContentReadCase::Sync
);
content_scope_controls!(
    scoped_content_items_read,
    scoped_content_items_inspect,
    ContentReadCase::Items
);
content_scope_controls!(
    scoped_content_item_read,
    scoped_content_item_inspect,
    ContentReadCase::Item
);
content_scope_controls!(
    scoped_content_output_read,
    scoped_content_output_inspect,
    ContentReadCase::Output
);
content_scope_controls!(
    scoped_content_outline_read,
    scoped_content_outline_inspect,
    ContentReadCase::Outline
);
content_scope_controls!(
    scoped_content_catchup_read,
    scoped_content_catchup_inspect,
    ContentReadCase::Catchup
);
content_scope_controls!(
    scoped_content_blob_read,
    scoped_content_blob_inspect,
    ContentReadCase::Blob
);
