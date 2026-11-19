//! Native Skill windows and once-only guarded legacy URL custody outside read snapshots.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Json, new_id};
use serde_json::json;

fn rule(operation: &str, kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Native Skill read scopes",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
async fn list(world: &World, token: &str, query: &SkillListQuery) -> RawReply {
    world
        .app
        .call(SkillList, &[])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
async fn get(world: &World, token: &str, id: Id) -> RawReply {
    world
        .app
        .call(SkillGet, &[&id])
        .token(token)
        .send_bytes()
        .await
}
fn accepted<T: serde::de::DeserializeOwned>(reply: RawReply) -> T {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn visible(rows: &[Listed<Skill>]) -> Vec<Id> {
    rows.iter()
        .filter_map(|row| row.as_visible().map(|skill| skill.id))
        .collect()
}

#[tokio::test]
async fn skill_read_family_uses_actual_selected_rows_and_native_org_filter_without_operation_borrowing()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let first = make(&world, &c.viewer, c.acme, "scoped-first").await;
    let second = make(&world, &c.viewer, c.acme, "scoped-second").await;
    let personal = make(
        &world,
        &c.viewer,
        c.viewer.person.personal_org,
        "scoped-personal",
    )
    .await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![
            rule("SkillList", "skill", first.id, "self"),
            rule("SkillList", "skill", personal.id, "self"),
            rule("SkillGet", "skill", second.id, "self"),
        ],
    )
    .await;
    let all: Vec<Listed<Skill>> = accepted(list(&world, &token, &Default::default()).await);
    let mut ids = visible(&all);
    ids.sort();
    let mut expected = vec![first.id, personal.id];
    expected.sort();
    assert_eq!(ids, expected);
    let filtered: Vec<Listed<Skill>> = accepted(
        list(
            &world,
            &token,
            &SkillListQuery {
                org_id: Some(c.acme),
            },
        )
        .await,
    );
    assert_eq!(visible(&filtered), vec![first.id]);
    assert!(
        !serde_json::to_string(&filtered)
            .unwrap()
            .contains("scoped-second")
    );
    assert_eq!(
        get(&world, &token, first.id).await.status,
        StatusCode::FORBIDDEN
    );
    let own: Skill = accepted(get(&world, &token, second.id).await);
    assert_eq!(own.id, second.id);
    let wrong = key(
        &world,
        c.viewer.id(),
        vec![rule("SkillGet", "template", first.id, "self")],
    )
    .await;
    assert_eq!(
        get(&world, &wrong, first.id).await.status,
        StatusCode::FORBIDDEN
    );
    let outsider = key(
        &world,
        c.stranger.id(),
        vec![rule("SkillGet", "skill", first.id, "self")],
    )
    .await;
    assert_eq!(
        get(&world, &outsider, first.id).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn skill_read_family_keeps_primary_content_and_binds_only_its_own_protected_home_names() {
    let world = World::new().await;
    let c = company(&world).await;
    let skill = make(&world, &c.viewer, c.acme, "scope-home-label").await;
    for endpoint in ["SkillGet", "SkillList"] {
        for names in [
            None,
            Some(if endpoint == "SkillGet" {
                "SkillList"
            } else {
                "SkillGet"
            }),
            Some(endpoint),
        ] {
            let mut rules = vec![rule(endpoint, "skill", skill.id, "self")];
            if let Some(operation) = names {
                rules.push(rule(operation, "org", c.acme, "self"));
            }
            let token = key(&world, c.viewer.id(), rules).await;
            let value = if endpoint == "SkillGet" {
                accepted::<Skill>(get(&world, &token, skill.id).await)
            } else {
                accepted::<Vec<Listed<Skill>>>(list(&world, &token, &Default::default()).await)
                    .into_iter()
                    .find_map(Listed::into_visible)
                    .unwrap()
            };
            assert_eq!(value.name, "scope-home-label");
            assert_eq!(
                value.latest.as_ref().unwrap().id,
                skill.latest.as_ref().unwrap().id
            );
            assert_eq!(value.home.name.is_some(), names == Some(endpoint));
        }
    }
}

#[tokio::test]
async fn skill_list_preserves_the_native_exhaustive_cardinality_and_real_empty_result() {
    use orch_access_contract::decisions::ObjectRef;
    use orch_authz::ownership::{NewAccess, start_resource};
    let world = World::new().await;
    let c = company(&world).await;
    let empty = key(
        &world,
        c.viewer.id(),
        vec![rule("SkillList", "org", c.acme, "subtree")],
    )
    .await;
    let rows: Vec<Listed<Skill>> = accepted(list(&world, &empty, &Default::default()).await);
    assert!(rows.is_empty());
    let mut expected = Vec::new();
    for index in 0..105 {
        let mut conn = world.conn().await;
        let id = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "skill",
            Some(c.viewer.id()),
            Some(c.acme),
            &format!("exhaustive-skill-{index}"),
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Skill,
                id,
            },
            c.viewer.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        expected.push(id);
    }
    let rows: Vec<Listed<Skill>> = accepted(
        list(
            &world,
            &empty,
            &SkillListQuery {
                org_id: Some(c.acme),
            },
        )
        .await,
    );
    let mut actual = visible(&rows);
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

const PRIVATE_URL: &str = "https://fixture:fixture-private@example.invalid/skill.tar.gz?token=legacy-skill-secret-fixture#private";
async fn legacy(world: &World, c: &Company, name: &str) -> Skill {
    let skill = make(world, &c.viewer, c.acme, name).await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut *world.conn().await,
        "host",
        Some(c.viewer.id()),
        Some(c.acme),
        &format!("{name}-host"),
    )
    .await
    .unwrap();
    let origin = json!({"kind":"url","url":PRIVATE_URL,"folder":null,"host_id":host});
    sqlx::query("UPDATE skills SET origin=$2,source_url_secret_id=NULL WHERE id=$1")
        .bind(skill.id)
        .bind(&origin)
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE skill_versions SET origin=$2 WHERE skill_id=$1")
        .bind(skill.id)
        .bind(&origin)
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE audit_log SET details=jsonb_set(details,'{origin}',$2) WHERE object_kind='skill' AND object_id=$1").bind(skill.id).bind(origin).execute(&world.db.pool).await.unwrap();
    skill
}
async fn custody(
    world: &World,
    id: Id,
) -> (Json, Option<Id>, Vec<(Id, String, Vec<u8>, String)>, i64) {
    let (origin, pointer): (Json, Option<Id>) =
        sqlx::query_as("SELECT origin,source_url_secret_id FROM skills WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let secrets=sqlx::query_as("SELECT id,name,sealed,xmin::text FROM secrets WHERE owner_kind='skill' AND owner_id=$1 ORDER BY name").bind(id).fetch_all(&world.db.pool).await.unwrap();
    let audits=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_kind='skill' AND object_id=$1 AND details ? 'origin'").bind(id).fetch_one(&world.db.pool).await.unwrap();
    (origin, pointer, secrets, audits)
}
#[tokio::test]
async fn skill_reads_migrate_only_the_selected_legacy_sources_once_and_never_expose_the_private_url()
 {
    for endpoint in ["SkillGet", "SkillList"] {
        let world = World::new().await;
        let c = company(&world).await;
        let selected = legacy(&world, &c, "legacy-selected-read").await;
        let excluded = legacy(&world, &c, "legacy-excluded-read").await;
        let excluded_before = custody(&world, excluded.id).await;
        let token = key(
            &world,
            c.viewer.id(),
            vec![rule(endpoint, "skill", selected.id, "self")],
        )
        .await;
        let read = if endpoint == "SkillGet" {
            get(&world, &token, selected.id).await
        } else {
            list(&world, &token, &Default::default()).await
        };
        assert_eq!(
            read.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&read.raw)
        );
        assert!(!String::from_utf8_lossy(&read.raw).contains("legacy-skill-secret-fixture"));
        let migrated = custody(&world, selected.id).await;
        assert!(migrated.1.is_some());
        assert_eq!(migrated.2.len(), 2);
        assert_eq!(migrated.3, 0);
        assert!(
            !migrated
                .0
                .to_string()
                .contains("legacy-skill-secret-fixture")
        );
        for (_, name, sealed, _) in &migrated.2 {
            assert_eq!(
                orch_db::secrets::MasterKey::new([7; 32])
                    .open(
                        sealed,
                        &orch_api_core::routes::resources::sealing::binding(
                            "skill",
                            selected.id,
                            name
                        )
                    )
                    .unwrap(),
                PRIVATE_URL.as_bytes()
            );
        }
        let again = if endpoint == "SkillGet" {
            get(&world, &token, selected.id).await
        } else {
            list(&world, &token, &Default::default()).await
        };
        assert_eq!(again.status, StatusCode::OK);
        assert_eq!(custody(&world, selected.id).await, migrated);
        assert_eq!(custody(&world, excluded.id).await, excluded_before);
    }
}

async fn wait_on(
    world: &World,
    pid: i32,
    needle: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{assert!(!request.is_finished(),"the actual Skill request must reach its native wait");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
            .bind(pid).bind(needle).fetch_one(&world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;
    }}).await.expect("the native Skill query must wait on the exact held backend");
}

#[tokio::test]
async fn skill_url_maintenance_rechecks_after_actual_source_version_secret_and_audit_waits_before_any_write()
 {
    for wait in ["skill", "version", "secret", "audit"] {
        for expired in [false, true] {
            let world = World::new().await;
            let c = company(&world).await;
            let skill = legacy(&world, &c, "legacy-wait-read").await;
            if wait == "secret" {
                orch_api_core::routes::resources::sealing::put_skill_source_secret(
                    &mut *world.conn().await,
                    skill.id,
                    "source_url:unrelated_fixture",
                    "https://example.invalid/kept",
                    None,
                    world.now(),
                )
                .await
                .unwrap();
            }
            let before = custody(&world, skill.id).await;
            let token = key(
                &world,
                c.viewer.id(),
                vec![rule("SkillList", "skill", skill.id, "self")],
            )
            .await;
            let mut blocker = world.db.pool.begin().await.unwrap();
            let (sql, needle) = match wait {
                "skill" => (
                    "SELECT id FROM skills WHERE id=$1 FOR UPDATE",
                    "%FROM skills%FOR UPDATE%",
                ),
                "version" => (
                    "SELECT id FROM skill_versions WHERE skill_id=$1 FOR UPDATE",
                    "%FROM skill_versions%FOR UPDATE%",
                ),
                "secret" => (
                    "SELECT id FROM secrets WHERE owner_kind='skill' AND owner_id=$1 FOR UPDATE",
                    "%FROM secrets%FOR UPDATE%",
                ),
                _ => (
                    "SELECT id FROM audit_log WHERE object_kind='skill' AND object_id=$1 AND details ? 'origin' FOR UPDATE",
                    "%FROM audit_log%FOR UPDATE%",
                ),
            };
            sqlx::query(sql)
                .bind(skill.id)
                .fetch_all(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = world.app.clone();
            let request = tokio::spawn(async move {
                let request = app
                    .call(SkillList, &[])
                    .token(&token)
                    .query(&SkillListQuery::default());
                request.send_bytes().await
            });
            wait_on(&world, pid, needle, &request).await;
            assert_eq!(custody(&world, skill.id).await, before);
            if expired {
                world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap();
            if expired {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert_eq!(custody(&world, skill.id).await, before);
            } else {
                assert_eq!(
                    reply.status,
                    StatusCode::OK,
                    "{}",
                    String::from_utf8_lossy(&reply.raw)
                );
                let after = custody(&world, skill.id).await;
                assert!(after.1.is_some());
                assert_eq!(after.3, 0);
            }
        }
    }
}

#[tokio::test]
async fn skill_publication_keeps_completed_source_custody_and_never_repeats_maintenance_or_returns_unread_metadata()
 {
    for change in ["unchanged", "expired", "read_lost"] {
        let world = World::new().await;
        let c = company(&world).await;
        let skill = legacy(&world, &c, "legacy-publication-read").await;
        let token = key(
            &world,
            c.viewer.id(),
            vec![rule("SkillList", "org", c.acme, "subtree")],
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
        let request_token = token.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(SkillList, &[])
                .token(&request_token)
                .query(&SkillListQuery::default());
            request.send_bytes().await
        });
        wait_on(&world, pid, "%INSERT INTO api_token_activity%", &request).await;
        let completed = custody(&world, skill.id).await;
        assert!(completed.1.is_some());
        assert_eq!(completed.2.len(), 2);
        assert_eq!(completed.3, 0);
        if change == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if change == "read_lost" {
            sqlx::query("UPDATE skills SET org_id=$2,home_id=$2 WHERE id=$1")
                .bind(skill.id)
                .bind(c.viewer.person.personal_org)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change == "unchanged" {
            let rows: Vec<Listed<Skill>> = accepted(reply);
            assert_eq!(visible(&rows), vec![skill.id]);
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("legacy-publication-read"));
        }
        assert_eq!(custody(&world, skill.id).await, completed);
    }
}
