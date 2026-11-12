//! Native shared-setting/individual-document resolution and event pacing, without dispatch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]
mod common;
mod shared;

use common::World;
use orch_api_contract::summaries::{SummaryTarget, SummaryTrigger};
use orch_db::queries::{settings as stored, summaries};
use orch_engine::summaries::{options, sources};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use shared::Places;

async fn setting(world: &World, node: Id, key: &str, value: Json, writer: Id, locked: bool) {
    let mut conn = world.pool.acquire().await.unwrap();
    let result = stored::put(
        &mut conn,
        Some(node),
        key,
        &stored::Stored {
            value: &value,
            locked,
            by: Some(writer),
            at: world.clock.now(),
        },
        None,
    )
    .await
    .unwrap();
    assert!(matches!(result, stored::Put::Written(_)));
}

#[tokio::test]
async fn target_defaults_ship_off_and_each_target_uses_its_actual_place_and_producer_defaults() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread_in(world.topic, "Work").await;
    let document = world
        .document_beside(thread, "Notes", &["Written text."])
        .await;
    let chat = shared::chat_of(&world, thread).await;
    let mut conn = world.pool.acquire().await.unwrap();
    for (target, seconds) in [
        (SummaryTarget::Thread { id: thread }, 120),
        (SummaryTarget::Topic { id: world.topic }, 900),
        (SummaryTarget::Document { id: document }, 300),
        (SummaryTarget::Chat { id: chat }, 300),
        (SummaryTarget::ChatTitle { id: chat }, 300),
    ] {
        let identity = sources::identify(&mut conn, target).await.unwrap();
        let policy = options::resolve(&mut conn, &identity).await.unwrap();
        assert!(!policy.options.automatic);
        assert!(policy.producer.is_none());
        assert_eq!(
            policy.output_chars.get(),
            if matches!(target, SummaryTarget::ChatTitle { .. }) {
                80
            } else {
                600
            }
        );
        assert_eq!(policy.context_tokens.get(), 32_000);
        assert_eq!(policy.min_interval_secs, seconds, "{target:?}");
        assert_eq!(policy.follow_interval_secs, 5);
    }
    drop(conn);
    setting(
        &world,
        world.root,
        keys::AUTO_SUMMARIZER,
        json!({"kind":"scripted","context_tokens":42_000}),
        world.ada,
        false,
    )
    .await;
    setting(
        &world,
        world.root,
        keys::SUMMARY_TARGETS,
        json!({
            "thread":{"automatic":true,"output_chars":1000},
            "document":{"automatic":true,"output_chars":1200}
        }),
        world.ada,
        false,
    )
    .await;
    let other = world.person("other").await;
    setting(
        &world,
        thread,
        keys::SUMMARY_TARGETS,
        json!({"thread":{"automatic":false,"context_tokens":3000}}),
        other,
        false,
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let identity = sources::identify(&mut conn, SummaryTarget::Document { id: document })
        .await
        .unwrap();
    assert_eq!(
        identity.place(),
        thread,
        "a document uses the actual thread place"
    );
    let doc = options::resolve(&mut conn, &identity).await.unwrap();
    assert!(doc.options.automatic);
    assert_eq!(doc.output_chars.get(), 1200);
    assert_eq!(
        doc.options_writer,
        Some(world.ada),
        "an unrelated later thread entry cannot become the document entry's writer"
    );
    assert_eq!(doc.producer.as_ref().unwrap().writer, Some(world.ada));
    assert_eq!(doc.context_tokens.get(), 42_000);
    assert_eq!(
        doc.on,
        std::collections::BTreeSet::from([SummaryTrigger::ContentChanged])
    );
    let identity = sources::identify(&mut conn, SummaryTarget::Thread { id: thread })
        .await
        .unwrap();
    let thread = options::resolve(&mut conn, &identity).await.unwrap();
    assert!(!thread.options.automatic);
    assert_eq!(thread.options_writer, Some(other));
    assert_eq!(thread.context_tokens.get(), 3000);
    assert_eq!(
        thread.output_chars.get(),
        600,
        "ordinary Map replaces a target entry rather than secretly deep-merging it"
    );
    assert!(thread.on.contains(&SummaryTrigger::Milestone));
    assert!(thread.on.contains(&SummaryTrigger::TurnCompleted));
    let before = thread.fingerprint;
    drop(conn);
    setting(
        &world,
        world.root,
        keys::AUTO_SUMMARIZER,
        json!({"kind":"scripted","context_tokens":42_000}),
        other,
        false,
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let changed = options::resolve(&mut conn, &identity).await.unwrap();
    assert_eq!(changed.producer.as_ref().unwrap().writer, Some(other));
    assert_ne!(
        before, changed.fingerprint,
        "a new responsible writer cannot reuse the old effect/publication fingerprint"
    );
}

#[tokio::test]
async fn explicit_shared_pacing_overrides_kind_defaults_and_target_values_take_precedence() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread_in(world.topic, "Work").await;
    let document = world
        .document_beside(thread, "Notes", &["Written text."])
        .await;
    let chat = shared::chat_of(&world, thread).await;
    setting(
        &world,
        world.root,
        keys::AUTO_TRIGGERS,
        json!({"on":["turn_completed"],"min_interval_secs":77}),
        world.ada,
        false,
    )
    .await;
    let targets = [
        SummaryTarget::Thread { id: thread },
        SummaryTarget::Topic { id: world.topic },
        SummaryTarget::Document { id: document },
        SummaryTarget::Chat { id: chat },
        SummaryTarget::ChatTitle { id: chat },
    ];
    let mut conn = world.pool.acquire().await.unwrap();
    for target in targets {
        let identity = sources::identify(&mut conn, target).await.unwrap();
        assert_eq!(
            options::resolve(&mut conn, &identity)
                .await
                .unwrap()
                .min_interval_secs,
            77,
            "{target:?}"
        );
    }
    drop(conn);
    setting(
        &world,
        world.root,
        keys::SUMMARY_TARGETS,
        json!({"topic":{"automatic":false,"min_interval_secs":0}}),
        world.ada,
        false,
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let topic = sources::identify(&mut conn, SummaryTarget::Topic { id: world.topic })
        .await
        .unwrap();
    assert_eq!(
        options::resolve(&mut conn, &topic)
            .await
            .unwrap()
            .min_interval_secs,
        0
    );
    let document = sources::identify(&mut conn, SummaryTarget::Document { id: document })
        .await
        .unwrap();
    assert_eq!(
        options::resolve(&mut conn, &document)
            .await
            .unwrap()
            .min_interval_secs,
        77
    );
}

#[tokio::test]
async fn individual_document_options_inherit_restore_and_obey_every_ancestor_lock() {
    let world = World::new().await;
    world.engine().stop();
    let doc = world
        .document_beside(world.topic, "Notes", &["Written text."])
        .await;
    let writer = world.person("writer").await;
    setting(
        &world,
        world.root,
        keys::AUTO_SUMMARIZER,
        json!({"kind":"scripted"}),
        world.ada,
        false,
    )
    .await;
    setting(
        &world,
        world.topic,
        keys::SUMMARY_TARGETS,
        json!({"document":{"automatic":true,"output_chars":900}}),
        world.ada,
        false,
    )
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    let cache = summaries::ensure(
        &mut tx,
        SummaryTarget::Document { id: doc },
        None,
        world.clock.now(),
    )
    .await
    .unwrap()
    .unwrap();
    let own = json!({"automatic":false,"on":[],"min_interval_secs":0,"output_chars":2000});
    summaries::configure(&mut tx, cache.id, 0, Some(&own), writer, world.clock.now())
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let identity = sources::identify(&mut conn, SummaryTarget::Document { id: doc })
        .await
        .unwrap();
    let selected = options::resolve(&mut conn, &identity).await.unwrap();
    assert!(!selected.options.automatic);
    assert_eq!(selected.options_writer, Some(writer));
    assert_eq!(
        selected.producer.as_ref().unwrap().writer,
        Some(world.ada),
        "the paying producer writer is separate from an options editor"
    );
    assert_eq!(selected.output_chars.get(), 2000);
    assert_eq!(selected.min_interval_secs, 0);
    assert!(selected.on.is_empty());
    assert_eq!(selected.own_revision, 1);
    drop(conn);
    // The containing topic's own lock is above the document, even though it is not above
    // that same topic's ordinary node settings writer.
    setting(
        &world,
        world.topic,
        keys::SUMMARY_TARGETS,
        json!({"document":{"automatic":true,"output_chars":900}}),
        world.ada,
        true,
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let locked = options::resolve(&mut conn, &identity).await.unwrap();
    assert_eq!(locked.locked_by.as_ref().unwrap().node, Some(world.topic));
    assert!(
        locked.own.is_some(),
        "ignored own state is retained, not silently rewritten"
    );
    assert!(locked.options.automatic);
    assert_eq!(locked.output_chars.get(), 900);
    assert_ne!(selected.fingerprint, locked.fingerprint);
    drop(conn);
    setting(
        &world,
        world.topic,
        keys::SUMMARY_TARGETS,
        json!({"document":{"automatic":true,"output_chars":900}}),
        world.ada,
        false,
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    summaries::configure(&mut conn, cache.id, 1, None, writer, world.clock.now())
        .await
        .unwrap()
        .unwrap();
    let inherited = options::resolve(&mut conn, &identity).await.unwrap();
    assert!(inherited.own.is_none());
    assert_eq!(inherited.own_revision, 2);
    assert_eq!(inherited.output_chars.get(), 900);
    assert!(inherited.locked_by.is_none());
}

#[tokio::test]
async fn change_events_are_delayed_through_the_gap_and_explicit_zero_and_empty_values_are_preserved()
 {
    use chrono::Duration;
    let world = World::new().await;
    world.engine().stop();
    let doc = world
        .document_beside(world.topic, "Notes", &["Written text."])
        .await;
    setting(
        &world,
        world.topic,
        keys::AUTO_SUMMARIZER,
        json!({"kind":"scripted"}),
        world.ada,
        false,
    )
    .await;
    setting(
        &world,
        world.topic,
        keys::SUMMARY_TARGETS,
        json!({"document":{"automatic":true,"min_interval_secs":10}}),
        world.ada,
        false,
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let identity = sources::identify(&mut conn, SummaryTarget::Document { id: doc })
        .await
        .unwrap();
    let mut policy = options::resolve(&mut conn, &identity).await.unwrap();
    let now = world.clock.now();
    assert_eq!(
        policy.earliest(SummaryTrigger::ContentChanged, true, None, now),
        Some(now)
    );
    assert_eq!(
        policy.earliest(
            SummaryTrigger::ContentChanged,
            true,
            Some(now - Duration::seconds(5)),
            now
        ),
        Some(now + Duration::seconds(5))
    );
    assert_eq!(
        policy.earliest(SummaryTrigger::ContentChanged, false, None, now),
        None
    );
    assert_eq!(
        policy.earliest(SummaryTrigger::Milestone, true, None, now),
        None
    );
    policy.min_interval_secs = 0;
    assert_eq!(
        policy.earliest(SummaryTrigger::ContentChanged, true, Some(now), now),
        Some(now)
    );
    policy.on.clear();
    assert_eq!(
        policy.earliest(SummaryTrigger::ContentChanged, true, None, now),
        None
    );
    policy.on.insert(SummaryTrigger::ContentChanged);
    policy.options.automatic = false;
    assert_eq!(
        policy.earliest(SummaryTrigger::ContentChanged, true, None, now),
        None
    );
    policy.options.automatic = true;
    policy.producer = None;
    assert_eq!(
        policy.earliest(SummaryTrigger::ContentChanged, true, None, now),
        None
    );
}
