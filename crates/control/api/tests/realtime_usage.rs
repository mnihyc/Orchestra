//! Model-request feeds use the same scope and per-record access cuts as historical usage records.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use chrono::Duration as Span;
use orch_realtime_contract::feeds::Feed;
use orch_types::{Id, new_id};
use realtime_support::{Client, Rig};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_usage_subscription_and_its_first_revision_keep_the_socket_open() {
    let rig = Rig::new().await;
    let thread = rig.thread("Requests after an empty subscription").await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "usage:me", None).await;
    client.expect("subscribed").await;
    // Even an empty snapshot must query running requests without binding Chrono's minimum date.
    assert!(ids(&client.expect("event").await).is_empty());
    for _ in 0..2 {
        let id = record(&rig, rig.ada, thread).await;
        assert_eq!(
            ids(&client.expect("event").await),
            vec![id.to_string()],
            "revision-only reads must not bind either timestamp extremum"
        );
    }
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_requests_send_no_frames_and_new_rows_are_not_full_snapshots() {
    let rig = Rig::new().await;
    let thread = rig.thread("Incremental requests").await;
    let old = record(&rig, rig.ada, thread).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "usage:me", None).await;
    client.expect("subscribed").await;
    assert_eq!(ids(&client.expect("event").await), vec![old.to_string()]);
    rig.server
        .realtime
        .publish_list(Feed::Usage("me".into()), wake())
        .await
        .unwrap();
    assert!(
        client
            .is_quiet_for(std::time::Duration::from_millis(200))
            .await
    );
    let fresh = record(&rig, rig.ada, thread).await;
    let received = ids(&client.expect("event").await);
    assert_eq!(received, vec![fresh.to_string()]);
    assert!(!received.contains(&old.to_string()));
    rig.server
        .realtime
        .publish_list(Feed::Usage("me".into()), wake())
        .await
        .unwrap();
    assert!(
        client
            .is_quiet_for(std::time::Duration::from_millis(200))
            .await
    );
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_org_filter_is_applied_before_a_live_row_leaves_the_server() {
    let rig = Rig::new().await;
    let thread = rig.thread("Org requests").await;
    let _ = record(&rig, rig.ada, thread).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &format!("usage:me:org:{}", new_id()), None)
        .await;
    client.expect("subscribed").await;
    assert!(ids(&client.expect("event").await).is_empty());
    let _ = record(&rig, rig.ada, thread).await;
    assert!(
        client
            .is_quiet_for(std::time::Duration::from_millis(1200))
            .await
    );
    rig.server.stop().await;
}

async fn record(rig: &Rig, user: Id, thread: Id) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO usage_events(id,at,origin,purpose,scope,counted,org_id,project_id,topic_id,thread_id,provider_id,user_id,model)
        VALUES($1,$2,'harness','turn','request',true,$3,$4,$5,$6,$7,$8,'fake-1')")
        .bind(id).bind(rig.clock.now()).bind(rig.root).bind(rig.project).bind(rig.topic).bind(thread).bind(rig.provider).bind(user)
        .execute(rig.pool.pool()).await.unwrap();
    id
}

/// What a test publishes to a usage feed: it only wakes the feed, whose readers are each sent their own
/// rows, and it is still a page of what the feed carries, as everything published to a list is.
fn wake() -> Value {
    json!({"items": [], "next_cursor": null})
}

fn ids(event: &Value) -> Vec<String> {
    event["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_health_is_scoped_lightweight_and_unchanged_figures_send_nothing() {
    let rig = Rig::new().await;
    let mut owner = Client::signed_in(rig.server.addr, &rig.token).await;
    let feed = format!("usage:provider:{}:health", rig.provider);
    owner.sub(1, &feed, None).await;
    owner.expect("subscribed").await;
    let first = owner.expect("event").await;
    assert!(first["data"]["health"]["recent"].is_object());
    for key in ["items", "series", "comparison", "breakdowns"] {
        assert!(first["data"].get(key).is_none());
    }
    rig.server
        .realtime
        .publish_list(
            Feed::Usage(format!("provider:{}:health", rig.provider)),
            first["data"].clone(),
        )
        .await
        .unwrap();
    assert!(
        owner
            .is_quiet_for(std::time::Duration::from_millis(200))
            .await
    );
    let (_, token) = rig.person("unrelated-health-reader").await;
    let mut stranger = Client::signed_in(rig.server.addr, &token).await;
    stranger.sub(1, &feed, None).await;
    assert_eq!(stranger.expect("error").await["code"], "forbidden");
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_small_personal_notice_delivers_its_native_value_only_to_its_owner() {
    let rig = Rig::new().await;
    let mut owner = Client::signed_in(rig.server.addr, &rig.token).await;
    let (_, token) = rig.person("another-device-owner").await;
    let mut stranger = Client::signed_in(rig.server.addr, &token).await;
    for client in [&mut owner, &mut stranger] {
        client.sub(1, "user", None).await;
        client.expect("subscribed").await;
    }
    // A large saved view must not be put in PostgreSQL's sub-8KiB notification.
    let value = json!({"layout":"saved view ".repeat(1000)});
    {
        let mut conn = rig.pool.acquire().await.unwrap();
        orch_db::queries::identity::put_interface_state(
            &mut conn,
            rig.ada,
            "layouts",
            &value,
            rig.clock.now(),
        )
        .await
        .unwrap();
        assert!(
            orch_db::sequences::notify_user_event(
                &mut conn,
                &[rig.ada],
                &json!({"what":"state","key":"layouts"})
            )
            .await
            .unwrap()
        );
    }
    let event = owner.expect("event").await;
    assert_eq!(event["data"]["what"], "state");
    assert_eq!(event["data"]["value"], value);
    assert!(
        stranger
            .is_quiet_for(std::time::Duration::from_millis(200))
            .await
    );
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_summary_subscriber_receives_only_their_own_rows_and_an_unrelated_viewer_receives_none() {
    let rig = Rig::new().await;
    let thread = rig.thread("A request log").await;
    let (viewer, token) = rig.person("viewer").await;
    rig.give(viewer, "viewer").await;
    let other = new_id();
    {
        let mut conn = rig.pool.acquire().await.unwrap();
        orch_db::queries::identity::testing::insert_user(&mut conn, other, "other")
            .await
            .unwrap();
    }
    let other_token = rig.token_for(other, Span::days(1)).await;
    let (other_project, other_topic, other_thread) = (new_id(), new_id(), new_id());
    let foreign_row = {
        use orch_db::queries::{
            access::testing as access_testing, hierarchy::testing::insert_node,
        };
        let mut conn = rig.pool.acquire().await.unwrap();
        insert_node(
            &mut conn,
            other_project,
            "project",
            Some(rig.root),
            "Another project",
            rig.ada,
        )
        .await
        .unwrap();
        insert_node(
            &mut conn,
            other_topic,
            "topic",
            Some(other_project),
            "Another place",
            rig.ada,
        )
        .await
        .unwrap();
        insert_node(
            &mut conn,
            other_thread,
            "thread",
            Some(other_topic),
            "Another thread",
            rig.ada,
        )
        .await
        .unwrap();
        access_testing::give_role(
            &mut conn,
            other_topic,
            "user",
            other,
            "viewer",
            rig.ada,
            None,
        )
        .await
        .unwrap();
        let id = new_id();
        sqlx::query("INSERT INTO usage_events(id,at,origin,purpose,scope,counted,org_id,project_id,topic_id,thread_id,provider_id,user_id,model) VALUES($1,$2,'harness','turn','request',true,$3,$4,$5,$6,$7,$8,'fake-1')")
            .bind(id).bind(rig.clock.now()).bind(rig.root).bind(other_project).bind(other_topic).bind(other_thread).bind(rig.provider).bind(viewer).execute(&mut *conn).await.unwrap();
        id
    };
    let owners_row = record(&rig, rig.ada, thread).await;
    let viewers_row = record(&rig, viewer, thread).await;
    let feed = Feed::Usage(format!("node:{}", rig.topic));
    let mut owner = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut summary = Client::signed_in(rig.server.addr, &token).await;
    let mut stranger = Client::signed_in(rig.server.addr, &other_token).await;
    for client in [&mut owner, &mut summary] {
        client.sub(1, &feed.to_string(), None).await;
        assert_eq!(client.expect("subscribed").await["mode"], "ephemeral");
    }
    stranger.sub(1, &feed.to_string(), None).await;
    assert_eq!(stranger.expect("error").await["code"], "forbidden");
    rig.server
        .realtime
        .publish_list(feed, wake())
        .await
        .unwrap();
    let received = ids(&owner.expect("event").await);
    assert!(received.contains(&owners_row.to_string()));
    assert!(received.contains(&viewers_row.to_string()));
    assert!(
        !received.contains(&foreign_row.to_string()),
        "a reader of one place receives no row from another place"
    );
    assert_eq!(
        ids(&summary.expect("event").await),
        vec![viewers_row.to_string()]
    );
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_own_scope_never_receives_another_persons_requests() {
    let rig = Rig::new().await;
    let thread = rig.thread("Own requests").await;
    let (viewer, token) = rig.person("viewer").await;
    let _ = record(&rig, rig.ada, thread).await;
    let own = record(&rig, viewer, thread).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub(1, "usage:me", None).await;
    client.expect("subscribed").await;
    rig.server
        .realtime
        .publish_list(Feed::Usage("me".to_owned()), wake())
        .await
        .unwrap();
    assert_eq!(ids(&client.expect("event").await), vec![own.to_string()]);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn losing_scope_access_closes_the_feed_before_another_row_can_be_sent() {
    let rig = Rig::new().await;
    let (viewer, token) = rig.person("viewer").await;
    let role = rig.give(viewer, "viewer").await;
    let feed = Feed::Usage(format!("node:{}", rig.topic));
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    // The feed first sends what it holds now, as every usage subscription does; only then is the place taken.
    assert!(ids(&client.expect("event").await).is_empty());
    rig.take(role).await;
    assert_eq!(client.expect("closed").await["reason"], "revoked");
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_users_only_receive_viewable_places_while_managers_receive_redacted_foreign_rows()
{
    use orch_db::queries::{
        access::{PageKey, testing as access},
        hierarchy::testing::insert_node,
    };
    let rig = Rig::new().await;
    let thread = rig.thread("Visible provider work").await;
    let (viewer, token) = rig.person("provider-reader").await;
    let role = rig.give(viewer, "viewer").await;
    let (manager, manager_token) = rig.person("provider-manager").await;
    let (elsewhere, elsewhere_token) = rig.person("elsewhere-reader").await;
    let (foreign_org, foreign_project, foreign_topic, foreign_thread) =
        (new_id(), new_id(), new_id(), new_id());
    let foreign = new_id();
    {
        let mut conn = rig.pool.acquire().await.unwrap();
        insert_node(&mut conn, foreign_org, "org", None, "Private org", rig.ada)
            .await
            .unwrap();
        insert_node(
            &mut conn,
            foreign_project,
            "project",
            Some(foreign_org),
            "Private project",
            rig.ada,
        )
        .await
        .unwrap();
        insert_node(
            &mut conn,
            foreign_topic,
            "topic",
            Some(foreign_project),
            "Private topic",
            rig.ada,
        )
        .await
        .unwrap();
        insert_node(
            &mut conn,
            foreign_thread,
            "thread",
            Some(foreign_topic),
            "Private thread",
            rig.ada,
        )
        .await
        .unwrap();
        access::give_role(
            &mut conn,
            foreign_topic,
            "user",
            elsewhere,
            "viewer",
            rig.ada,
            None,
        )
        .await
        .unwrap();
        for user in [viewer, manager, elsewhere] {
            access::add_entry(
                &mut conn,
                &PageKey::new("provider", rig.provider, "live"),
                "any",
                "provider.use",
                ("user", Some(user), None),
                None,
                rig.clock.now(),
            )
            .await
            .unwrap();
        }
        access::add_entry(
            &mut conn,
            &PageKey::new("provider", rig.provider, "live"),
            "any",
            "provider.manage",
            ("user", Some(manager), None),
            None,
            rig.clock.now(),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO usage_events(id,at,origin,purpose,scope,counted,org_id,project_id,topic_id,thread_id,provider_id,user_id,model) VALUES($1,$2,'harness','turn','request',true,$3,$4,$5,$6,$7,$8,'fake-1')")
            .bind(foreign).bind(rig.clock.now()).bind(foreign_org).bind(foreign_project).bind(foreign_topic).bind(foreign_thread).bind(rig.provider).bind(rig.ada).execute(&mut *conn).await.unwrap();
    }
    let own = record(&rig, rig.ada, thread).await;
    let feed = format!("usage:provider:{}", rig.provider);
    let mut reader = Client::signed_in(rig.server.addr, &token).await;
    reader.sub(1, &feed, None).await;
    reader.expect("subscribed").await;
    assert_eq!(ids(&reader.expect("event").await), vec![own.to_string()]);
    let mut other = Client::signed_in(rig.server.addr, &elsewhere_token).await;
    other.sub(1, &feed, None).await;
    other.expect("subscribed").await;
    assert_eq!(
        ids(&other.expect("event").await),
        vec![foreign.to_string()],
        "a viewer of another place receives none of this place's rows"
    );
    let mut managing = Client::signed_in(rig.server.addr, &manager_token).await;
    managing.sub(1, &feed, None).await;
    managing.expect("subscribed").await;
    let all = managing.expect("event").await;
    assert_eq!(ids(&all).len(), 2);
    assert!(
        all["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["thread"]["redacted"] == true && row["thread"]["name"].is_null())
    );
    let next = record(&rig, rig.ada, thread).await;
    assert_eq!(ids(&reader.expect("event").await), vec![next.to_string()]);
    assert_eq!(ids(&managing.expect("event").await), vec![next.to_string()]);
    assert!(
        other
            .is_quiet_for(std::time::Duration::from_millis(1200))
            .await
    );
    rig.take(role).await;
    let _ = record(&rig, rig.ada, thread).await;
    // Taking the place away changes who may do what, so the feed asks its reader to load again
    // (`resync` with `reload`); no row of that place comes after it.
    let mut heard = Vec::new();
    while let Some(realtime_support::Heard::Frame(frame)) = reader
        .hear_within(std::time::Duration::from_millis(1200))
        .await
    {
        heard.push(frame);
    }
    assert!(
        heard.iter().all(|frame| frame["op"] != "event"),
        "provider use alone cannot deliver new rows after place access is lost: {heard:?}"
    );
    rig.server.stop().await;
}
