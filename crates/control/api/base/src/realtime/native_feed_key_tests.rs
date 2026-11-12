//! Real credentials, ordinary HTTP readers and actual subscription/delivery paths.
//! Only the native roster observation is scripted; no model/provider call is made.

use super::Proof;

use super::Reader;

use super::Rig;

use crate::middleware::authentication::API_TOKEN_PREFIX;

use crate::middleware::authentication::new_token;

use crate::middleware::authentication::token_hash;

use crate::realtime::ephemeral;

use crate::realtime::raw_traffic;

use crate::realtime::subscriptions;

use crate::testing::TestApp;

use orch_api_contract::background::ThreadBackgroundGet;

use orch_api_contract::background::ThreadBackgroundGetQuery;

use orch_api_contract::threads::RawEvent;

use orch_api_contract::threads::ThreadRawList;

use orch_api_contract::threads::ThreadRawListQuery;

use orch_db::queries::identity;

use orch_db::queries::threads;

use orch_link_contract::background::BackgroundPlace;

use orch_link_contract::background::BackgroundState;

use orch_link_contract::background::NativeBackground;

use orch_link_contract::background::NativeBackgroundCommand;

use orch_link_contract::commands::CommandKind;

use orch_link_contract::commands::CommandResult;

use orch_realtime_contract::feeds::Feed;

use orch_realtime_contract::frames::SubFrame;

use orch_types::Id;

use orch_types::new_id;

use serde_json::Value;

use serde_json::json;
fn rule(kind: &str, id: Id, operation: &str, action: &str) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "resources":[kind],"allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(rig: &Rig, user: Id, rules: Vec<Value>) -> Proof<(Id, String)> {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules}))?;
    let mut conn = rig.pool.acquire().await?;
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "native feed read control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: rig.clock.now(),
            expires_at: Some(rig.clock.now() + chrono::Duration::minutes(5)),
        },
        Some(&serde_json::to_value(scope)?),
    )
    .await?;
    Ok((id, secret))
}

fn app(rig: &Rig) -> TestApp {
    let mut registry = crate::registry::Registry::new();
    crate::routes::mount(&mut registry);
    TestApp::over(
        rig.server.app.clone(),
        orch_testkit::clock::manual(),
        registry,
    )
}

async fn native_thread(rig: &Rig) -> Id {
    let thread = rig.thread("Native feed scope control").await;
    rig.server
        .engine
        .submit_input(
            &orch_authz::Caller::session(rig.ada),
            thread,
            super::fixture::queued("Open the existing native source"),
        )
        .await
        .unwrap();
    let turn = rig.turn_start(thread, 0).await;
    rig.begin_message(thread, &turn, "native-key-source").await;
    rig.finish_turn(thread, &turn, "native-key-source", "Ready")
        .await;
    thread
}

async fn raw_query(rig: &Rig, thread: Id, segment: Id) -> Proof<ThreadRawListQuery> {
    let projection = orch_db::queries::thread_continuations::projection(
        &mut *rig.pool.acquire().await?,
        thread,
        None,
    )
    .await?
    .ok_or("the actual thread projection is missing")?;
    Ok(ThreadRawListQuery {
        segment_id: segment,
        continuation_id: Some(projection.viewed_continuation_id),
        projection_revision: Some(projection.projection_revision),
        ..Default::default()
    })
}

async fn raw_subscribe(reader: &Reader, thread: Id, query: &ThreadRawListQuery) -> Proof<Value> {
    let frame: SubFrame = serde_json::from_value(json!({
        "id":1,"feed":Feed::ThreadRaw(thread).to_string(),
        "raw_segment_id":query.segment_id,
        "continuation_id":query.continuation_id,
        "projection_revision":query.projection_revision,
    }))?;
    subscriptions::subscribe(&reader.realtime.inner, &reader.connection, frame).await?;
    reader.next().await
}

async fn keep_raw(rig: &Rig, thread: Id, segment: Id, n: i64) -> Proof<RawEvent> {
    orch_engine::raw_events::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    let mut conn = rig.pool.acquire().await?;
    let row = threads::thread(&mut conn, thread)
        .await?
        .ok_or("thread missing")?;
    orch_engine::raw_events::Keeper::new(&row, segment)
        .keep(
            &mut conn,
            &orch_link_contract::events::RawFrame {
                n,
                direction: "in".into(),
                payload: json!({"observation":"scoped native frame"}),
            },
            rig.clock.now(),
        )
        .await?;
    orch_engine::raw_events::page(&mut conn, &row, segment, None, 1)
        .await?
        .events
        .into_iter()
        .find(|event| event.n == n)
        .ok_or_else(|| "the ordinary raw keeper did not retain the source row".into())
}

#[cfg(any(test, feature = "test-support"))]
async fn raw_manage_key_reads_the_same_segment_over_http_and_live_delivery_then_revocation_stops_it()
-> Proof {
    let rig = Rig::with_time_held().await;
    let thread = native_thread(&rig).await;
    let segment = rig.segment_of(thread).await;
    let query = raw_query(&rig, thread, segment).await?;
    let (key_id, secret) = key(
        &rig,
        rig.ada,
        vec![rule("thread", thread, "ThreadRawList", "manage")],
    )
    .await?;
    let http = app(&rig);
    // Ordinary source/permission validity is established independently of feed admission.
    http.call(ThreadRawList, &[&thread])
        .token(&secret)
        .query(&query)
        .send()
        .await
        .ok();
    let reader = Reader::new(&rig, &secret).await?;
    let ack = raw_subscribe(&reader, thread, &query).await?;
    assert_eq!(
        ack["op"], "subscribed",
        "an explicitly selected Manage read must admit its existing raw feed"
    );
    let floor = ack["raw"]["after"]
        .as_i64()
        .ok_or("missing native live floor")?;
    let event = keep_raw(&rig, thread, segment, floor + 1).await?;
    let page = http
        .call(ThreadRawList, &[&thread])
        .token(&secret)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(page.events.contains(&event));
    raw_traffic::deliver(
        &reader.realtime.inner,
        raw_traffic::Batch {
            thread,
            segment,
            events: vec![event.clone()],
        },
    )
    .await?;
    let frame = reader.next().await?;
    assert_eq!(frame["op"], "event");
    assert_eq!(frame["data"]["segment_id"], json!(segment));
    assert_eq!(frame["data"]["event"], json!(event));
    {
        let mut conn = rig.pool.acquire().await?;
        assert!(identity::revoke_api_token(&mut conn, key_id, rig.ada, rig.clock.now()).await?);
    }
    let event = keep_raw(&rig, thread, segment, floor + 2).await?;
    raw_traffic::deliver(
        &reader.realtime.inner,
        raw_traffic::Batch {
            thread,
            segment,
            events: vec![event],
        },
    )
    .await?;
    reader.empty();
    assert_eq!(
        reader.connection.outbox.closing().map(|(code, _)| code),
        Some(4401)
    );
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_raw_manage_key_reads_the_same_segment_over_http_and_live_delivery_then_revocation_stops_it()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::epoch_handoff_tests::Proof> {
    ::std::boxed::Box::pin(self::raw_manage_key_reads_the_same_segment_over_http_and_live_delivery_then_revocation_stops_it())
}

#[cfg(any(test, feature = "test-support"))]
async fn raw_scope_does_not_supply_ordinary_management_or_admit_a_foreign_segment() -> Proof {
    let rig = Rig::with_time_held().await;
    let thread = native_thread(&rig).await;
    let segment = rig.segment_of(thread).await;
    let query = raw_query(&rig, thread, segment).await?;
    let (viewer, _) = rig.person("native-viewer").await;
    rig.give(viewer, "viewer").await;
    let (_, secret) = key(
        &rig,
        viewer,
        vec![rule("thread", thread, "ThreadRawList", "manage")],
    )
    .await?;
    let reader = Reader::new(&rig, &secret).await?;
    let refusal = raw_subscribe(&reader, thread, &query).await?;
    assert_eq!(refusal["code"], "forbidden");
    reader.finish().await;

    let other = native_thread(&rig).await;
    let foreign = rig.segment_of(other).await;
    let (_, secret) = key(
        &rig,
        rig.ada,
        vec![rule("thread", thread, "ThreadRawList", "manage")],
    )
    .await?;
    let reader = Reader::new(&rig, &secret).await?;
    let foreign_query = ThreadRawListQuery {
        segment_id: foreign,
        ..query
    };
    let http = app(&rig)
        .call(ThreadRawList, &[&thread])
        .token(&secret)
        .query(&foreign_query)
        .send()
        .await;
    assert_eq!(http.status, http::StatusCode::NOT_FOUND);
    let refusal = raw_subscribe(&reader, thread, &foreign_query).await?;
    assert_eq!(refusal["code"], "not_found");
    reader.finish().await;
    rig.server.stop().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_raw_scope_does_not_supply_ordinary_management_or_admit_a_foreign_segment()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::epoch_handoff_tests::Proof> {
    ::std::boxed::Box::pin(
        self::raw_scope_does_not_supply_ordinary_management_or_admit_a_foreign_segment(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn background_read_uses_the_same_provider_directory_scope_for_http_and_delivery() -> Proof {
    let rig = Rig::with_time_held().await;
    let thread = native_thread(&rig).await;
    rig.machine.handler.on(CommandKind::SessionBackground, |_| {
        Ok(CommandResult::SessionBackground(NativeBackground {
            supported: true,
            as_of: None,
            state: BackgroundState::Live,
            reason: None,
            items: vec![NativeBackgroundCommand {
                id: "native-background-source".into(),
                native_item_ref: None,
                turn_id: None,
                command: Some("background observation".into()),
                command_truncated: false,
                cwd: Some("/provider/private-directory".into()),
                place: BackgroundPlace::Provider,
                since: None,
                cpu_percent: None,
                memory_bytes: None,
            }],
            next_cursor: None,
            total: Some(1),
        }))
    });
    let http = app(&rig);
    let ordinary = http
        .call(ThreadBackgroundGet, &[&thread])
        .token(&rig.token)
        .query(&ThreadBackgroundGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        ordinary.items[0].cwd.as_deref(),
        Some("/provider/private-directory")
    );
    let capture = rig.server.engine.capture_background(thread).await?;
    let data = serde_json::to_value(capture)?;
    let (viewer, _) = rig.person("background-viewer").await;
    rig.give(viewer, "viewer").await;
    for (user, provider_operation, directory_visible) in [
        (rig.ada, None, false),
        (rig.ada, Some("ThreadBackgroundGet"), true),
        (viewer, Some("ThreadBackgroundGet"), false),
        (rig.ada, Some("ProviderGet"), false),
    ] {
        let mut rules = vec![rule("thread", thread, "ThreadBackgroundGet", "read")];
        if let Some(operation) = provider_operation {
            rules.push(rule("provider", rig.provider, operation, "read"));
        }
        let (key_id, secret) = key(&rig, user, rules).await?;
        let page = http
            .call(ThreadBackgroundGet, &[&thread])
            .token(&secret)
            .query(&ThreadBackgroundGetQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].cwd.is_some(), directory_visible);
        assert_eq!(page.items[0].cwd_redacted, !directory_visible);
        let reader = Reader::new(&rig, &secret).await?;
        let sub = reader.subscribe(Feed::ThreadBackground(thread)).await?;
        assert!(sub.is_live());
        ephemeral::deliver(&reader.realtime.inner, sub.feed(), &data, None).await?;
        let frame = reader.next().await?;
        assert_eq!(frame["op"], "event");
        assert_eq!(frame["data"]["items"], json!(page.items));
        assert_eq!(frame["data"]["total"], json!(page.total));
        {
            let mut conn = rig.pool.acquire().await?;
            assert!(identity::revoke_api_token(&mut conn, key_id, user, rig.clock.now()).await?);
        }
        ephemeral::deliver(&reader.realtime.inner, sub.feed(), &data, None).await?;
        reader.empty();
        assert_eq!(
            reader.connection.outbox.closing().map(|(code, _)| code),
            Some(4401)
        );
        reader.finish().await;
    }
    rig.server.stop().await;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_background_read_uses_the_same_provider_directory_scope_for_http_and_delivery()
-> ::futures::future::LocalBoxFuture<'static, crate::realtime::epoch_handoff_tests::Proof> {
    ::std::boxed::Box::pin(
        self::background_read_uses_the_same_provider_directory_scope_for_http_and_delivery(),
    )
}
