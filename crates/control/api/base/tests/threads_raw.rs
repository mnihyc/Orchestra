//! The raw events of a thread over HTTP: what an adapter reports of its harness's own protocol is kept in the order it came, with nothing in it that must not be stored, and listed newest first, a page at a time, to whoever manages the thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{
    RawDirection, ThreadRawList, ThreadRawListQuery, ThreadRawListReply,
};
use orch_db::audit::{Actor, Origin};
use orch_db::secrets::MasterKey;
use orch_link_contract::commands::{Command, CommandKind};
use orch_link_contract::events::{Event, RawFrame};
use orch_settings::Target;
use orch_settings::secrets::{Owner, Put, put};
use orch_types::Id;
use places::Who;
use rig::{Scene, eventually};
use serde_json::{Value, json};

/// The key that opens secrets in every test of this file: the engine keeps one for a process, the
/// first installed, and a test application installs this one for every place that seals.
const KEY: [u8; 32] = [7; 32];

fn frame(direction: &str, payload: Value) -> Event {
    Event::RawFrame(RawFrame {
        n: 0,
        direction: direction.to_owned(),
        payload,
    })
}

async fn list(
    s: &Scene,
    by: &Who,
    thread: Id,
    segment: Id,
    before: Option<i64>,
    limit: Option<u32>,
) -> orch_api_core::testing::Reply<ThreadRawListReply> {
    s.rig
        .world
        .app
        .call(ThreadRawList, &[&thread])
        .token(&by.token)
        .query(&ThreadRawListQuery {
            segment_id: segment,
            before,
            limit,
            ..Default::default()
        })
        .send()
        .await
}

/// A thread that has had a turn, so that it has a segment, and the segment.
async fn started(s: &Scene) -> (Id, Id) {
    orch_engine::raw_events::install_master_key(MasterKey::new(KEY));
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "hello", "hi").await;
    (thread, s.rig.segment_of(thread).await)
}

/// Waits until `count` frames are kept for the segment.
async fn wait_for(s: &Scene, thread: Id, segment: Id, count: usize) {
    eventually("the frames are kept", || async move {
        list(s, &s.ada, thread, segment, None, Some(500))
            .await
            .body
            .map(|page| page.events.len())
            .unwrap_or_default()
            == count
    })
    .await;
}

#[tokio::test]
async fn what_a_harness_reports_of_its_protocol_is_listed_newest_first_with_the_way_it_went() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    let sent = json!({ "method": "turn/start", "params": { "input": "do it" } });
    let heard = json!({ "method": "turn/started", "result": { "id": 7 } });
    s.rig.say(thread, frame("out", sent.clone())).await;
    s.rig.say(thread, frame("in", heard.clone())).await;
    s.rig
        .say(
            thread,
            frame("connector", json!({ "policy": "x", "asked": 9, "used": 4 })),
        )
        .await;
    wait_for(&s, thread, segment, 3).await;

    let page = list(&s, &s.ada, thread, segment, None, None).await.ok();
    assert!(!page.has_older);
    let numbers: Vec<i64> = page.events.iter().map(|event| event.n).collect();
    assert_eq!(
        numbers,
        [3, 2, 1],
        "newest first, numbered from 1 in the order reported"
    );
    assert_eq!(page.events[1].direction, RawDirection::FromHarness);
    assert_eq!(page.events[1].payload, heard);
    assert_eq!(page.events[2].direction, RawDirection::ToHarness);
    assert_eq!(page.events[2].payload, sent);
    // A word that is neither way travels with the frame, which is then a note of the connector's.
    assert_eq!(
        page.events[0].payload,
        json!({ "direction": "connector", "frame": { "policy": "x", "asked": 9, "used": 4 } })
    );
    assert!(page.events[0].at >= page.events[2].at);
}

#[tokio::test]
async fn a_page_goes_back_from_a_number_and_says_whether_older_ones_remain() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    for each in 1..=5 {
        s.rig.say(thread, frame("in", json!({ "n": each }))).await;
    }
    wait_for(&s, thread, segment, 5).await;
    let first = list(&s, &s.ada, thread, segment, None, Some(2)).await.ok();
    assert_eq!(
        first.events.iter().map(|each| each.n).collect::<Vec<_>>(),
        [5, 4]
    );
    assert!(first.has_older);
    let second = list(&s, &s.ada, thread, segment, Some(4), Some(2))
        .await
        .ok();
    assert_eq!(
        second.events.iter().map(|each| each.n).collect::<Vec<_>>(),
        [3, 2]
    );
    assert!(second.has_older);
    let last = list(&s, &s.ada, thread, segment, Some(2), Some(2))
        .await
        .ok();
    assert_eq!(
        last.events.iter().map(|each| each.n).collect::<Vec<_>>(),
        [1]
    );
    assert!(!last.has_older);
    assert_eq!(
        list(&s, &s.ada, thread, segment, None, Some(0))
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        list(&s, &s.ada, thread, segment, Some(0), None)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn only_whoever_manages_the_thread_lists_its_raw_events() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    s.rig.say(thread, frame("in", json!({ "a": 1 }))).await;
    wait_for(&s, thread, segment, 1).await;
    // Somebody who may drive the thread and not manage it.
    s.rig
        .lets_use("node", thread, "thread.interact", &s.bea, json!({}))
        .await;
    for (who, name) in [(&s.cai, "a viewer"), (&s.bea, "somebody who drives it")] {
        let reply = list(&s, who, thread, segment, None, None).await;
        assert!(reply.body.is_err(), "{name} was let in: {:?}", reply.body);
        let refused = reply.problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{name}");
        assert_eq!(
            places::missing_permissions(&refused),
            ["thread.manage"],
            "{name}"
        );
    }
    // The scene's environment is a managed one, where the developers of the org manage the threads.
    assert_eq!(
        list(&s, &s.dan, thread, segment, None, None)
            .await
            .ok()
            .events
            .len(),
        1
    );
    let stranger = s.rig.person("fay").await;
    let reply = list(&s, &stranger, thread, segment, None, None).await;
    assert!(
        reply.body.is_err(),
        "a stranger was let in: {:?}",
        reply.body
    );
    let anonymous = s
        .rig
        .world
        .app
        .call(ThreadRawList, &[&thread])
        .query(&ThreadRawListQuery {
            segment_id: segment,
            before: None,
            limit: None,
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(anonymous.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_segment_of_another_thread_is_not_there() {
    let s = Scene::new().await;
    let (thread, _) = started(&s).await;
    let other = s.thread_by(&s.ada, "Another").await.base.id;
    s.exchange(other, 0, "hello", "hi").await;
    let theirs = s.rig.segment_of(other).await;
    assert_eq!(
        list(&s, &s.ada, thread, theirs, None, None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        list(&s, &s.ada, thread, orch_types::new_id(), None, None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn no_secret_is_kept_and_what_is_kept_is_masked_before_it_is_written() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    let topic = s.tree.topic.base.id;
    {
        let mut conn = s.rig.world.conn().await;
        put(
            &mut conn,
            &MasterKey::new(KEY),
            &Put {
                actor: Actor::User(s.ada.id()),
                owner: Owner::Node(topic),
                name: "DEPLOY_TOKEN",
                value: "hunter2-hunter2",
                origin: &Origin::default(),
            },
            s.rig.world.now(),
        )
        .await
        .unwrap();
    }
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.org.base.id),
            "redaction.patterns",
            json!([{ "name": "key", "regex": "sk-[a-z0-9]{8,}" }]),
        )
        .await;
    s.rig
        .say(
            thread,
            frame(
                "out",
                json!({
                    "method": "thread/start",
                    "params": {
                        "prompt": "deploy with hunter2-hunter2 and sk-abcdef123456, please",
                        "headers": { "Authorization": "Bearer abc.def", "Accept": "json" },
                        "env": { "hunter2-hunter2": "named by the secret" }
                    },
                    "tokens": 12
                }),
            ),
        )
        .await;
    wait_for(&s, thread, segment, 1).await;
    let page = list(&s, &s.ada, thread, segment, None, None).await.ok();
    let shown = page.events[0].payload.to_string();
    for secret in ["hunter2", "sk-abcdef", "abc.def"] {
        assert!(!shown.contains(secret), "{secret} is shown: {shown}");
    }
    assert!(shown.contains("[masked]"));
    assert_eq!(
        page.events[0].payload["params"]["headers"]["Accept"],
        "json"
    );
    assert_eq!(page.events[0].payload["tokens"], 12);

    // Masked before it was stored: what the database holds has none of it either.
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT payload FROM raw_events WHERE segment_id = $1")
            .bind(segment)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    let text = String::from_utf8(zstd::decode_all(stored.as_slice()).unwrap()).unwrap();
    for secret in ["hunter2", "sk-abcdef", "abc.def"] {
        assert!(!text.contains(secret), "{secret} is stored: {text}");
    }
}

/// Puts a secret of the topic under `key`.
async fn put_secret(s: &Scene, key: [u8; 32], name: &str, value: &str) {
    let mut conn = s.rig.world.conn().await;
    put(
        &mut conn,
        &MasterKey::new(key),
        &Put {
            actor: Actor::User(s.ada.id()),
            owner: Owner::Node(s.tree.topic.base.id),
            name,
            value,
            origin: &Origin::default(),
        },
        s.rig.world.now(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_secret_put_in_after_a_frame_was_kept_is_masked_when_it_is_listed() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    s.rig
        .say(
            thread,
            frame(
                "out",
                json!({ "prompt": "deploy with late-hunter2-value, please" }),
            ),
        )
        .await;
    wait_for(&s, thread, segment, 1).await;
    let before = list(&s, &s.ada, thread, segment, None, None).await.ok();
    assert!(
        before.events[0]
            .payload
            .to_string()
            .contains("late-hunter2-value"),
        "no secret was known when it was kept"
    );
    put_secret(&s, KEY, "LATE", "late-hunter2-value").await;
    let after = list(&s, &s.ada, thread, segment, None, None).await.ok();
    let shown = after.events[0].payload.to_string();
    assert!(!shown.contains("late-hunter2-value"), "{shown}");
    assert!(shown.contains("[masked]"));
}

#[tokio::test]
async fn where_the_secrets_cannot_be_opened_when_listing_the_frames_are_listed_as_they_were_kept() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    s.rig
        .say(thread, frame("out", json!({ "prompt": "plain words" })))
        .await;
    wait_for(&s, thread, segment, 1).await;
    put_secret(&s, [99; 32], "OTHER", "sealed-under-another-key").await;
    let page = list(&s, &s.ada, thread, segment, None, None).await.ok();
    assert_eq!(page.events[0].payload, json!({ "prompt": "plain words" }));
}

#[tokio::test]
async fn a_policy_that_does_not_mask_the_secrets_of_a_topic_leaves_their_values_and_never_a_credential()
 {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    {
        let mut conn = s.rig.world.conn().await;
        put(
            &mut conn,
            &MasterKey::new(KEY),
            &Put {
                actor: Actor::User(s.ada.id()),
                owner: Owner::Node(s.tree.topic.base.id),
                name: "DEPLOY_TOKEN",
                value: "hunter2-hunter2",
                origin: &Origin::default(),
            },
            s.rig.world.now(),
        )
        .await
        .unwrap();
    }
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.org.base.id),
            "redaction.mask_secrets",
            json!(false),
        )
        .await;
    s.rig
        .say(
            thread,
            frame(
                "out",
                json!({ "note": "hunter2-hunter2", "api_key": "k-should-never-show" }),
            ),
        )
        .await;
    wait_for(&s, thread, segment, 1).await;
    let page = list(&s, &s.ada, thread, segment, None, None).await.ok();
    assert_eq!(page.events[0].payload["note"], "hunter2-hunter2");
    assert_eq!(
        page.events[0].payload["api_key"], "[masked]",
        "a member named like a secret is masked whatever the setting says"
    );
}

#[tokio::test]
async fn a_frame_that_cannot_be_masked_is_dropped_and_never_kept_whole() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    {
        // A secret sealed with another key than the engine has: it does not open.
        let mut conn = s.rig.world.conn().await;
        put(
            &mut conn,
            &MasterKey::new([99; 32]),
            &Put {
                actor: Actor::User(s.ada.id()),
                owner: Owner::Node(s.tree.topic.base.id),
                name: "OTHER",
                value: "kept-under-another-key",
                origin: &Origin::default(),
            },
            s.rig.world.now(),
        )
        .await
        .unwrap();
    }
    s.rig
        .say(
            thread,
            frame("out", json!({ "text": "kept-under-another-key" })),
        )
        .await;
    // Give the machine's event time to be applied, then see that nothing was kept.
    s.rig
        .say(thread, frame("in", json!({ "after": true })))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let page = list(&s, &s.ada, thread, segment, None, None).await.ok();
    assert!(page.events.is_empty(), "{:?}", page.events);
}

#[tokio::test]
async fn a_frame_larger_than_may_be_kept_is_kept_as_a_note_of_what_it_was() {
    let s = Scene::new().await;
    let (thread, segment) = started(&s).await;
    let big = "x".repeat(orch_engine::raw_events::FRAME_MAX_BYTES + 10);
    s.rig
        .say(thread, frame("out", json!({ "file": big })))
        .await;
    wait_for(&s, thread, segment, 1).await;
    let page = list(&s, &s.ada, thread, segment, None, None).await.ok();
    let payload = &page.events[0].payload;
    assert_eq!(payload["truncated"], true);
    assert!(
        payload["head"]
            .as_str()
            .unwrap()
            .starts_with("{\"file\":\"xxx")
    );
}

#[tokio::test]
async fn a_session_is_opened_asking_for_the_frames_of_the_harness() {
    let s = Scene::new().await;
    let (thread, _) = started(&s).await;
    let opened: Vec<_> = s
        .rig
        .seen
        .of_kind(CommandKind::SessionOpen)
        .into_iter()
        .filter_map(|command| match command {
            Command::SessionOpen(open) if open.thread_id == thread => Some(open),
            _ => None,
        })
        .collect();
    assert!(!opened.is_empty());
    assert!(opened.iter().all(|open| open.raw_events));
}
