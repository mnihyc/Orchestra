//! Native Snapshot windows keep current copied custody, recorded origins and scoped pagination.

use super::*;
use crate::snapshot_scope_helpers as keys;
use orch_api_core::testing::RawReply;

async fn saved(site: &Site, origin: Id, label: &str, recorded: bool) -> Id {
    let id = new_id();
    let mut conn = site.world.conn().await;
    storage::begin_snapshot(
        &mut conn,
        &NewSnapshot {
            id,
            thread_id: Some(site.thread),
            environment_id: origin,
            env_generation: 1,
            label,
            selection: &json!({"include":["**"],"exclude":[]}),
            created_by: Some(site.ada.id()),
            at: site.world.now(),
        },
    )
    .await
    .unwrap();
    storage::end_snapshot(
        &mut conn,
        id,
        &SnapshotEnd {
            state: snapshot_state::COMPLETE,
            file_count: 2,
            total_bytes: 19,
            missing_count: 0,
            manifest_blob: None,
            at: site.world.now(),
        },
    )
    .await
    .unwrap();
    if recorded {
        sqlx::query(
            "UPDATE snapshots SET kind='turn',storage='environment',directory='web',
            tree_ref='4b825dc642cb6eb9a060e54bf8d69288fbee4904',expires_at=$2 WHERE id=$1",
        )
        .bind(id)
        .bind(site.world.now() + Duration::days(1))
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    id
}

async fn list(site: &Site, token: &str, query: &PageQuery) -> RawReply {
    site.app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}

fn shown(reply: RawReply) -> orch_api_contract::common::Page<Snapshot> {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn snapshot_list_exact_read_preserves_native_metadata_without_a_machine_command() {
    let site = Site::new().await;
    let id = saved(&site, site.environment, "Native selected snapshot", false).await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind LIKE 'fs.%'")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    let ordinary = shown(list(&site, &site.ada.token, &Default::default()).await);
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::read("ThreadSnapshotList", site.environment)],
    )
    .await;
    let scoped = shown(list(&site, &key.secret, &Default::default()).await);
    assert_eq!(scoped.items[0].id, id);
    assert_eq!(
        serde_json::to_value(&scoped).unwrap(),
        serde_json::to_value(&ordinary).unwrap()
    );
    assert_eq!(scoped.items[0].file_count, 2);
    assert_eq!(scoped.items[0].total_bytes, 19);
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind LIKE 'fs.%'")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        commands, before,
        "the list projects stored metadata without taking or reading bytes"
    );
}

#[tokio::test]
async fn snapshot_list_origin_scope_precedes_its_limit_cursor_and_same_operation_names() {
    let site = Site::new().await;
    let other = snapshot_scopes::elsewhere(&site, &site.eve, site.eve.person.personal_org).await;
    let old = saved(&site, site.environment, "Permitted older snapshot", false).await;
    site.world.advance(Duration::minutes(1));
    saved(&site, other, "Excluded middle snapshot", false).await;
    site.world.advance(Duration::minutes(1));
    let recent = saved(&site, site.environment, "Permitted recent snapshot", false).await;
    site.world.advance(Duration::minutes(1));
    saved(&site, other, "Excluded newest snapshot", false).await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            keys::read("ThreadSnapshotList", site.environment),
            keys::read("SnapshotGet", other),
        ],
    )
    .await;
    let first = shown(
        list(
            &site,
            &key.secret,
            &PageQuery {
                limit: Some(1),
                cursor: None,
            },
        )
        .await,
    );
    assert_eq!(
        first.items.iter().map(|row| row.id).collect::<Vec<_>>(),
        [recent]
    );
    let cursor = first.next_cursor.unwrap();
    assert!(cursor.ends_with(&recent.to_string()));
    let second = shown(
        list(
            &site,
            &key.secret,
            &PageQuery {
                limit: Some(1),
                cursor: Some(cursor),
            },
        )
        .await,
    );
    assert_eq!(
        second.items.iter().map(|row| row.id).collect::<Vec<_>>(),
        [old]
    );
    assert!(second.next_cursor.is_none());
    let full = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            keys::read("ThreadSnapshotList", site.environment),
            keys::read("ThreadSnapshotList", other),
        ],
    )
    .await;
    let all = shown(list(&site, &full.secret, &Default::default()).await);
    assert_eq!(all.items.len(), 4);
    assert_eq!(all.items[0].environment.id, other);
    assert_eq!(
        all.items[0].environment.name,
        format!("Other private source {}", other.simple())
    );
}

#[tokio::test]
async fn snapshot_list_separates_current_manual_custody_from_recorded_original_custody() {
    let site = Site::new().await;
    let manual = saved(&site, site.environment, "Copied through old binding", false).await;
    let recorded = saved(&site, site.environment, "Recorded under old binding", true).await;
    let other = snapshot_scopes::elsewhere(&site, &site.eve, site.eve.person.personal_org).await;
    snapshot_scopes::move_thread(&site, other).await;
    for (who, expected) in [(&site.vic, recorded), (&site.eve, manual)] {
        let token = keys::scoped(
            &site,
            who.id(),
            vec![
                keys::read("ThreadSnapshotList", site.environment),
                keys::read("ThreadSnapshotList", other),
            ],
        )
        .await;
        let page = shown(
            list(
                &site,
                &token.secret,
                &PageQuery {
                    limit: Some(1),
                    cursor: None,
                },
            )
            .await,
        );
        assert_eq!(
            page.items.iter().map(|row| row.id).collect::<Vec<_>>(),
            [expected]
        );
        assert_eq!(page.items[0].environment.id, site.environment);
        assert!(page.next_cursor.is_none());
    }
    let only_current = keys::scoped(
        &site,
        site.eve.id(),
        vec![keys::read("ThreadSnapshotList", other)],
    )
    .await;
    let empty = shown(list(&site, &only_current.secret, &Default::default()).await);
    assert!(
        empty.items.is_empty(),
        "the copied row still returns its old origin EnvironmentRef"
    );
}

#[tokio::test]
async fn snapshot_list_keeps_typed_scope_and_the_native_files_gate() {
    let site = Site::new().await;
    saved(
        &site,
        site.environment,
        "Protected selected snapshot",
        false,
    )
    .await;
    let wrong_id = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::read("ThreadSnapshotList", site.thread)],
    )
    .await;
    assert!(
        shown(list(&site, &wrong_id.secret, &Default::default()).await)
            .items
            .is_empty()
    );
    let wrong_kind = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::rule(
            "ThreadSnapshotList",
            "thread",
            site.environment,
            "self",
            &["read"],
        )],
    )
    .await;
    assert!(
        shown(list(&site, &wrong_kind.secret, &Default::default()).await)
            .items
            .is_empty()
    );
    let wrong_family = keys::scoped(
        &site,
        site.ada.id(),
        vec![json!({
            "target":{"kind":"environment","id":site.environment,"reach":"self"},
            "resources":["machine"],
            "allow":[{"operation":"ThreadSnapshotList","actions":["read"]}]
        })],
    )
    .await;
    assert_eq!(
        list(&site, &wrong_family.secret, &Default::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let unrelated = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::read("SnapshotGet", site.environment)],
    )
    .await;
    assert_eq!(
        list(&site, &unrelated.secret, &Default::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let no_ordinary_view = keys::scoped(
        &site,
        site.eve.id(),
        vec![keys::read("ThreadSnapshotList", site.environment)],
    )
    .await;
    let denied = list(&site, &no_ordinary_view.secret, &Default::default()).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Protected selected snapshot"));
}

#[tokio::test]
async fn snapshot_list_restores_current_key_policy_revocation_and_legacy_files_view() {
    let site = Site::new().await;
    saved(
        &site,
        site.environment,
        "Current native snapshot metadata",
        false,
    )
    .await;
    let legacy = keys::legacy(&site, site.ada.id(), None).await;
    assert_eq!(
        shown(list(&site, &legacy.secret, &Default::default()).await)
            .items
            .len(),
        1
    );
    for revoke in [false, true] {
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![keys::read("ThreadSnapshotList", site.environment)],
        )
        .await;
        assert_eq!(
            shown(list(&site, &key.secret, &Default::default()).await)
                .items
                .len(),
            1
        );
        if revoke {
            keys::revoke(&site, &key).await;
        } else {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(
                    serde_json::to_value(orch_access_contract::key_scope::KeyScope::default())
                        .unwrap(),
                )
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        }
        let denied = list(&site, &key.secret, &Default::default()).await;
        assert_eq!(
            denied.status,
            if revoke {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert!(!String::from_utf8_lossy(&denied.raw).contains("Current native snapshot metadata"));
    }
}

#[tokio::test]
async fn snapshot_list_publication_keeps_actual_rows_instead_of_a_replacement_under_the_same_governor()
 {
    let site = Site::new().await;
    let old = saved(
        &site,
        site.environment,
        "Actual original held Snapshot",
        false,
    )
    .await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::read("ThreadSnapshotList", site.environment)],
    )
    .await;
    assert_eq!(
        shown(list(&site, &key.secret, &Default::default()).await).items[0].id,
        old
    );
    let mut blocker = site.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = site.world.app.clone();
    let secret = key.secret.clone();
    let thread = site.thread;
    let request = tokio::spawn(async move {
        let request = app
            .call(ThreadSnapshotList, &[&thread])
            .token(&secret)
            .query(&PageQuery::default());
        request.send_bytes().await
    });
    super::snapshot_mutations::activity_wait(&site, pid, &request).await;
    assert!(
        storage::delete_snapshot(&mut *site.world.conn().await, old)
            .await
            .unwrap()
            .is_some()
    );
    let replacement = saved(
        &site,
        site.environment,
        "Different current Snapshot under same governor",
        false,
    )
    .await;
    blocker.commit().await.unwrap();
    let held = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&held.raw).contains("Actual original held Snapshot"));
    assert!(!String::from_utf8_lossy(&held.raw).contains("Different current Snapshot"));
    let current = shown(list(&site, &key.secret, &Default::default()).await);
    assert_eq!(
        current.items.iter().map(|row| row.id).collect::<Vec<_>>(),
        [replacement]
    );
    assert_eq!(current.items[0].environment.id, site.environment);
}
