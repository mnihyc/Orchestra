//! The real shell composes only its own declared sources and reports a device zone once.

use super::{
    authority::{home, rule},
    checked, key, wait_for_activity,
};
use crate::{chat_world::Room, places, support::World};
use http::StatusCode;
use orch_api_contract::auth::MeBootstrapGet;
use orch_api_contract::chats::{ChatLevel, ChatStateSet, ChatStateSetBody};
use orch_api_contract::common::SubscriptionLevel;
use orch_api_contract::nodes::{NodeSubscriptionSet, NodeSubscriptionSetBody};
use orch_api_contract::personal::{MeFavoriteSet, MeFavoriteSetBody, MeStateSet};
use orch_types::{Id, new_id};
use serde_json::json;

async fn own_data(room: &Room) -> Id {
    let app = &room.world.app;
    app.call(MeStateSet, &[&"sidebar"])
        .token(&room.ada.token)
        .body(&json!({"native_saved":"Own shell layout"}))
        .send()
        .await
        .ok();
    app.call(MeFavoriteSet, &[&room.project])
        .token(&room.ada.token)
        .body(&MeFavoriteSetBody {
            after_node_id: None,
            view: None,
        })
        .send()
        .await
        .ok();
    // Native node chats accept a bell level; hidden belongs to direct/group chats only.
    app.call(ChatStateSet, &[&room.chat])
        .token(&room.ada.token)
        .body(&ChatStateSetBody {
            level: Some(Some(ChatLevel::All)),
            hidden: None,
        })
        .send()
        .await
        .ok();
    app.call(NodeSubscriptionSet, &[&room.project])
        .token(&room.ada.token)
        .body(&NodeSubscriptionSetBody {
            level: SubscriptionLevel::All,
        })
        .send()
        .await
        .ok();
    let thread = new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut *room.world.conn().await,
        thread,
        "thread",
        Some(room.topic),
        "Own retained position",
        room.ada.id(),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO thread_reads(user_id,thread_id,last_read_seq) VALUES($1,$2,17)")
        .bind(room.ada.id())
        .bind(thread)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    thread
}

#[tokio::test]
async fn bootstrap_account_state_cannot_lend_personal_list_or_other_operation_reads() {
    let room = Room::new().await;
    let thread = own_data(&room).await;
    let native = room
        .world
        .app
        .call(MeBootstrapGet, &[])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert!(
        native
            .favorites
            .iter()
            .any(|favorite| favorite.node.id == room.project)
    );
    assert!(native.read_states.iter().any(|row| row.thread_id == thread));
    assert!(
        native
            .chat_states
            .iter()
            .any(|row| row.chat_id == room.chat)
    );
    assert!(
        native
            .subscriptions
            .iter()
            .any(|row| row.node_id == room.project)
    );
    for extra in [
        Vec::new(),
        vec![
            rule("MeFavoriteList", "project", room.project),
            rule("MeReadStateList", "thread", thread),
            rule("MeSubscriptionList", "project", room.project),
            rule("MeChatStateList", "chat", room.chat),
            home("MeAuthorityGet", room.root),
        ],
    ] {
        let token = key(
            &room.world,
            room.ada.id(),
            std::iter::once(rule("MeBootstrapGet", "account", room.ada.id()))
                .chain(extra)
                .collect(),
        )
        .await;
        let reply = room
            .world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .send_bytes()
            .await;
        assert_eq!(reply.headers[http::header::CACHE_CONTROL], "no-store");
        let body = checked(reply, StatusCode::OK);
        assert_eq!(
            body["person"],
            serde_json::to_value(&native.person).unwrap()
        );
        assert_eq!(body["state"]["sidebar"]["native_saved"], "Own shell layout");
        assert_eq!(
            body["notification_summary"],
            serde_json::to_value(&native.notification_summary).unwrap()
        );
        for field in [
            "orgs",
            "favorites",
            "chat_states",
            "read_states",
            "subscriptions",
        ] {
            assert_eq!(body[field], json!([]), "{field}");
        }
    }
    let token = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("MeBootstrapGet", "account", room.ada.id()),
            home("MeBootstrapGet", room.root),
            home("MeBootstrapGet", room.ada.person.personal_org),
        ],
    )
    .await;
    let body = checked(
        room.world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body, serde_json::to_value(native).unwrap());
}

#[tokio::test]
async fn bootstrap_favorite_names_and_chat_state_keep_their_native_independent_sources() {
    let room = Room::new().await;
    let thread = own_data(&room).await;
    let token = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("MeBootstrapGet", "account", room.ada.id()),
            rule("MeBootstrapGet", "project", room.project),
            rule("MeBootstrapGet", "thread", thread),
            rule("MeBootstrapGet", "chat", room.chat),
            rule("NodeGet.read", "org", room.root),
        ],
    )
    .await;
    let body = checked(
        room.world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["orgs"], json!([]));
    assert_eq!(body["favorites"].as_array().unwrap().len(), 1);
    assert_eq!(body["favorites"][0]["node"]["name"], "Web");
    assert!(body["favorites"][0]["path"][0]["name"].is_null());
    assert_eq!(body["favorites"][0]["path"][0]["redacted"], true);
    assert_eq!(body["read_states"].as_array().unwrap().len(), 1);
    assert_eq!(body["read_states"][0]["thread_id"], json!(thread));
    assert_eq!(body["read_states"][0]["last_read_seq"], 17);
    assert_eq!(body["chat_states"].as_array().unwrap().len(), 1);
    assert_eq!(body["chat_states"][0]["chat_id"], json!(room.chat));
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);
    assert_eq!(body["subscriptions"][0]["node_id"], json!(room.project));
}

async fn count_reports(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM bootstrap_report_observations")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn observe_reports(world: &World) {
    // Isolated native database fixture; observe the genuine update, not a mocked renderer call.
    sqlx::query("CREATE TABLE bootstrap_report_observations(user_id uuid NOT NULL)")
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("CREATE FUNCTION observe_bootstrap_report() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO bootstrap_report_observations(user_id) VALUES(NEW.id); RETURN NEW; END $$").execute(&world.db.pool).await.unwrap();
    sqlx::query("CREATE TRIGGER observe_bootstrap_report AFTER UPDATE OF timezone_reported_at ON users FOR EACH ROW EXECUTE FUNCTION observe_bootstrap_report()").execute(&world.db.pool).await.unwrap();
}

#[tokio::test]
async fn bootstrap_reports_only_after_its_actual_account_read_and_preserves_fixed_and_invalid_zones()
 {
    let world = World::new().await;
    let actor = places::person(&world, "bootstrap-report").await;
    let other = places::person(&world, "bootstrap-other").await;
    observe_reports(&world).await;
    for rules in [
        vec![rule("MeGet", "account", actor.id())],
        vec![rule("MeBootstrapGet", "account", other.id())],
        vec![home("MeBootstrapGet", actor.person.personal_org)],
    ] {
        let token = key(&world, actor.id(), rules).await;
        checked(
            world
                .app
                .call(MeBootstrapGet, &[])
                .token(&token)
                .header("x-orch-timezone", "Asia/Tokyo")
                .send_bytes()
                .await,
            StatusCode::FORBIDDEN,
        );
        assert_eq!(count_reports(&world).await, 0);
    }
    let token = key(
        &world,
        actor.id(),
        vec![rule("MeBootstrapGet", "account", actor.id())],
    )
    .await;
    let body = checked(
        world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .header("x-orch-timezone", "Asia/Tokyo")
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["person"]["timezone"], "Asia/Tokyo");
    assert_eq!(count_reports(&world).await, 1);
    let body = checked(
        world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .header("x-orch-timezone", "Nowhere/Invalid")
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["person"]["timezone"], "Asia/Tokyo");
    assert_eq!(count_reports(&world).await, 1);
    sqlx::query("UPDATE users SET timezone='UTC',timezone_mode='fixed' WHERE id=$1")
        .bind(actor.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let body = checked(
        world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .header("x-orch-timezone", "Europe/Berlin")
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["person"]["timezone"], "UTC");
    assert_eq!(body["person"]["timezone_mode"], "fixed");
    assert_eq!(count_reports(&world).await, 1);
}

async fn bootstrap_expiry_before_first_report(scoped_key: bool) {
    let world = World::new().await;
    let actor = places::person(&world, "bootstrap-before-report").await;
    observe_reports(&world).await;
    let token = if scoped_key {
        key(
            &world,
            actor.id(),
            vec![rule("MeBootstrapGet", "account", actor.id())],
        )
        .await
    } else {
        // A scoped key also holds SHARE on its user while restoring its current
        // boundary. A browser reaches the later native preference-row wait.
        let changed = sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
            .bind(orch_api_core::middleware::authentication::token_hash(
                &actor.token,
            ))
            .bind(world.now() + chrono::Duration::hours(1))
            .execute(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(changed.rows_affected(), 1);
        actor.token.clone()
    };
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(actor.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .header("x-orch-timezone", "Asia/Tokyo");
        call.send_bytes().await
    });
    let query = if scoped_key {
        "%SELECT t.scopes,t.node_id,t.operation_scope,t.expires_at FROM api_tokens t%FOR SHARE OF t,u%"
    } else {
        "%SELECT timezone, timezone_mode FROM users%FOR NO KEY UPDATE%"
    };
    let observed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if request.is_finished() { return false; }
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(query).fetch_one(&world.db.pool).await.unwrap();
            if blocked { return true; } tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false);
    if !observed {
        blocker.rollback().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request).await;
        panic!("the exact native pre-report wait was not reached: {reply:?}");
    }
    assert_eq!(count_reports(&world).await, 0);
    world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(count_reports(&world).await, 0);
    let zone: String = sqlx::query_scalar("SELECT timezone FROM users WHERE id=$1")
        .bind(actor.id())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(zone, "UTC");
}

#[tokio::test]
async fn bootstrap_expiry_after_the_native_preference_wait_prevents_the_first_report_update() {
    bootstrap_expiry_before_first_report(false).await;
}

#[tokio::test]
async fn bootstrap_expiry_after_the_native_key_restore_wait_prevents_the_first_report_update() {
    bootstrap_expiry_before_first_report(true).await;
}

#[tokio::test]
async fn bootstrap_publication_expiry_keeps_the_completed_report_once_and_refuses_the_old_shell() {
    let world = World::new().await;
    let actor = places::person(&world, "bootstrap-after-report").await;
    observe_reports(&world).await;
    let token = key(
        &world,
        actor.id(),
        vec![rule("MeBootstrapGet", "account", actor.id())],
    )
    .await;
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .header("x-orch-timezone", "Asia/Tokyo");
        call.send_bytes().await
    });
    wait_for_activity(&world, pid, &request).await;
    assert_eq!(
        count_reports(&world).await,
        1,
        "the once-only initial step already committed"
    );
    world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("\"state\""));
    assert_eq!(count_reports(&world).await, 1);
    let zone: String = sqlx::query_scalar("SELECT timezone FROM users WHERE id=$1")
        .bind(actor.id())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        zone, "Asia/Tokyo",
        "completed auxiliary work is not undone by later read loss"
    );
}

#[tokio::test]
async fn bootstrap_serializes_a_current_key_identity_without_replaying_the_device_report() {
    let world = World::new().await;
    let actor = places::person(&world, "bootstrap-read-retry").await;
    observe_reports(&world).await;
    let token = key(
        &world,
        actor.id(),
        vec![rule("MeBootstrapGet", "account", actor.id())],
    )
    .await;
    let next_token = token.clone();
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE favorites IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .header("x-orch-timezone", "Asia/Tokyo");
        call.send_bytes().await
    });
    let reading = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if request.is_finished() { return None; }
            let blocked: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM favorites%' LIMIT 1").bind(pid).fetch_optional(&world.db.pool).await.unwrap();
            if blocked.is_some() { return blocked; } tokio::task::yield_now().await;
        }
    }).await.ok().flatten();
    let Some(reading) = reading else {
        blocker.rollback().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request).await;
        panic!("the actual favorites read did not wait: {reply:?}");
    };
    assert_eq!(count_reports(&world).await, 1);
    // Restoring a current key holds SHARE on its user, so even a non-key name
    // update must wait. It cannot create a stale identity inside this snapshot.
    let pool = world.db.pool.clone();
    let actor_id = actor.id();
    let update = tokio::spawn(async move {
        sqlx::query("UPDATE users SET display_name='After the actual identity wait' WHERE id=$1")
            .bind(actor_id)
            .execute(&pool)
            .await
    });
    let waiting = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if update.is_finished() { return false; }
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%UPDATE users SET display_name=%After the actual identity wait%')")
                .bind(reading).fetch_one(&world.db.pool).await.unwrap();
            if blocked { return true; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false);
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    let cache_control = reply.headers.get(http::header::CACHE_CONTROL).cloned();
    let body = checked(reply, StatusCode::OK);
    assert_eq!(
        cache_control.as_ref().and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let updated = tokio::time::timeout(std::time::Duration::from_secs(30), update)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        waiting,
        "the current key identity must hold its actual user row"
    );
    assert_eq!(updated.rows_affected(), 1);
    assert_eq!(
        body["person"]["display_name"], "bootstrap-read-retry",
        "the published body belongs to the original coherent snapshot"
    );
    let body = checked(
        world
            .app
            .call(MeBootstrapGet, &[])
            .token(&next_token)
            .header("x-orch-timezone", "Asia/Tokyo")
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(
        body["person"]["display_name"], "After the actual identity wait",
        "the next producing snapshot contains the changed native person"
    );
    assert_eq!(body["person"]["timezone"], "Asia/Tokyo");
    assert_eq!(count_reports(&world).await, 1);
}

#[tokio::test]
async fn bootstrap_publication_rechecks_its_actual_org_source_after_the_activity_wait() {
    let room = Room::new().await;
    let token = key(
        &room.world,
        room.bea.id(),
        vec![
            rule("MeBootstrapGet", "account", room.bea.id()),
            home("MeBootstrapGet", room.root),
        ],
    )
    .await;
    let control = checked(
        room.world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(control["orgs"][0]["id"], json!(room.root));
    let mut blocker = room.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = room.world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(MeBootstrapGet, &[]).token(&token);
        call.send_bytes().await
    });
    wait_for_activity(&room.world, pid, &request).await;
    sqlx::query("DELETE FROM org_members WHERE org_id=$1 AND user_id=$2")
        .bind(room.root)
        .bind(room.bea.id())
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("\"orgs\""));
}

#[tokio::test]
async fn bootstrap_favorite_publication_cannot_borrow_another_favorites_old_path() {
    let world = World::new().await;
    let actor = places::person(&world, "bootstrap-favorite-path").await;
    let root = places::make_org(&world, &actor, "Favorite root", "favorite-root")
        .await
        .base
        .id;
    let left = places::make_department(&world, &actor, root, "Earlier native path", "earlier")
        .await
        .base
        .id;
    let right = places::make_department(&world, &actor, root, "Later native path", "later")
        .await
        .base
        .id;
    let moving = places::make_project(&world, &actor, left, "Moving favorite")
        .await
        .base
        .id;
    let stays_left = places::make_project(&world, &actor, left, "Keeps earlier path")
        .await
        .base
        .id;
    let stays_right = places::make_project(&world, &actor, right, "Keeps later path")
        .await
        .base
        .id;
    for project in [moving, stays_left, stays_right] {
        world
            .app
            .call(MeFavoriteSet, &[&project])
            .token(&actor.token)
            .body(&MeFavoriteSetBody {
                after_node_id: None,
                view: None,
            })
            .send()
            .await
            .ok();
    }
    let token = key(
        &world,
        actor.id(),
        vec![
            rule("MeBootstrapGet", "account", actor.id()),
            home("MeBootstrapGet", root),
        ],
    )
    .await;
    let control = checked(
        world
            .app
            .call(MeBootstrapGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let original = control["favorites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|favorite| favorite["node"]["id"] == json!(moving))
        .unwrap();
    assert!(
        original["path"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == json!(left))
    );
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(MeBootstrapGet, &[]).token(&token);
        call.send_bytes().await
    });
    wait_for_activity(&world, pid, &request).await;
    // Native persistence helper, preserving its same-tree prerequisite and ordinary actor standing.
    let mut moved = world.db.pool.begin().await.unwrap();
    orch_db::queries::hierarchy::move_under(&mut moved, moving, right, world.now())
        .await
        .unwrap();
    moved.commit().await.unwrap();
    let native = world
        .app
        .call(MeBootstrapGet, &[])
        .token(&actor.token)
        .send()
        .await
        .ok();
    let path = &native
        .favorites
        .iter()
        .find(|favorite| favorite.node.id == moving)
        .unwrap()
        .path;
    assert!(path.iter().any(|node| node.id == right));
    assert!(path.iter().all(|node| node.id != left));
    assert!(
        native
            .favorites
            .iter()
            .any(|favorite| favorite.path.iter().any(|node| node.id == left))
    );
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Moving favorite"));
}
