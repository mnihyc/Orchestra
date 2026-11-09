//! The ordinary new-draft producer keeps its actual HTTP guard through native SQL waits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use super::*;
use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use crate::{rig::Scene, testing::RawReply};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Hash, Json, new_id};
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const NAME: &str = "native-draft-boundary";
const DESCRIPTION: &str = "A real newly supplied native draft.";
fn body(home: Id) -> api::SkillCreateBody {
    api::SkillCreateBody {
        home_id: home,
        from: SkillSource::New {
            name: NAME.into(),
            description: DESCRIPTION.into(),
        },
        access: None,
    }
}
async fn ledger(scene: &Scene, hash: Hash) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('skills',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM skills s WHERE name=$1),'drafts',(SELECT jsonb_agg(to_jsonb(d) ORDER BY skill_id) FROM skill_drafts d JOIN skills s ON s.id=d.skill_id WHERE s.name=$1),'files',(SELECT jsonb_agg(to_jsonb(f) ORDER BY skill_id,path) FROM skill_draft_files f JOIN skills s ON s.id=f.skill_id WHERE s.name=$1),'blob',(SELECT to_jsonb(b) FROM blobs b WHERE hash=$2))").bind(NAME).bind(hash.as_bytes().as_slice()).fetch_one(scene.rig.db.pool()).await.unwrap()
}
async fn wait(scene: &Scene, pid: i32, query: &str, task: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT,async{loop{assert!(!task.is_finished(),"the native request must reach its real SQL wait");let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(query).fetch_one(scene.rig.db.pool()).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("observe the literal native blocker PID");
}
#[tokio::test]
async fn actual_new_skill_draft_keeps_current_http_deadline_after_blob_header_and_draft_file_waits()
{
    for stage in ["blob", "header", "draft"] {
        for expired in [false, true] {
            let scene = Scene::new().await;
            let hash = Hash::of(super::super::new_skill_file(NAME, DESCRIPTION).as_bytes());
            if stage == "blob" {
                let request = scene
                    .rig
                    .world
                    .app
                    .call(api::SkillCreate, &[])
                    .token(&scene.ada.token)
                    .body(&body(scene.tree.project.base.id));
                request.send().await.ok();
            }
            let secret = new_token(API_TOKEN_PREFIX);
            identity::create_api_token(
                &mut *scene.rig.world.conn().await,
                &NewApiToken {
                    id: new_id(),
                    user_id: scene.ada.id(),
                    name: "Native draft recording",
                    token_hash: &token_hash(&secret),
                    scopes: &["*".into()],
                    node_id: None,
                    at: scene.rig.world.now(),
                    expires_at: Some(scene.rig.world.now() + chrono::Duration::hours(1)),
                },
            )
            .await
            .unwrap();
            let before = ledger(&scene, hash).await;
            let mut blocker = scene.rig.db.pool().begin().await.unwrap();
            let pattern = match stage {
                "blob" => {
                    orch_db::queries::uploads::lock_blobs(&mut blocker, &[hash])
                        .await
                        .unwrap();
                    "%FROM blobs%FOR UPDATE%"
                }
                "header" => {
                    sqlx::query("LOCK TABLE skills IN SHARE MODE")
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                    "%INSERT INTO skills%"
                }
                _ => {
                    sqlx::query("LOCK TABLE skill_draft_files IN SHARE MODE")
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                    "%skill_draft_files%"
                }
            };
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = scene.rig.world.app.clone();
            let home = scene.tree.org.base.id;
            let task = tokio::spawn(async move {
                let request = app
                    .call(api::SkillCreate, &[])
                    .token(&secret)
                    .body(&body(home));
                request.send_bytes().await
            });
            wait(&scene, pid, pattern, &task).await;
            if expired {
                scene.rig.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
            if expired {
                assert!(
                    matches!(
                        reply.status,
                        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
                    ),
                    "{stage}: {}",
                    String::from_utf8_lossy(&reply.raw)
                );
                assert_eq!(
                    ledger(&scene, hash).await,
                    before,
                    "no partial Skill/page/blob/draft/pin after the actual {stage} wait"
                );
            } else {
                assert_eq!(
                    reply.status,
                    http::StatusCode::CREATED,
                    "{}",
                    String::from_utf8_lossy(&reply.raw)
                );
                let skill: Skill = serde_json::from_slice(&reply.raw).unwrap();
                assert_eq!(skill.name, NAME);
                let row = rows::lock_draft(&mut *scene.rig.world.conn().await, skill.id)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(row.based_on, None);
            }
        }
    }
}
