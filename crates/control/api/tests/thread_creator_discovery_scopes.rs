//! Cold modern creators reuse ordinary descriptor preparation, without creating under a
//! credential that was revoked while the harness was answering.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod creator_support;
mod places;
mod rig;
mod support;

use creator_support::{body, key, rule, thread};
use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
use orch_api_contract::share::{ShareDocumentsLevel, ShareParts};
use orch_link_contract::commands::CommandKind;
use orch_types::Id;
use rig::Scene;

async fn cold(s: &Scene) {
    sqlx::query("UPDATE agents SET descriptor='{}' WHERE id=$1")
        .bind(s.on.agent)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

async fn token(s: &Scene, actor: Id) -> String {
    key(
        s,
        actor,
        vec![rule(
            "ThreadCreate",
            "topic",
            s.tree.topic.base.id,
            "subtree",
            &["thread"],
            &["write", "manage"],
        )],
    )
    .await
}

#[tokio::test]
async fn a_permitted_cold_existing_environment_creator_discovers_before_its_this_thread_birth() {
    let s = Scene::new().await;
    cold(&s).await;
    let denied = token(&s, s.dan.id()).await;
    let (status, reply) = thread(&s, &denied, s.tree.topic.base.id, &body(&s)).await;
    assert_eq!(
        status, 403,
        "ordinary resource Use is still required: {reply}"
    );
    assert!(
        s.rig.seen.of_kind(CommandKind::AgentStart).is_empty(),
        "a caller without the existing binding's ordinary use cannot start discovery"
    );
    let token = token(&s, s.ada.id()).await;
    let mut body = body(&s);
    body.agent_access = Some(AgentAccessChoice {
        where_: AgentAccessWhere::Thread,
        parts: ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..Default::default()
        },
    });
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &body).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(s.rig.seen.of_kind(CommandKind::AgentStart).len(), 1);
    let id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let stored = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.environment_id, Some(s.on.environment));
}

#[tokio::test]
async fn revoking_the_actual_key_during_cold_discovery_prevents_the_thread_birth() {
    let s = Scene::new().await;
    cold(&s).await;
    let token = token(&s, s.ada.id()).await;
    let token_id: Id = sqlx::query_scalar("SELECT id FROM api_tokens WHERE user_id=$1")
        .bind(s.ada.id())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let requested = body(&s);
    s.rig.machine.handler.hold(CommandKind::AgentStart);
    let revoke = async {
        rig::eventually("the scoped creator starts descriptor discovery", || async {
            s.rig
                .machine
                .handler
                .executions()
                .iter()
                .any(|command| command.kind == CommandKind::AgentStart)
        })
        .await;
        let mut tx = s.rig.db.begin().await.unwrap();
        orch_db::queries::identity::revoke_api_token(
            &mut tx,
            token_id,
            s.ada.id(),
            s.rig.world.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        s.rig.machine.handler.release();
    };
    let request = async {
        let result = thread(&s, &token, s.tree.topic.base.id, &requested).await;
        assert_eq!(
            result.0, 401,
            "the request must discover before rejecting the revoked key: {}",
            result.1
        );
        result
    };
    let ((status, reply), ()) =
        tokio::time::timeout(rig::PATIENCE, async { tokio::join!(request, revoke) })
            .await
            .expect("discovery releases credential locks before waiting for the harness");
    assert_eq!(status, 401, "{reply}");
    assert_eq!(reply["code"], "auth.invalid_token");
    let created: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        created, 0,
        "a revoked creator key cannot complete a thread birth"
    );
}

#[tokio::test]
async fn a_cold_existing_managed_generation_zero_uses_the_actual_provider_without_provisioning_it()
{
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ObjectKind;
    use orch_authz::{
        Caller, Principal,
        ownership::{self, NewAccess},
    };
    use orch_db::queries::{environments::thread_engine as environments, jobs};
    use orch_engine::environments::{self as producer, Request};
    use orch_types::{Time, new_id};
    use serde_json::json;

    let s = Scene::new().await;
    let (host, template) = creator_support::managed(&s).await;
    let now = s.rig.world.now();
    let runner = new_id();
    let mut tx = s.rig.db.begin().await.unwrap();
    let principal = Principal::load(&mut tx, Caller::session(s.ada.id()), Time::new(now))
        .await
        .unwrap();
    let made = producer::create(
        &mut tx,
        now,
        &principal,
        &Request {
            owner: s.ada.id(),
            name: Some("Never activated existing workspace"),
            home: s.tree.topic.base.id,
            template: Some(template),
            host: Some(Some(host)),
            config: &json!({}),
        },
    )
    .await
    .unwrap();
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: made.id,
        },
        s.ada.id(),
        NewAccess::Defaults,
        Time::new(now),
    )
    .await
    .unwrap();
    // Claim the real queued provision job before it is visible to the supervisor. Its lease
    // uses this fixture's existing maximum wait; no fake runtime or generation is installed.
    assert!(
        jobs::claim(
            &mut tx,
            made.job.id,
            runner,
            now + chrono::Duration::from_std(rig::PATIENCE).unwrap(),
            now
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
    cold(&s).await;
    let token = token(&s, s.ada.id()).await;
    let mut requested = body(&s);
    requested.environment = Some(orch_api_contract::binding::EnvironmentChoice::Use(made.id));
    requested.binding.as_mut().unwrap().arrangement =
        Some(orch_api_contract::binding::ArrangementType::Separated);
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &requested).await;
    // Teardown releases exactly this fixture's claim only after its supervisor is stopped.
    // Keep that ordering even when the response assertion below reports a regression.
    s.rig.engine.stop();
    jobs::release(
        &mut *s.rig.world.conn().await,
        made.job.id,
        runner,
        s.rig.world.now(),
    )
    .await
    .unwrap();
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(s.rig.seen.of_kind(CommandKind::AgentStart).len(), 1);
    let created: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let mut conn = s.rig.world.conn().await;
    let stored = orch_db::queries::threads::thread(&mut conn, created)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.environment_id, Some(made.id));
    let target = environments::environment(&mut conn, made.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        target.generation, 0,
        "descriptor discovery never guesses an activated generation"
    );
    assert_eq!(target.machine_id, None);
    let runtimes: i64 = sqlx::query_scalar("SELECT count(*) FROM runtimes WHERE environment_id=$1")
        .bind(made.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        runtimes, 0,
        "only the provider's actual existing machine answers discovery"
    );
}
