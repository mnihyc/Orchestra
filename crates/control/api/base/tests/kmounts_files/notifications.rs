//! Availability changes notify only their accepted audience and only by explicit choice.
use super::*;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::{FactTarget, ResourceImpactRead};
use orch_db::queries::resources::access_fixtures as fixtures;

#[tokio::test]
async fn changing_mount_availability_leaves_the_current_people_to_use_their_existing_bindings() {
    for enabled in [false, true] {
        {
            let world = World::new().await;
            let people = company(&world).await;
            let mount = named_mount(&world, &people.boss, people.acme, "shared-data").await;
            sqlx::query("UPDATE mounts SET enabled=$2 WHERE id=$1")
                .bind(mount.base.id)
                .bind(!enabled)
                .execute(&world.db.pool)
                .await
                .unwrap();
            for name in ["First dependent", "Second dependent"] {
                let environment = fixtures::insert_environment(
                    &mut *world.conn().await,
                    people.viewer.id(),
                    Some(people.acme),
                    name,
                    "connected",
                )
                .await
                .unwrap();
                sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
                    .bind(environment)
                    .bind(json!({"env.mounts": {mount.base.id.to_string(): true}}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
                own_page(
                    &world,
                    orch_access_contract::permissions::ResourceKind::Environment,
                    environment,
                    people.viewer.id(),
                )
                .await;
            }
            let excluded = fixtures::insert_environment(
                &mut *world.conn().await,
                people.stranger.id(),
                Some(people.acme),
                "Explicitly excluded",
                "connected",
            )
            .await
            .unwrap();
            sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
                .bind(excluded)
                .bind(json!({"env.mounts": {mount.base.id.to_string(): false}}))
                .execute(&world.db.pool)
                .await
                .unwrap();
            own_page(
                &world,
                orch_access_contract::permissions::ResourceKind::Environment,
                excluded,
                people.stranger.id(),
            )
            .await;
            let preview = world
                .app
                .call(ResourceImpactRead, &[])
                .token(&people.boss.token)
                .body(&FactTarget {
                    object: ObjectRef {
                        kind: ObjectKind::Mount,
                        id: mount.base.id,
                    },
                    model_id: None,
                })
                .send()
                .await
                .ok();
            assert!(
                preview.notification.is_none(),
                "a mount edit needs no notice audience"
            );
            let mut revision = mount.revision;
            for repeat in [false, true] {
                let body = json!({"expected_revision": revision, "enabled": enabled});
                let reply = world
                    .app
                    .call(MountUpdate, &[&mount.base.id])
                    .token(&people.boss.token)
                    .raw_body(&body)
                    .send_bytes()
                    .await;
                assert_eq!(
                    reply.status,
                    http::StatusCode::OK,
                    "{}",
                    String::from_utf8_lossy(&reply.raw)
                );
                let current: Mount = serde_json::from_slice(&reply.raw).unwrap();
                assert_eq!(current.enabled, enabled);
                assert_eq!(current.revision, revision + i64::from(!repeat));
                revision = current.revision;
                let notices: Vec<serde_json::Value> = sqlx::query_scalar("SELECT data FROM notifications WHERE user_id=$1 AND kind='environment.attention'")
                    .bind(people.viewer.id()).fetch_all(&world.db.pool).await.unwrap();
                assert!(notices.is_empty());
            }
            let unrelated: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=ANY($1) AND kind='environment.attention'")
                .bind(vec![people.stranger.id(), people.boss.id()]).fetch_one(&world.db.pool).await.unwrap();
            assert_eq!(
                unrelated, 0,
                "unaffected people and the actor receive no operation notice"
            );
        }
    }
}
