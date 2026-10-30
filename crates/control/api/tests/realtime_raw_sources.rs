//! Real HTTP/socket source joins. Extra segment rows are explicit retained-reader fixtures,
//! not claims that a native adapter opened or executed nine new conversations.

mod continuation_read_support;
mod realtime_support;

use continuation_read_support::{Proof, Retained, retained, select};
use orch_api_contract::threads::ThreadRawListReply;
use orch_api_core::realtime::raw_traffic;
use orch_db::queries::{machine_streams, threads};
use orch_link::streams::{Delivery, Received, StreamReceiver};
use orch_link_contract::events::{Event, RawFrame};
use orch_link_contract::session::Role;
use orch_link_contract::streams::{Ev, Stream};
use orch_types::{Id, Json, new_id};
use realtime_support::{Client, Heard, Rig};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

async fn scene() -> Proof<(Rig, Id, Id, Retained)> {
    let rig = Rig::new().await;
    let thread = rig.thread("Independent native raw readers").await;
    for (text, reference) in [("first", "raw-first"), ("second", "raw-second")] {
        let turn = rig.send(thread, text).await;
        rig.begin_message(thread, &turn, reference).await;
        rig.finish_turn(thread, &turn, reference, "Synthetic native answer")
            .await;
    }
    realtime_support::eventually(
        "the actual native end work is settled before a retained read fixture",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
            AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
            )
            .bind(thread)
            .fetch_one(rig.pool.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let segment = rig.segment_of(thread).await;
    let view = retained(&rig.pool, thread, rig.ada, rig.clock.time()).await?;
    Ok((rig, thread, segment, view))
}

async fn subscribe(
    client: &mut Client,
    id: i64,
    thread: Id,
    segment: Id,
    view: Option<Id>,
    revision: i64,
) -> Value {
    let mut frame = json!({"op":"sub","id":id,"feed":format!("thread:{thread}:raw"),
        "raw_segment_id":segment,"projection_revision":revision});
    if let Some(view) = view {
        frame["continuation_id"] = json!(view);
    }
    client.send(frame).await;
    client.frame().await
}

async fn page(rig: &Rig, thread: Id, segment: Id, view: Id) -> Proof<reqwest::Response> {
    page_as(rig, &rig.token, thread, segment, view).await
}

fn with_query(path: &str, pairs: &[(&str, String)]) -> Proof<reqwest::Url> {
    let mut url = reqwest::Url::parse(path)?;
    url.query_pairs_mut()
        .extend_pairs(pairs.iter().map(|(key, value)| (*key, value.as_str())));
    Ok(url)
}

async fn page_as(
    rig: &Rig,
    token: &str,
    thread: Id,
    segment: Id,
    view: Id,
) -> Proof<reqwest::Response> {
    let url = with_query(
        &format!("http://{}/api/v1/threads/{thread}/raw", rig.server.addr),
        &[
            ("segment_id", segment.to_string()),
            ("continuation_id", view.to_string()),
            ("projection_revision", "1".into()),
        ],
    )?;
    Ok(reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .await?)
}

/// This is the existing native report admission/engine commit path. It does not bypass ownership
/// or turn an old segment into an executing lane. Raw observations have no fabricated turn ID.
async fn ingest(
    rig: &Rig,
    thread: Id,
    segment: Id,
    n: i64,
    text: &str,
) -> Proof<(Option<raw_traffic::Boundary>, Delivery)> {
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let event = Event::RawFrame(RawFrame {
        n,
        direction: "in".into(),
        payload: json!({"source":text}),
    });
    let mut receiver = StreamReceiver::new(
        BTreeMap::from([(stream.to_string(), n - 1)]),
        vec![Role::Agent],
    );
    let Received::Apply(delivery) = receiver.on_ev(Ev::of(&stream, n, rig.clock.time(), &event)?)
    else {
        return Err(
            "the synthetic native raw report did not form its exact ordered delivery".into(),
        );
    };
    let body = event.body()?;
    let mut tx = rig.pool.begin().await?;
    assert!(
        orch_db::queries::machine_reports::accepts(
            &mut tx,
            rig.machine_id,
            &stream,
            &[(event.kind().as_str(), &body)]
        )
        .await?,
        "the original physical machine must own this exact retained segment"
    );
    let boundary = raw_traffic::before(&mut tx, &delivery).await?;
    rig.server
        .engine
        .apply_events(&mut tx, rig.machine_id, &delivery)
        .await?;
    tx.commit().await?;
    Ok((boundary, delivery))
}

async fn publish(rig: &Rig, thread: Id, segment: Id, n: i64, text: &str) -> Proof {
    let (boundary, delivery) = ingest(rig, thread, segment, n, text).await?;
    raw_traffic::after(&rig.pool, boundary, &delivery).await?;
    Ok(())
}

async fn extra_sources(
    rig: &Rig,
    thread: Id,
    source: Id,
    owner: Id,
    count: usize,
) -> Proof<Vec<Id>> {
    let original = threads::segment(&mut *rig.pool.acquire().await?, source)
        .await?
        .ok_or("original segment missing")?;
    let mut tx = rig.pool.begin().await?;
    threads::lock(&mut tx, thread)
        .await?
        .ok_or("thread missing")?;
    let seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread sequence missing")?;
    let mut result = Vec::new();
    for _ in 0..count {
        let id = new_id();
        let native = format!("synthetic-retained-raw-{id}");
        let inserted = threads::insert_segment_in(
            &mut tx,
            &threads::NewSegment {
                id,
                thread_id: thread,
                agent_id: original.agent_id,
                provider_id: original.provider_id,
                environment_id: original.environment_id,
                env_generation: original.env_generation,
                arrangement: &original.arrangement,
                executor: original.executor.as_deref(),
                tool_set: original.tool_set.as_deref(),
                credential_path: &original.credential_path,
                native_session_ref: Some(&native),
                started_by: Some(rig.ada),
                started_at: rig.clock.now(),
            },
            Some(owner),
            seq,
        )
        .await?;
        assert_eq!(inserted.continuation_id, owner);
        assert!(machine_streams::bind_segment(&mut tx, thread, id, rig.machine_id).await?);
        result.push(id);
    }
    orch_db::sequences::notify(&mut tx, thread, seq).await?;
    tx.commit().await?;
    Ok(result)
}

async fn no_new_work(rig: &Rig, thread: Id) -> Proof<Json> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('inputs',(SELECT count(*) FROM thread_inputs WHERE thread_id=$1),
        'turns',(SELECT count(*) FROM turns WHERE thread_id=$1),'jobs',(SELECT count(*) FROM jobs WHERE node_id=$1))")
        .bind(thread).fetch_one(rig.pool.pool()).await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nine_independent_views_of_one_thread_never_mix_segment_local_numbers() -> Proof {
    let (rig, thread, original, view) = scene().await?;
    let segments = extra_sources(&rig, thread, original, view.root, 9).await?;
    let before = no_new_work(&rig, thread).await?;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for (index, segment) in segments.iter().enumerate() {
        let id = i64::try_from(index + 1)?;
        let ack = subscribe(&mut client, id, thread, *segment, Some(view.root), 1).await;
        assert_eq!(ack["op"], "subscribed");
        assert_eq!(ack["mode"], "ephemeral");
        assert_eq!(ack["raw"], json!({"segment_id":segment,"after":0}));
        assert_eq!(
            ack["projection"]["viewed_continuation_id"],
            json!(view.root)
        );
        let response = page(&rig, thread, *segment, view.root).await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert!(
            response
                .json::<ThreadRawListReply>()
                .await?
                .events
                .is_empty()
        );
    }
    assert_eq!(
        rig.server.realtime.subscriptions(),
        9,
        "each independent source owns exactly one real subscription"
    );
    for (index, segment) in segments.iter().enumerate() {
        let text = format!("only-source-{index}");
        publish(&rig, thread, *segment, 1, &text).await?;
        let event = client.expect("event").await;
        assert_eq!(event["id"], i64::try_from(index + 1)?);
        assert_eq!(event["data"]["segment_id"], json!(segment));
        assert_eq!(event["data"]["event"]["n"], 1);
        assert_eq!(event["data"]["event"]["payload"], json!({"source":text}));
        assert_eq!(
            event["data"]["projection"]["viewed_continuation_id"],
            json!(view.root)
        );
    }
    assert!(
        client.is_quiet_for(Duration::from_millis(300)).await,
        "one source must not fan its row into the other eight views"
    );
    assert_eq!(no_new_work(&rig, thread).await?, before);
    assert_eq!(
        rig.turn_starts(thread).len(),
        2,
        "raw readers cannot activate new native conversations"
    );
    drop(client);
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delayed_publication_is_not_replayed_to_a_new_raw_subscriber() -> Proof {
    let (rig, thread, segment, view) = scene().await?;
    let mut original = Client::signed_in(rig.server.addr, &rig.token).await;
    assert_eq!(
        subscribe(&mut original, 1, thread, segment, Some(view.root), 1).await["raw"]["after"],
        0
    );
    let (boundary, delivery) = ingest(
        &rig,
        thread,
        segment,
        1,
        "committed before newcomer admission",
    )
    .await?;
    let mut newcomer = Client::signed_in(rig.server.addr, &rig.token).await;
    let ack = subscribe(&mut newcomer, 2, thread, segment, Some(view.root), 1).await;
    assert_eq!(ack["raw"]["after"], 1);
    raw_traffic::after(&rig.pool, boundary, &delivery).await?;
    assert_eq!(original.expect("event").await["data"]["event"]["n"], 1);
    assert!(
        newcomer.is_quiet_for(Duration::from_millis(300)).await,
        "admission floor excludes already committed rows even when publication is late"
    );
    publish(&rig, thread, segment, 2, "a genuine new live frame").await?;
    assert_eq!(original.expect("event").await["data"]["event"]["n"], 2);
    assert_eq!(newcomer.expect("event").await["data"]["event"]["n"], 2);
    drop((original, newcomer));
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inherited_partial_segment_refuses_both_http_and_raw_ws_while_its_original_owner_works()
-> Proof {
    let (rig, thread, segment, view) = scene().await?;
    let response = page(&rig, thread, segment, view.child).await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let refusal: Value = response.json().await?;
    assert_eq!(refusal["code"], "request.value_refused");
    assert_eq!(refusal["continuation_id"], json!(view.root));
    let mut partial = Client::signed_in(rig.server.addr, &rig.token).await;
    let refused = subscribe(&mut partial, 1, thread, segment, Some(view.child), 1).await;
    assert_eq!(refused["op"], "error");
    assert_eq!(refused["code"], "read_unavailable");
    assert_eq!(
        refused["detail"],
        "These raw protocol frames describe the whole native session. Open its original history to read them."
    );
    partial.send(json!({"op":"ping","n":1})).await;
    assert_eq!(partial.expect("pong").await["n"], 1);
    assert_eq!(
        rig.server.realtime.subscriptions(),
        0,
        "a refused source retains no subscription slot"
    );
    let mut owner = Client::signed_in(rig.server.addr, &rig.token).await;
    assert_eq!(
        subscribe(&mut owner, 2, thread, segment, Some(view.root), 1).await["mode"],
        "ephemeral"
    );
    publish(&rig, thread, segment, 1, "whole original native session").await?;
    assert_eq!(
        owner.expect("event").await["data"]["event"]["payload"],
        json!({"source":"whole original native session"})
    );
    assert!(partial.is_quiet_for(Duration::from_millis(300)).await);
    let page = page(&rig, thread, segment, view.root)
        .await?
        .json::<ThreadRawListReply>()
        .await?;
    assert_eq!(page.events.len(), 1);
    assert_eq!(
        page.events[0].payload,
        json!({"source":"whole original native session"})
    );
    drop((partial, owner));
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_active_resets_without_a_raw_frame_while_an_explicit_inactive_owner_keeps_its_late_facts()
-> Proof {
    let (rig, thread, segment, view) = scene().await?;
    let before = no_new_work(&rig, thread).await?;
    let mut following = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut owner = Client::signed_in(rig.server.addr, &rig.token).await;
    assert_eq!(
        subscribe(&mut following, 1, thread, segment, None, 1).await["projection"]["follows_active"],
        true
    );
    assert_eq!(
        subscribe(&mut owner, 2, thread, segment, Some(view.root), 1).await["projection"]["follows_active"],
        false
    );
    select(&rig.pool, thread, view.child, rig.clock.time()).await?;
    assert_eq!(
        following.expect("resync").await["id"],
        1,
        "the commit itself resets raw even with no later raw report"
    );
    publish(&rig, thread, segment, 1, "late factual original report").await?;
    let late = owner.expect("event").await;
    assert_eq!(late["data"]["segment_id"], json!(segment));
    assert_eq!(
        late["data"]["projection"]["viewed_continuation_id"],
        json!(view.root)
    );
    assert_eq!(
        late["data"]["projection"]["active_continuation_id"],
        json!(view.child)
    );
    assert_eq!(
        late["data"]["event"]["payload"],
        json!({"source":"late factual original report"})
    );
    assert!(
        following.is_quiet_for(Duration::from_millis(300)).await,
        "the reset subscription cannot receive another source's fact"
    );
    select(&rig.pool, thread, view.root, rig.clock.time()).await?;
    let mut aba = Client::signed_in(rig.server.addr, &rig.token).await;
    assert_eq!(
        subscribe(&mut aba, 3, thread, segment, None, 3).await["mode"],
        "ephemeral"
    );
    select(&rig.pool, thread, view.child, rig.clock.time()).await?;
    select(&rig.pool, thread, view.root, rig.clock.time()).await?;
    assert_eq!(
        aba.expect("resync").await["id"],
        3,
        "A -> B -> A cannot reuse an earlier execution revision"
    );
    publish(&rig, thread, segment, 2, "same original owner after ABA").await?;
    assert_eq!(
        owner.expect("event").await["data"]["projection"]["execution_revision"],
        5
    );
    assert!(aba.is_quiet_for(Duration::from_millis(300)).await);
    assert_eq!(no_new_work(&rig, thread).await?, before);
    drop((following, owner, aba));
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_grammar_membership_and_stale_admission_never_fall_back_to_an_unscoped_live_feed()
-> Proof {
    let (rig, thread, segment, view) = scene().await?;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for (id, extra) in [
        (1, json!({})),
        (2, json!({"raw_segment_id":segment})),
        (3, json!({"raw_segment_id":segment,"projection_revision":0})),
    ] {
        let mut frame = json!({"op":"sub","id":id,"feed":format!("thread:{thread}:raw")});
        frame
            .as_object_mut()
            .ok_or("object")?
            .extend(extra.as_object().ok_or("extra object")?.clone());
        client.send(frame).await;
        assert_eq!(client.expect("error").await["code"], "bad_feed");
    }
    client
        .send(json!({"op":"sub","id":4,"feed":format!("thread:{thread}"),"raw_segment_id":segment}))
        .await;
    assert_eq!(client.expect("error").await["code"], "bad_feed");
    let foreign = rig.thread("Foreign raw source").await;
    let foreign_owner = threads::thread(&mut *rig.pool.acquire().await?, foreign)
        .await?
        .ok_or("foreign thread")?
        .active_continuation_id;
    let foreign_segment = extra_sources(&rig, foreign, segment, foreign_owner, 1).await?[0];
    assert_eq!(
        subscribe(&mut client, 5, thread, foreign_segment, Some(view.root), 1).await["code"],
        "not_found"
    );
    assert_eq!(
        subscribe(&mut client, 6, thread, segment, Some(new_id()), 1).await["code"],
        "not_found"
    );
    let ack = subscribe(&mut client, 7, thread, segment, Some(view.root), 99).await;
    assert_eq!(ack["op"], "subscribed");
    assert_eq!(ack["mode"], "reload");
    assert_eq!(ack["projection"]["projection_revision"], 1);
    client.send(json!({"op":"ping","n":1})).await;
    assert_eq!(client.expect("pong").await["n"], 1);
    assert_eq!(
        rig.server.realtime.subscriptions(),
        0,
        "reload releases the source before processing the next client frame"
    );
    publish(&rig, thread, segment, 1, "no retained stale subscription").await?;
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    drop(client);
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_active_change_requires_a_verified_owner_opening_before_the_new_raw_head_join()
-> Proof {
    let (rig, thread, segment, view) = scene().await?;
    let before = no_new_work(&rig, thread).await?;
    select(&rig.pool, thread, view.child, rig.clock.time()).await?;
    select(&rig.pool, thread, view.root, rig.clock.time()).await?;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    let stale = subscribe(&mut client, 1, thread, segment, None, 1).await;
    assert_eq!(stale["mode"], "reload");
    assert_eq!(stale["projection"]["projection_revision"], 3);
    assert_eq!(
        stale["projection"]["viewed_continuation_id"],
        json!(view.root)
    );
    client.send(json!({"op":"ping","n":1})).await;
    assert_eq!(client.expect("pong").await["n"], 1);
    assert_eq!(rig.server.realtime.subscriptions(), 0);
    publish(
        &rig,
        thread,
        segment,
        1,
        "a stale raw ACK retains no subscriber",
    )
    .await?;
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    let http = reqwest::Client::new();
    let path = format!("http://{}/api/v1/threads/{thread}", rig.server.addr);
    let old = http
        .get(with_query(
            &format!("{path}/raw"),
            &[
                ("segment_id", segment.to_string()),
                ("projection_revision", "1".into()),
            ],
        )?)
        .bearer_auth(&rig.token)
        .send()
        .await?;
    assert_eq!(
        old.status(),
        reqwest::StatusCode::CONFLICT,
        "the client cannot loop the old revision through raw GET"
    );
    // This is the ordinary owning reader's bounded opening, not applying the raw ACK as thread data.
    let opening = http
        .get(with_query(
            &format!("{path}/sync"),
            &[("limit", "1".into())],
        )?)
        .bearer_auth(&rig.token)
        .send()
        .await?;
    assert_eq!(opening.status(), reqwest::StatusCode::OK);
    let opening: orch_api_contract::threads::ThreadSync = opening.json().await?;
    let projection = opening
        .projection
        .ok_or("verified opening projection absent")?;
    assert_eq!(projection.projection_revision, 3);
    assert!(projection.follows_active);
    let fresh = subscribe(
        &mut client,
        2,
        thread,
        segment,
        None,
        projection.projection_revision,
    )
    .await;
    assert_eq!(fresh["mode"], "ephemeral");
    assert_eq!(fresh["raw"]["after"], 1);
    let response = http
        .get(with_query(
            &format!("{path}/raw"),
            &[
                ("segment_id", segment.to_string()),
                (
                    "projection_revision",
                    projection.projection_revision.to_string(),
                ),
                ("limit", "100".into()),
            ],
        )?)
        .bearer_auth(&rig.token)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let head: ThreadRawListReply = response.json().await?;
    assert_eq!(head.projection.as_ref(), Some(&projection));
    assert_eq!(
        head.events.iter().map(|event| event.n).collect::<Vec<_>>(),
        [1]
    );
    publish(
        &rig,
        thread,
        segment,
        2,
        "only the new captured join receives live facts",
    )
    .await?;
    let event = client.expect("event").await;
    assert_eq!(event["id"], 2);
    assert_eq!(
        event["data"]["projection"],
        serde_json::to_value(&projection)?
    );
    assert_eq!(event["data"]["event"]["n"], 2);
    assert_eq!(no_new_work(&rig, thread).await?, before);
    drop(client);
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_permission_only_resync_rejoins_the_same_raw_source_without_reopening_its_owner() -> Proof
{
    let (rig, thread, segment, view) = scene().await?;
    let before = no_new_work(&rig, thread).await?;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    let original = subscribe(&mut client, 1, thread, segment, Some(view.root), 1).await;
    assert_eq!(original["mode"], "ephemeral");
    rig.bump_epoch().await;
    assert_eq!(client.expect("resync").await["id"], 1);
    // since is ignored by raw, including when reusing a formerly reset subscription ID.
    // The unchanged source must pass its own admission rather than a generic durable reload ACK.
    client
        .send(
            json!({"op":"sub","id":1,"feed":format!("thread:{thread}:raw"),
        "raw_segment_id":segment,"continuation_id":view.root,"projection_revision":1,"since":0}),
        )
        .await;
    let fresh = client.expect("subscribed").await;
    assert_eq!(fresh["mode"], "ephemeral");
    assert_eq!(fresh["projection"], original["projection"]);
    assert_eq!(fresh["raw"], json!({"segment_id":segment,"after":0}));
    assert_eq!(rig.server.realtime.subscriptions(), 1);
    publish(
        &rig,
        thread,
        segment,
        1,
        "same verified owner after authority revalidation",
    )
    .await?;
    let event = client.expect("event").await;
    assert_eq!(event["data"]["projection"], original["projection"]);
    assert_eq!(event["data"]["event"]["n"], 1);
    assert_eq!(no_new_work(&rig, thread).await?, before);
    drop(client);
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_delivery_rechecks_manage_and_the_exact_session_without_disabling_another_authorized_reader()
-> Proof {
    let (rig, thread, segment, view) = scene().await?;
    let (person, token) = rig.person("raw-recipient").await;
    let mut peer = Client::signed_in(rig.server.addr, &token).await;
    assert_eq!(
        page_as(&rig, &token, thread, segment, view.root)
            .await?
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert_eq!(
        subscribe(&mut peer, 1, thread, segment, Some(view.root), 1).await["code"],
        "forbidden"
    );
    let role = rig.give(person, "developer").await;
    assert_eq!(
        page_as(&rig, &token, thread, segment, view.root)
            .await?
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        subscribe(&mut peer, 2, thread, segment, Some(view.root), 1).await["mode"],
        "ephemeral"
    );
    rig.take(role).await;
    assert_eq!(
        page_as(&rig, &token, thread, segment, view.root)
            .await?
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    publish(&rig, thread, segment, 1, "never after manage withdrawal").await?;
    let closed = peer.expect("closed").await;
    assert_eq!(closed["reason"], "revoked");
    let _restored = rig.give(person, "developer").await;
    let mut credential = Client::signed_in(rig.server.addr, &token).await;
    assert_eq!(
        subscribe(&mut credential, 3, thread, segment, Some(view.root), 1).await["raw"]["after"],
        1
    );
    let mut owner = Client::signed_in(rig.server.addr, &rig.token).await;
    assert_eq!(
        subscribe(&mut owner, 4, thread, segment, Some(view.root), 1).await["mode"],
        "ephemeral"
    );
    sqlx::query(
        "UPDATE sessions s SET revoked_at=$2 FROM access_tokens a
        WHERE a.session_id=s.id AND a.token_hash=$1",
    )
    .bind(orch_api_core::middleware::authentication::token_hash(
        &token,
    ))
    .bind(rig.clock.now())
    .execute(rig.pool.pool())
    .await?;
    assert_eq!(
        page_as(&rig, &token, thread, segment, view.root)
            .await?
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    publish(
        &rig,
        thread,
        segment,
        2,
        "only the still-current credential",
    )
    .await?;
    assert_eq!(
        owner.expect("event").await["data"]["event"]["payload"],
        json!({"source":"only the still-current credential"})
    );
    match credential.hear_within(realtime_support::PATIENCE).await {
        Some(Heard::Closed(Some(4401))) => {}
        other => {
            return Err(format!(
                "a revoked raw credential must close before any further payload: {other:?}"
            )
            .into());
        }
    }
    drop((peer, credential, owner));
    rig.server.stop().await;
    Ok(())
}
