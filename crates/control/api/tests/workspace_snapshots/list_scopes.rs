//! Native pagination scopes the actual selected custody, historical metadata and cursor witness.

use super::*;
use orch_types::Json;

fn read(environment: Id) -> Json {
    keys::read("ThreadSnapshotList", environment)
}
fn page(limit: Option<u32>, cursor: Option<String>) -> PageQuery {
    PageQuery { limit, cursor }
}

async fn listing(site: &Site, token: &str, query: &PageQuery) -> RawReply {
    let call = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(token)
        .query(query);
    call.send_bytes().await
}
fn body(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn dated(site: &Site, id: Id, days_ago: i64, origin: Id) {
    sqlx::query("UPDATE snapshots SET created_at=$2, environment_id=$3 WHERE id=$1")
        .bind(id)
        .bind(site.world.now() - chrono::Duration::days(days_ago))
        .bind(origin)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
}
async fn wait_activity(site: &Site, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(RENDEZVOUS, async {
        loop {
            assert!(!request.is_finished(), "the actual activity append must wait");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&site.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("observe the real post-snapshot publication boundary");
}

#[tokio::test]
async fn list_keeps_the_ordinary_body_and_needs_its_own_read_and_native_files_authority() {
    let (site, id) = fixture().await;
    let query = PageQuery::default();
    let ordinary = body(listing(&site, &site.vic.token, &query).await);
    assert_eq!(ordinary["items"][0]["id"], json!(id));
    let key = keys::scoped(&site, site.vic.id(), vec![read(site.environment)]).await;
    assert_eq!(body(listing(&site, &key.secret, &query).await), ordinary);
    for rule in [
        keys::read("SnapshotGet", site.environment),
        keys::rule(
            "ThreadSnapshotList",
            "environment",
            site.environment,
            "self",
            &["manage"],
        ),
        keys::rule(
            "ThreadSnapshotList",
            "thread",
            site.thread,
            "self",
            &["read"],
        ),
    ] {
        let key = keys::scoped(&site, site.vic.id(), vec![rule]).await;
        assert_eq!(
            listing(&site, &key.secret, &query).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let denied = keys::scoped(&site, site.eve.id(), vec![read(site.environment)]).await;
    assert_eq!(
        listing(&site, &denied.secret, &query).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(site.sent(CommandKind::FsRecordRead), 0);
}

#[tokio::test]
async fn copied_origins_are_filtered_before_the_native_page_and_cursor_are_chosen() {
    let (site, oldest) = fixture().await;
    let middle = copied(
        &site,
        Some(site.thread),
        vec![(FILE.into(), CONTENT.into())],
    )
    .await;
    let hidden = copied(
        &site,
        Some(site.thread),
        vec![(FILE.into(), CONTENT.into())],
    )
    .await;
    let historical = elsewhere(&site, &site.ada, site.org).await;
    let current = elsewhere(&site, &site.ada, site.org).await;
    dated(&site, oldest, 3, site.environment).await;
    dated(&site, middle, 2, site.environment).await;
    dated(&site, hidden, 1, historical).await;
    move_thread(&site, current).await;
    let partial = keys::scoped(
        &site,
        site.ada.id(),
        vec![read(current), read(site.environment)],
    )
    .await;
    let first = body(listing(&site, &partial.secret, &page(Some(1), None)).await);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["id"], json!(middle));
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let last = body(listing(&site, &partial.secret, &page(Some(1), Some(cursor))).await);
    assert_eq!(last["items"][0]["id"], json!(oldest));
    assert!(last["next_cursor"].is_null());
    let both = keys::scoped(
        &site,
        site.ada.id(),
        vec![read(current), read(site.environment), read(historical)],
    )
    .await;
    let full = body(listing(&site, &both.secret, &page(Some(1), None)).await);
    assert_eq!(full["items"][0]["id"], json!(hidden));
    assert_eq!(full["items"][0]["environment"]["id"], json!(historical));
    let origin_only = keys::scoped(&site, site.ada.id(), vec![read(site.environment)]).await;
    let empty = body(listing(&site, &origin_only.secret, &PageQuery::default()).await);
    assert_eq!(empty["items"], json!([]));
    assert!(empty["next_cursor"].is_null());
}

#[tokio::test]
async fn recorded_snapshots_keep_their_origin_when_the_thread_moves_to_an_unreadable_environment() {
    let (site, id, _) = recorded().await;
    let foreign = elsewhere(&site, &site.eve, site.eve.person.personal_org).await;
    move_thread(&site, foreign).await;
    let ordinary = body(listing(&site, &site.vic.token, &PageQuery::default()).await);
    assert_eq!(ordinary["items"][0]["id"], json!(id));
    let key = keys::scoped(&site, site.vic.id(), vec![read(site.environment)]).await;
    assert_eq!(
        body(listing(&site, &key.secret, &PageQuery::default()).await),
        ordinary
    );
    let wrong = keys::scoped(&site, site.vic.id(), vec![read(foreign)]).await;
    assert_eq!(
        body(listing(&site, &wrong.secret, &PageQuery::default()).await)["items"],
        json!([])
    );
}

#[tokio::test]
async fn a_genuinely_empty_native_window_keeps_its_operation_proof_and_ordinary_gate() {
    let site = Site::new().await;
    site.power_off().await;
    let other = elsewhere(&site, &site.ada, site.org).await;
    for environment in [site.environment, other] {
        let key = keys::scoped(&site, site.ada.id(), vec![read(environment)]).await;
        let shown = body(listing(&site, &key.secret, &PageQuery::default()).await);
        assert_eq!(shown["items"], json!([]));
        assert!(shown["next_cursor"].is_null());
    }
    let unrelated = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::read("SnapshotGet", site.environment)],
    )
    .await;
    assert_eq!(
        listing(&site, &unrelated.secret, &PageQuery::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let denied = keys::scoped(&site, site.eve.id(), vec![read(site.environment)]).await;
    assert_eq!(
        listing(&site, &denied.secret, &PageQuery::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn held_rows_cannot_borrow_a_new_empty_or_same_governor_replacement_window_at_publication() {
    for change in ["live", "scope", "expiry", "association"] {
        let (site, id) = fixture().await;
        let other = elsewhere(&site, &site.ada, site.org).await;
        let replacement = copied(
            &site,
            Some(site.thread),
            vec![(FILE.into(), b"Other native source".to_vec())],
        )
        .await;
        dated(&site, id, 1, site.environment).await;
        dated(&site, replacement, 2, site.environment).await;
        let key = keys::scoped(&site, site.ada.id(), vec![read(site.environment)]).await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let token = key.secret.clone();
        let thread = site.thread;
        let request = tokio::spawn(async move {
            let call = app
                .call(ThreadSnapshotList, &[&thread])
                .token(&token)
                .query(&page(Some(1), None));
            call.send_bytes().await
        });
        wait_activity(&site, pid, &request).await;
        if change == "scope" {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(json!({"rules":[read(other)]}))
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        } else if change == "expiry" {
            site.world.advance(chrono::Duration::hours(2));
        } else if change == "association" {
            sqlx::query("UPDATE snapshots SET thread_id=NULL WHERE id=$1")
                .bind(id)
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(RENDEZVOUS, request)
            .await
            .unwrap()
            .unwrap();
        if change == "live" {
            assert_eq!(body(reply)["items"][0]["id"], json!(id));
        } else {
            assert!(
                !reply.status.is_success(),
                "old rows never use a replacement's proof"
            );
            let raw: Json = serde_json::from_slice(&reply.raw).unwrap();
            assert!(raw.get("items").is_none());
        }
        if change == "scope" {
            let fresh = body(listing(&site, &key.secret, &PageQuery::default()).await);
            assert_eq!(fresh["items"], json!([]));
        } else if change == "association" {
            let fresh = body(listing(&site, &key.secret, &page(Some(1), None)).await);
            assert_eq!(fresh["items"][0]["id"], json!(replacement));
        }
    }
}

#[tokio::test]
async fn a_held_continuation_cursor_retains_its_actual_lookahead_origin_read() {
    let (site, first) = fixture().await;
    let continuation = copied(
        &site,
        Some(site.thread),
        vec![(FILE.into(), CONTENT.into())],
    )
    .await;
    let historical = elsewhere(&site, &site.ada, site.org).await;
    dated(&site, first, 1, site.environment).await;
    dated(&site, continuation, 2, historical).await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![read(site.environment), read(historical)],
    )
    .await;
    let positive = body(listing(&site, &key.secret, &page(Some(1), None)).await);
    assert_eq!(positive["items"][0]["id"], json!(first));
    assert!(positive["next_cursor"].is_string());
    let mut blocker = site.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = site.app().clone();
    let token = key.secret.clone();
    let thread = site.thread;
    let request = tokio::spawn(async move {
        let call = app
            .call(ThreadSnapshotList, &[&thread])
            .token(&token)
            .query(&page(Some(1), None));
        call.send_bytes().await
    });
    wait_activity(&site, pid, &request).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(json!({"rules":[read(site.environment)]}))
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let held = tokio::time::timeout(RENDEZVOUS, request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.status, StatusCode::FORBIDDEN);
    let fresh = body(listing(&site, &key.secret, &page(Some(1), None)).await);
    assert_eq!(fresh["items"][0]["id"], json!(first));
    assert!(fresh["next_cursor"].is_null());
}
