//! Bounded history windows retain every older anchor and the exact turn/segment references it needs.

mod places;
mod rig;
mod support;

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
    for ordinal in 1..=SEGMENTS {
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
            for message in 0..MESSAGES {
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
        if ordinal < SEGMENTS {
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
    assert_eq!(opening.sync.seq, opening.outline.seq);
    assert!(opening.sync.items.len() <= 10); // Existing union: five overall plus five main.
    assert_eq!(
        opening.sync.turns.len(),
        usize::try_from(THREAD_SYNC_ENDED_TURNS)?
    );
    assert_eq!(opening.sync.segments.len(), opening.sync.turns.len());
    assert!(opening.outline.next_before.is_some());
    assert!(
        opening
            .outline
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
