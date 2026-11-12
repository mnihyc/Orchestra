//! The browser's socket as a protocol: who may open it, what it is closed for, how it is answered when a request is wrong, and how a token that runs out is replaced.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use chrono::Duration as Span;
use orch_types::new_id;
use realtime_support::{Client, Rig, SlowLine};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_good_token_opens_the_socket_and_ready_says_who_and_for_how_long() {
    let rig = Rig::new().await;
    let token = rig.token_for(rig.ada, Span::minutes(15)).await;
    let mut client = Client::connect(rig.server.addr).await;
    client.send(json!({ "op": "auth", "token": token })).await;
    let ready = client.expect("ready").await;
    assert_eq!(ready["user_id"], rig.ada.to_string());
    assert_eq!(ready["heartbeat_secs"], 25);
    assert!(ready["connection_id"].as_str().unwrap().starts_with('c'));
    assert_eq!(ready["build"], rig.server.app.build.to_string());
    let expires: chrono::DateTime<chrono::Utc> = ready["access_expires_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let wanted = chrono::Utc::now() + Span::minutes(15);
    assert!((expires - wanted).num_seconds().abs() < 10, "{expires}");
    let said: chrono::DateTime<chrono::Utc> =
        ready["server_time"].as_str().unwrap().parse().unwrap();
    assert!((chrono::Utc::now() - said).num_seconds().abs() < 10);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ping_is_answered_with_its_number() {
    let rig = Rig::new().await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for _ in 0..3 {
        client.ping().await;
    }
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_does_not_offer_the_protocol_is_closed_with_4409() {
    let rig = Rig::new().await;
    for offered in [&["orch.v0"][..], &[][..]] {
        let mut client = Client::connect_offering(rig.server.addr, offered).await;
        assert_eq!(client.closed().await, Some(4409), "offering {offered:?}");
    }
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_says_nothing_of_who_it_is_in_time_is_closed_with_4401() {
    let rig = Rig::tuned(|tuning| tuning.auth_timeout = Duration::from_millis(700)).await;
    let mut client = Client::connect(rig.server.addr).await;
    assert_eq!(client.closed().await, Some(4401));
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_frame_that_is_not_auth_closes_with_4401() {
    let rig = Rig::new().await;
    let mut client = Client::connect(rig.server.addr).await;
    client
        .send(json!({ "op": "sub", "id": 1, "feed": "user" }))
        .await;
    assert_eq!(client.closed().await, Some(4401));
    let mut other = Client::connect(rig.server.addr).await;
    other.send(json!({ "op": "ping", "n": 1 })).await;
    assert_eq!(other.closed().await, Some(4401));
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_that_is_not_good_is_refused_with_4401() {
    let rig = Rig::new().await;
    let gone = rig.token_for(rig.ada, Span::minutes(15)).await;
    rig.sign_out(&gone).await;
    let ran_out = rig.token_for(rig.ada, Span::seconds(-5)).await;
    let api = rig.api_token(rig.ada).await;
    for (what, token) in [
        ("nonsense", "oat_nonsense".to_owned()),
        ("a token of a session that was ended", gone),
        ("a token that ran out", ran_out),
        ("an API token", api),
        (
            "a token that cannot be written in a header",
            "bad\ntoken".to_owned(),
        ),
    ] {
        let mut client = Client::connect(rig.server.addr).await;
        client.send(json!({ "op": "auth", "token": token })).await;
        assert_eq!(client.closed().await, Some(4401), "{what}");
    }
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_person_who_is_suspended_is_refused_with_4403_and_so_is_one_who_is_suspended_while_connected()
 {
    let rig = Rig::new().await;
    let (bo, bo_token) = rig.person("bobby").await;
    let mut connected = Client::signed_in(rig.server.addr, &bo_token).await;
    rig.suspend(bo).await;
    assert_eq!(connected.closed().await, Some(4403));
    let mut refused = Client::connect(rig.server.addr).await;
    refused
        .send(json!({ "op": "auth", "token": bo_token }))
        .await;
    assert_eq!(refused.closed().await, Some(4403));
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_is_ended_closes_its_sockets_with_4401() {
    let rig = Rig::new().await;
    let token = rig.token_for(rig.ada, Span::minutes(15)).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    rig.sign_out(&token).await;
    rig.bump_epoch().await;
    assert_eq!(client.closed().await, Some(4401));
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_personal_revocation_closes_only_that_persons_socket_without_moving_the_global_epoch() {
    let rig = Rig::new().await;
    let (other, other_token) = rig.person("other").await;
    let mut first = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut second = Client::signed_in(rig.server.addr, &other_token).await;
    let mut conn = rig.pool.acquire().await.unwrap();
    let before = orch_db::queries::access::epoch(&mut conn).await.unwrap();
    let session = orch_db::queries::identity::session_by_access_token(
        &mut conn,
        &orch_api_core::middleware::authentication::token_hash(&other_token),
        chrono::Utc::now(),
    )
    .await
    .unwrap()
    .unwrap();
    drop(conn);
    let mut tx = rig.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_person(&mut tx, other)
        .await
        .unwrap()
        .unwrap();
    assert!(
        orch_db::queries::identity::revoke_session(
            &mut tx,
            session.session_id,
            other,
            chrono::Utc::now()
        )
        .await
        .unwrap()
    );
    assert_eq!(
        orch_db::queries::access::epoch(&mut tx).await.unwrap(),
        before
    );
    tx.commit().await.unwrap();
    assert_eq!(second.closed().await, Some(4401));
    first.ping().await;
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signing_out_other_browsers_closes_their_open_sockets_but_keeps_this_one_and_another_person()
 {
    let rig = Rig::new().await;
    let other_token = rig.token_for(rig.ada, Span::days(1)).await;
    let (_, bob_token) = rig.person("bob").await;
    let mut current = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut other = Client::signed_in(rig.server.addr, &other_token).await;
    let mut bob = Client::signed_in(rig.server.addr, &bob_token).await;
    let reply = reqwest::Client::new()
        .delete(format!("http://{}/api/v1/me/sessions", rig.server.addr))
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 200);
    let kept: Vec<orch_api_contract::auth::SessionInfo> = reply.json().await.unwrap();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].current);
    assert_eq!(other.closed().await, Some(4401));
    current.ping().await;
    bob.ping().await;
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_thread_marks_follow_the_current_session_on_updates_and_switching_off_revokes_only_that_session()
 {
    async fn next_at(client: &mut Client, phase: &'static str) -> serde_json::Value {
        next_before(
            client,
            tokio::time::Instant::now() + realtime_support::PATIENCE,
            phase,
        )
        .await
    }
    async fn next_before(
        client: &mut Client,
        end: tokio::time::Instant,
        phase: &'static str,
    ) -> serde_json::Value {
        let remaining = end
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|left| !left.is_zero())
            .unwrap_or_else(|| {
                panic!("native marks phase={phase}: the original bounded wait elapsed")
            });
        match client.hear_within(remaining).await {
            Some(realtime_support::Heard::Frame(frame)) => frame,
            Some(realtime_support::Heard::Closed(code)) => {
                panic!("native marks phase={phase}: socket closed {code:?}")
            }
            None => panic!("native marks phase={phase}: original bounded wait elapsed"),
        }
    }
    async fn expect_at(client: &mut Client, op: &str, phase: &'static str) -> serde_json::Value {
        let frame = next_at(client, phase).await;
        assert_eq!(frame["op"], op, "native marks phase={phase}");
        frame
    }
    async fn expect_before(
        client: &mut Client,
        end: tokio::time::Instant,
        op: &str,
        phase: &'static str,
    ) -> serde_json::Value {
        let frame = next_before(client, end, phase).await;
        assert_eq!(frame["op"], op, "native marks phase={phase}");
        frame
    }
    async fn closed_at(
        client: &mut Client,
        end: tokio::time::Instant,
        feed: &str,
    ) -> serde_json::Value {
        loop {
            let frame = next_before(client, end, "first session off: exact revoked closure").await;
            assert_eq!(frame["id"], 1);
            assert!(
                frame["feed"] == feed,
                "revocation must name the first reader's exact thread feed"
            );
            if frame["op"] == "presence" {
                continue;
            }
            assert_eq!(
                frame["op"], "closed",
                "native marks phase=first session off requires revoked closure, never an allowed reset"
            );
            return frame;
        }
    }
    async fn mode(rig: &Rig, token: &str, enabled: bool) {
        let answer = reqwest::Client::new()
            .put(format!("http://{}/api/v1/me/admin-mode", rig.server.addr))
            .bearer_auth(token)
            .json(&json!({"as_admin":enabled}))
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 200);
        assert_eq!(
            answer.json::<serde_json::Value>().await.unwrap()["as_admin"],
            enabled
        );
    }
    let rig = Rig::new().await;
    let thread = rig.thread("Private live work").await;
    let thread_feed = format!("thread:{thread}");
    let (operator, token) = rig.person("operator").await;
    rig.give(operator, "admin").await;
    let other_token = rig.token_for(operator, Span::days(1)).await;
    let mut tx = rig.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let principal = orch_authz::Principal::load(
        &mut tx,
        orch_authz::Caller::session(rig.ada),
        orch_types::Time::new(rig.clock.now()),
    )
    .await
    .unwrap();
    orch_authz::pages::change(
        &mut tx,
        &principal,
        orch_authz::pages::Target::Live(thread),
        &serde_json::from_value(json!({"whole":"private"})).unwrap(),
        orch_authz::Policy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let http = reqwest::Client::new();
    let url = format!("http://{}/api/v1/nodes/{thread}", rig.server.addr);
    assert!(
        !http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let mut first = Client::signed_in(rig.server.addr, &token).await;
    let mut other = Client::signed_in(rig.server.addr, &other_token).await;
    mode(&rig, &token, true).await;
    let on = http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(
        on["access"]["admin_access"]["place"]["id"],
        rig.topic.to_string()
    );
    assert!(
        !http
            .get(&url)
            .bearer_auth(&other_token)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    other.sub(2, &format!("thread:{thread}"), None).await;
    assert_eq!(
        expect_at(&mut other, "error", "other session off: refused").await["code"],
        "forbidden"
    );
    first.sub(1, &format!("thread:{thread}"), None).await;
    let acknowledged = expect_at(&mut first, "subscribed", "first session on: subscribe").await;
    assert_eq!(acknowledged["mode"], "delta");
    expect_at(&mut first, "presence", "first session on: initial presence").await;
    let before_seq: (i64, i64) =
        sqlx::query_as("SELECT next_seq-1,seq_state FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(rig.pool.pool())
            .await
            .unwrap();
    let owner = http
        .get(&url)
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let changed = http
        .patch(&url)
        .bearer_auth(&rig.token)
        .json(&json!({"name":"Private live work renamed","expected_revision":owner["revision"]}))
        .send()
        .await
        .unwrap();
    assert!(changed.status().is_success());
    let committed: (i64, i64) =
        sqlx::query_as("SELECT next_seq-1,seq_state FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(rig.pool.pool())
            .await
            .unwrap();
    assert!(
        committed.0 > before_seq.0 && committed.1 > before_seq.1,
        "native marks phase=rename committed: both the canonical thread head and common-metadata stamp must advance"
    );
    let end = tokio::time::Instant::now() + realtime_support::PATIENCE;
    let sync = loop {
        let heard = first
            .hear_within(end.saturating_duration_since(tokio::time::Instant::now()))
            .await;
        let frame = match heard {
            Some(realtime_support::Heard::Frame(frame)) => frame,
            Some(realtime_support::Heard::Closed(code)) => {
                panic!("native marks phase=rename committed delta: socket closed {code:?}")
            }
            None => {
                panic!("native marks phase=rename committed delta: original bounded wait elapsed")
            }
        };
        assert_eq!(frame["id"], 1);
        if frame["op"] == "presence" {
            continue;
        }
        assert_eq!(frame["op"], "sync");
        let seq = frame["seq"]
            .as_i64()
            .expect("a native thread delta has its canonical sequence");
        if seq >= committed.1 {
            break frame;
        }
        assert!(
            seq > acknowledged["seq"].as_i64().unwrap(),
            "a prior committed delta must advance this acknowledged reader"
        );
    };
    assert_eq!(sync["data"]["thread"]["name"], "Private live work renamed");
    assert_eq!(
        sync["data"]["thread"]["access"]["admin_access"], on["access"]["admin_access"],
        "a live update keeps the same qualified native-only marker as HTTP"
    );
    let transition_end = tokio::time::Instant::now() + realtime_support::PATIENCE;
    let epoch_before = {
        let mut conn = rig.pool.acquire().await.unwrap();
        orch_db::queries::access::epoch(&mut conn).await.unwrap()
    };
    mode(&rig, &other_token, true).await;
    let epoch_after = {
        let mut conn = rig.pool.acquire().await.unwrap();
        orch_db::queries::access::epoch(&mut conn).await.unwrap()
    };
    assert_eq!(
        epoch_after, epoch_before,
        "switching another session announces personal identity, not shared authority"
    );
    // Identity announcements conservatively recheck all this person's sockets.
    // The first session still has native mode on: its allowed recheck pauses
    // this durable reader with resync. Re-admit it before testing its own Off.
    loop {
        let frame = next_before(
            &mut first,
            transition_end,
            "other session on: first reader's allowed reset",
        )
        .await;
        assert!(
            frame["id"] == 1 && frame["feed"] == thread_feed,
            "the allowed reset belongs to the original first reader"
        );
        if frame["op"] == "presence" {
            continue;
        }
        assert_eq!(
            frame["op"], "resync",
            "an allowed earlier reset is not a revoked closure"
        );
        break;
    }
    let still_first = http.get(&url).bearer_auth(&token).send().await.unwrap();
    assert_eq!(still_first.status(), 200);
    assert!(
        still_first.json::<serde_json::Value>().await.unwrap()["access"]["admin_access"]
            == on["access"]["admin_access"],
        "the exact first session still has its original native administration reach before Off"
    );
    first.sub(1, &thread_feed, None).await;
    let readmitted = loop {
        let frame = next_before(
            &mut first,
            transition_end,
            "first reader re-admitted after allowed reset",
        )
        .await;
        assert!(
            frame["id"] == 1 && frame["feed"] == thread_feed,
            "readmission must answer this exact first reader"
        );
        // A presence broadcast already in flight is neither the new ACK nor
        // an admission result. The real replacement still needs both below.
        if frame["op"] == "presence" {
            continue;
        }
        assert_eq!(frame["op"], "subscribed");
        break frame;
    };
    assert!(
        readmitted["id"] == 1 && readmitted["feed"] == thread_feed && readmitted["mode"] == "delta"
    );
    let presence = expect_before(
        &mut first,
        transition_end,
        "presence",
        "first reader's current re-admitted presence",
    )
    .await;
    assert!(presence["id"] == 1 && presence["feed"] == thread_feed);
    other.sub(3, &format!("thread:{thread}"), None).await;
    let acknowledged = expect_before(
        &mut other,
        transition_end,
        "subscribed",
        "other session on: subscribe",
    )
    .await;
    assert!(
        acknowledged["id"] == 3
            && acknowledged["feed"] == thread_feed
            && acknowledged["mode"] == "delta"
    );
    let presence = expect_before(
        &mut other,
        transition_end,
        "presence",
        "other session on: initial presence",
    )
    .await;
    assert!(presence["id"] == 3 && presence["feed"] == thread_feed);
    mode(&rig, &token, false).await;
    let closed = closed_at(&mut first, transition_end, &thread_feed).await;
    assert_eq!(closed["id"], 1);
    assert_eq!(closed["reason"], "revoked");
    assert!(
        !http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let still_on = http
        .get(&url)
        .bearer_auth(&other_token)
        .send()
        .await
        .unwrap();
    assert!(still_on.status().is_success());
    assert!(
        still_on.json::<serde_json::Value>().await.unwrap()["access"]["admin_access"].is_object()
    );
    other.ping().await;
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_that_is_no_frame_of_the_protocol_closes_the_socket_with_4400() {
    let rig = Rig::new().await;
    for (what, send) in [
        ("text that is not JSON", "this is not JSON".to_owned()),
        ("an object with no op", json!({ "id": 1 }).to_string()),
        (
            "an op the protocol does not have",
            json!({ "op": "mutate" }).to_string(),
        ),
        (
            "a member the frame does not have",
            json!({ "op": "ping", "n": 1, "extra": true }).to_string(),
        ),
        (
            "a member of the wrong type",
            json!({ "op": "ping", "n": "one" }).to_string(),
        ),
        (
            "a frame over what a client may send",
            json!({ "op": "sub", "id": 1, "feed": "x".repeat(20_000) }).to_string(),
        ),
        (
            "a second auth",
            json!({ "op": "auth", "token": "again" }).to_string(),
        ),
    ] {
        let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
        client.send_text(&send).await;
        assert_eq!(client.closed().await, Some(4400), "{what}");
    }
    let mut binary = Client::signed_in(rig.server.addr, &rig.token).await;
    binary.send_binary(b"\x00\x01").await;
    assert_eq!(binary.closed().await, Some(4400), "a binary frame");
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_hears_nothing_for_too_long_is_closed() {
    let rig = Rig::tuned(|tuning| tuning.silence = Duration::from_millis(1_200)).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    assert!(client.closed().await.is_some());
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_pings_is_never_closed_for_silence() {
    let rig = Rig::tuned(|tuning| tuning.silence = Duration::from_millis(1_200)).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        client.ping().await;
    }
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_outlives_its_token_without_reauth_is_closed_with_4401() {
    // The server's time is held: the token runs out when the test says so, not while a busy machine is
    // still signing the connection in.
    let rig = Rig::with_time_held().await;
    let token = rig.token_for(rig.ada, Span::seconds(2)).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.ping().await;
    rig.let_time_pass(Span::seconds(3));
    assert_eq!(client.closed().await, Some(4401));
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reauth_with_a_new_token_keeps_the_connection_and_is_not_answered() {
    let rig = Rig::with_time_held().await;
    let short = rig.token_for(rig.ada, Span::seconds(2)).await;
    let mut client = Client::signed_in(rig.server.addr, &short).await;
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;
    let fresh = rig.token_for(rig.ada, Span::minutes(15)).await;
    client.send(json!({ "op": "reauth", "token": fresh })).await;
    // The next frame is the answer to the ping, not to the reauth: there is none.
    client.send(json!({ "op": "ping", "n": 41 })).await;
    let next = client.frame().await;
    assert_eq!(next["op"], "pong");
    assert_eq!(next["n"], 41);
    // The old token ran out; the connection goes on, with its subscription. The server looks at its
    // clocks every tenth of a second in these tests: it has had five looks when the ping is sent.
    rig.let_time_pass(Span::seconds(3));
    tokio::time::sleep(Duration::from_millis(500)).await;
    client.ping().await;
    assert_eq!(rig.server.realtime.subscriptions(), 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reauth_with_a_token_that_is_not_good_or_is_another_persons_closes_with_4401() {
    let rig = Rig::new().await;
    let (_, bo_token) = rig.person("bobby").await;
    let gone = rig.token_for(rig.ada, Span::minutes(15)).await;
    rig.sign_out(&gone).await;
    for (what, token) in [
        ("nonsense", "oat_nonsense".to_owned()),
        ("another person's", bo_token),
        ("a session that was ended", gone),
    ] {
        let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
        client.send(json!({ "op": "reauth", "token": token })).await;
        assert_eq!(client.closed().await, Some(4401), "{what}");
    }
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_follows_sixty_four_feeds_and_not_a_sixty_fifth() {
    let rig = Rig::new().await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for id in 1..=64 {
        client.sub(id, "user", None).await;
        let subscribed = client.expect("subscribed").await;
        assert_eq!(subscribed["id"], id);
    }
    client.sub(65, "user", None).await;
    let refused = client.expect("error").await;
    assert_eq!(refused["id"], 65);
    assert_eq!(refused["code"], "too_many_subscriptions");
    // Ending one makes room, and the connection was never in danger.
    client.send(json!({ "op": "unsub", "id": 3 })).await;
    client.expect("closed").await;
    client.sub(65, "user", None).await;
    client.expect("subscribed").await;
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_cannot_be_served_is_an_error_and_the_connection_stays_open() {
    let rig = Rig::new().await;
    let thread = rig.thread("Private").await;
    let (_, bo_token) = rig.person("bobby").await;
    let mut bo = Client::signed_in(rig.server.addr, &bo_token).await;

    bo.sub(1, "thread:not-an-id", None).await;
    let bad = bo.expect("error").await;
    assert_eq!(
        (bad["id"].as_i64(), bad["code"].as_str()),
        (Some(1), Some("bad_feed"))
    );

    bo.sub(2, &format!("thread:{thread}"), None).await;
    let forbidden = bo.expect("error").await;
    assert_eq!(
        (forbidden["id"].as_i64(), forbidden["code"].as_str()),
        (Some(2), Some("forbidden"))
    );

    bo.sub(3, &format!("thread:{}", new_id()), None).await;
    let missing = bo.expect("error").await;
    assert_eq!(
        (missing["id"].as_i64(), missing["code"].as_str()),
        (Some(3), Some("not_found"))
    );

    // A feed this server has no data for is not found, whoever asks.
    bo.sub(4, &format!("chat:{}", new_id()), None).await;
    let chat = bo.expect("error").await;
    assert_eq!(
        (chat["id"].as_i64(), chat["code"].as_str()),
        (Some(4), Some("not_found"))
    );
    // The processes of an environment are the environment's own feed, which this server has no data
    // for yet: not found, whoever asks. There is none through a thread: that is no feed at all.
    bo.sub(5, &format!("env:{}:procs", new_id()), None).await;
    assert_eq!(bo.expect("error").await["code"], "not_found");
    bo.sub(7, &format!("thread:{thread}:procs"), None).await;
    let through_a_thread = bo.expect("error").await;
    assert_eq!(
        (
            through_a_thread["id"].as_i64(),
            through_a_thread["code"].as_str()
        ),
        (Some(7), Some("bad_feed"))
    );
    bo.sub(8, &format!("thread:{thread}:dir:c3Jj"), None).await;
    assert_eq!(bo.expect("error").await["code"], "bad_feed");

    // A visible for a feed that is no feed is an error without a number.
    bo.send(json!({ "op": "visible", "feed": "nothing", "visible": true }))
        .await;
    let visible = bo.expect("error").await;
    assert_eq!(visible["code"], "bad_feed");
    assert!(visible.get("id").is_none() || visible["id"].is_null());

    // The connection is open and serves the next request.
    bo.sub(6, "user", None).await;
    assert_eq!(bo.expect("subscribed").await["id"], 6);
    assert_eq!(rig.server.realtime.subscriptions(), 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sub_that_reuses_a_number_takes_the_place_of_the_subscription_without_a_closed() {
    let rig = Rig::new().await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;
    client.sub(1, "user", None).await;
    // The answer is a `subscribed`: a `closed` for the number would end the new one in the client's books.
    let again = client.expect("subscribed").await;
    assert_eq!(again["id"], 1);
    assert_eq!(rig.server.realtime.subscriptions(), 1);
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    // The same number for another feed takes its place too.
    let thread = rig.thread("Elsewhere").await;
    client.sub(1, &format!("thread:{thread}"), None).await;
    let moved = client.expect("subscribed").await;
    assert_eq!(moved["feed"], format!("thread:{thread}"));
    client.expect("presence").await;
    assert_eq!(rig.server.realtime.subscriptions(), 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_feed_under_two_numbers_is_followed_twice_and_ended_one_at_a_time() {
    let rig = Rig::new().await;
    let thread = rig.thread("Twice").await;
    let now = rig.rows(thread).await.seq;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    let feed = format!("thread:{thread}");
    client.sub(1, &feed, Some(now)).await;
    client.sub(2, &feed, Some(now)).await;
    let mut subscribed = Vec::new();
    while subscribed.len() < 2 {
        let frame = client.frame().await;
        if frame["op"] == "subscribed" {
            subscribed.push(frame["id"].as_i64().unwrap());
        }
    }
    subscribed.sort_unstable();
    assert_eq!(subscribed, [1, 2]);
    assert_eq!(rig.server.realtime.subscriptions(), 2);

    // Each is sent what happens, under its own number.
    let turn = rig.send(thread, "Hello").await;
    rig.begin_message(thread, &turn, "m1").await;
    let mut heard_by = std::collections::BTreeSet::new();
    let started = std::time::Instant::now();
    while heard_by.len() < 2 && started.elapsed() < Duration::from_secs(10) {
        if let Some(realtime_support::Heard::Frame(frame)) =
            client.hear_within(Duration::from_millis(200)).await
            && frame["op"] == "sync"
        {
            heard_by.insert(frame["id"].as_i64().unwrap());
        }
    }
    assert_eq!(heard_by.into_iter().collect::<Vec<_>>(), [1, 2]);

    // One ended, the other goes on.
    client.send(json!({ "op": "unsub", "id": 1 })).await;
    let closed = loop {
        let frame = client.frame().await;
        if frame["op"] == "closed" {
            break frame;
        }
    };
    assert_eq!(
        (closed["id"].as_i64(), closed["reason"].as_str()),
        (Some(1), Some("unsubscribed"))
    );
    assert_eq!(rig.server.realtime.subscriptions(), 1);
    rig.finish_turn(thread, &turn, "m1", "Done.").await;
    let mut still_heard = false;
    let started = std::time::Instant::now();
    while !still_heard && started.elapsed() < Duration::from_secs(10) {
        if let Some(realtime_support::Heard::Frame(frame)) =
            client.hear_within(Duration::from_millis(200)).await
        {
            assert_ne!(
                frame["id"], 1,
                "{frame}: nothing is sent on an ended subscription"
            );
            still_heard = frame["op"] == "sync" && frame["id"] == 2;
        }
    }
    assert!(still_heard);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsub_is_answered_once_whether_or_not_the_subscription_was_open() {
    let rig = Rig::new().await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;
    client.send(json!({ "op": "unsub", "id": 1 })).await;
    let closed = client.expect("closed").await;
    assert_eq!(
        (
            closed["id"].as_i64(),
            closed["feed"].as_str(),
            closed["reason"].as_str()
        ),
        (Some(1), Some("user"), Some("unsubscribed"))
    );
    // A number that is not open, or was ended, is an error that names it.
    client.send(json!({ "op": "unsub", "id": 1 })).await;
    let again = client.expect("error").await;
    assert_eq!(
        (again["id"].as_i64(), again["code"].as_str()),
        (Some(1), Some("not_found"))
    );
    client.send(json!({ "op": "unsub", "id": 99 })).await;
    assert_eq!(client.expect("error").await["id"], 99);
    assert_eq!(rig.server.realtime.subscriptions(), 0);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_ends_leaves_nothing_behind() {
    let rig = Rig::new().await;
    let thread = rig.thread("Left").await;
    {
        let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
        client.sub(1, "user", None).await;
        client.sub(2, &format!("thread:{thread}"), None).await;
        client.until("presence").await;
        assert_eq!(rig.server.realtime.connections(), 1);
        assert_eq!(rig.server.realtime.subscriptions(), 2);
    }
    realtime_support::eventually("the connection is forgotten", || async {
        rig.server.realtime.connections() == 0 && rig.server.realtime.subscriptions() == 0
    })
    .await;
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_a_line_with_a_round_trip_of_300_milliseconds_everything_completes_and_the_round_trip_is_measured()
 {
    let rig = Rig::new().await;
    let line = SlowLine::shaped(rig.server.addr, Duration::from_millis(150), Some(200_000)).await;
    let mut client = Client::connect(line.addr).await;
    // Authentication, a subscription, a new token and the pings all complete.
    let started = std::time::Instant::now();
    client.auth(&rig.token).await;
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "the answer to auth cannot come sooner than a round trip"
    );
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;
    let fresh = rig.token_for(rig.ada, Span::minutes(15)).await;
    client.send(json!({ "op": "reauth", "token": fresh })).await;
    let mut round_trips = Vec::new();
    for _ in 0..5 {
        round_trips.push(client.ping().await);
    }
    round_trips.sort_unstable();
    let middle = round_trips[round_trips.len() / 2];
    assert!(
        (240..=360).contains(&middle),
        "the measured round trip was {middle} ms: {round_trips:?}"
    );
    assert_eq!(
        rig.server.realtime.subscriptions(),
        1,
        "nothing was closed for lateness"
    );
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_metadata_patch_has_one_canonical_stamp_and_a_stale_patch_rolls_back_without_changing_its_lane()
 {
    // This counter/CAS control owns no supervisor work; the live two-session
    // marker test above retains the original normally running engine.
    let rig = Rig::with_unstarted_engine().await;
    let thread = rig.thread("Metadata counter control").await;
    let read = || {
        sqlx::query_as::<_, (i64, i64, orch_types::Id, Option<orch_types::Id>, Option<orch_types::Id>)>(
        "SELECT next_seq-1,seq_state,active_continuation_id,current_turn_id,current_segment_id FROM threads WHERE node_id=$1")
        .bind(thread).fetch_one(rig.pool.pool())
    };
    let before = read().await.unwrap();
    let http = reqwest::Client::new();
    let url = format!("http://{}/api/v1/nodes/{thread}", rig.server.addr);
    let shown = http
        .get(&url)
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let changed = http
        .patch(&url)
        .bearer_auth(&rig.token)
        .json(&json!({"name":"Metadata renamed","expected_revision":shown["revision"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), 200);
    let changed = changed.json::<serde_json::Value>().await.unwrap();
    let committed = read().await.unwrap();
    assert_eq!(committed.0, before.0 + 1);
    assert_eq!(committed.1, committed.0);
    assert_eq!(
        (committed.2, committed.3, committed.4),
        (before.2, before.3, before.4)
    );
    let stale = http
        .patch(&url)
        .bearer_auth(&rig.token)
        .json(&json!({"name":"Must not commit","expected_revision":shown["revision"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    let stale = stale.json::<serde_json::Value>().await.unwrap();
    assert_eq!(stale["code"], "conflict.stale_revision");
    assert_eq!(stale["current"]["revision"], changed["revision"]);
    assert_eq!(read().await.unwrap(), committed);
    rig.shutdown().await;
}
