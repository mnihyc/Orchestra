//! Native requester capacity waits retain the complete HTTP credential and current native facts.
use super::*;
use chat_world::Room;
use orch_api_contract::auth::MeGet;
use orch_api_contract::chats::{ChatJoinRequest, ChatJoinRequestBody};
use std::time::Duration;

enum Entry {
    Access(AccessRequestCreateBody),
    Join(Id),
}
async fn blocked(world: &World, locker: i32) -> Result<(), tokio::time::error::Elapsed> {
    // Same exact lock-edge witness budget used by the qualified Invitation/Approval controls.
    tokio::time::timeout(Duration::from_secs(10),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(locker).fetch_one(&world.db.pool).await.unwrap();
            if waiting {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await
}
async fn after_wait(world: &World, actor: Id, token: String, entry: Entry) -> RawReply {
    let mut held = world.db.pool.begin().await.unwrap();
    let locker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    orch_db::queries::approvals::access::lock_requester(&mut held, actor)
        .await
        .unwrap();
    let app = world.app.clone();
    let mut pending = tokio::spawn(async move {
        match entry {
            Entry::Access(body) => {
                let request = app.call(AccessRequestCreate, &[]).token(&token).body(&body);
                request.send_bytes().await
            }
            Entry::Join(chat) => {
                let request = app
                    .call(ChatJoinRequest, &[&chat])
                    .token(&token)
                    .body(&ChatJoinRequestBody::default());
                request.send_bytes().await
            }
        }
    });
    tokio::select! {
        finished=&mut pending=>{
            held.rollback().await.unwrap();
            let reply=finished.unwrap();
            let value:Json=serde_json::from_slice(&reply.raw).unwrap();
            panic!("request ended before the real native requester lock: status={},code={}; expiry/currentness premise UNREACHED",reply.status,value["code"]);
        }
        edge=blocked(world,locker)=>{
            if edge.is_err(){pending.abort();let _=pending.await;held.rollback().await.unwrap();panic!("real requester advisory lock edge not observed");}
        }
    }
    // TTL is the existing one-second key/session boundary; no sleep aligns the race.
    world.advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    pending.await.unwrap()
}
async fn expire_browser(world: &World, token: &str) {
    let changed = sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
        .bind(token_hash(token))
        .bind(world.now() + chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        changed.rows_affected(),
        1,
        "this actual HTTP access token must be selected"
    );
    checked(
        world.app.call(MeGet, &[]).token(token).send_bytes().await,
        StatusCode::OK,
    );
}
async fn expire_key(world: &World, actor: Id) -> Key {
    let key = legacy(world, actor).await;
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(key.id)
        .bind(world.now() + chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    checked(
        world
            .app
            .call(MeGet, &[])
            .token(&key.secret)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    key
}
async fn access_expiry(browser: bool) {
    let s = Office::new().await;
    let ordinary = s.native(&s.document()).await;
    cancel(&s.desk.world, &s.requester.token, &ordinary["approvals"]).await;
    let token = if browser {
        expire_browser(&s.desk.world, &s.requester.token).await;
        s.requester.token.clone()
    } else {
        expire_key(&s.desk.world, s.requester.id()).await.secret
    };
    let before = footprint(&s.desk.world, s.requester.id()).await;
    let result = after_wait(
        &s.desk.world,
        s.requester.id(),
        token,
        Entry::Access(s.document()),
    )
    .await;
    checked(result, StatusCode::UNAUTHORIZED);
    assert_eq!(
        footprint(&s.desk.world, s.requester.id()).await,
        before,
        "no request/audit/notification may be accepted after the credential expires"
    );
}
async fn join_expiry(browser: bool) {
    let room = Room::new().await;
    let chat = room
        .group(&room.bea, &[&room.cai], Some("Native requester lock group"))
        .await
        .base
        .id;
    let ordinary = checked(
        room.world
            .app
            .call(ChatJoinRequest, &[&chat])
            .token(&room.gus.token)
            .body(&ChatJoinRequestBody::default())
            .send_bytes()
            .await,
        StatusCode::CREATED,
    );
    cancel(&room.world, &room.gus.token, &ordinary["approvals"]).await;
    let token = if browser {
        expire_browser(&room.world, &room.gus.token).await;
        room.gus.token.clone()
    } else {
        expire_key(&room.world, room.gus.id()).await.secret
    };
    let before = footprint(&room.world, room.gus.id()).await;
    let result = after_wait(&room.world, room.gus.id(), token, Entry::Join(chat)).await;
    checked(result, StatusCode::UNAUTHORIZED);
    assert_eq!(footprint(&room.world, room.gus.id()).await, before);
}
#[tokio::test]
async fn legacy_request_key_deadline_is_checked_after_the_actual_requester_lock() {
    access_expiry(false).await;
}
#[tokio::test]
async fn browser_request_access_token_deadline_is_checked_after_the_actual_requester_lock() {
    access_expiry(true).await;
}
#[tokio::test]
async fn legacy_join_key_deadline_is_checked_after_the_actual_requester_lock() {
    join_expiry(false).await;
}
#[tokio::test]
async fn browser_join_access_token_deadline_is_checked_after_the_actual_requester_lock() {
    join_expiry(true).await;
}
#[tokio::test]
async fn natural_work_manager_expiry_is_rechecked_after_the_actual_requester_lock() {
    let s = Office::new().await;
    let (manager, _, body) = work_request_at(
        &s,
        Some(Time::new(s.desk.world.now() + chrono::Duration::seconds(1))),
    )
    .await;
    let ordinary = checked(
        create(&s.desk.world, &manager.token, &body).await,
        StatusCode::OK,
    );
    cancel(&s.desk.world, &manager.token, &ordinary["approvals"]).await;
    let key = legacy(&s.desk.world, manager.id()).await;
    let before = footprint(&s.desk.world, manager.id()).await;
    let result = after_wait(&s.desk.world, manager.id(), key.secret, Entry::Access(body)).await;
    assert!(
        !can_node(
            &s.desk.world,
            manager.id(),
            s.desk.topic,
            Permission::AccessManage
        )
        .await,
        "ordinary Work manager entry must actually have expired"
    );
    let refused = checked(result, StatusCode::BAD_REQUEST);
    assert_eq!(refused["code"], "request.invalid");
    assert_eq!(footprint(&s.desk.world, manager.id()).await, before);
}
