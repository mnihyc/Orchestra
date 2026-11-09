//! A built-in conversation's material is controlled by both its personal storage and knowledge roots.
mod places;
mod support;
use orch_api_contract::{
    access::NodeDefaultsUpdate,
    ask::{AskCreate, AskCreateBody, AskKeep, AskKeepBody},
};
use orch_types::{Id, Json};
use serde_json::json;
use support::World;
type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn policy(world: &World, user: Id, root: Id, value: bool) -> Proof {
    let mut tx = world.db.pool.begin().await?;
    orch_settings::write::write(
        &mut tx,
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::User(user),
            target: orch_settings::Target::Node(root),
            key: orch_settings_contract::registry::keys::SHARING_ALLOW_COPY_OUT,
            value: json!(value),
            locked: None,
            expected_revision: None,
            may_lock: false,
            origin: &Default::default(),
        },
        world.now(),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn ask_copy_requires_both_roots_names_the_refusing_root_and_records_each_exiting_root()
-> Proof {
    let world = World::new().await;
    let person = places::person(&world, "two-root-person").await;
    let knowledge = places::make_org(&world, &person, "Knowledge", "knowledge")
        .await
        .base
        .id;
    let recipient = places::make_org(&world, &person, "Recipient", "recipient")
        .await
        .base
        .id;
    let project = places::make_project(&world, &person, recipient, "Copies")
        .await
        .base
        .id;
    let topic = places::make_topic(&world, &person, project, "Kept copies")
        .await
        .base
        .id;
    let original = world
        .app
        .call(AskCreate, &[])
        .token(&person.token)
        .body(&AskCreateBody {
            root_org_id: Some(knowledge),
            context: None,
            model: None,
            retention_days: None,
        })
        .send()
        .await
        .ok();
    let source = original.conversation_id;
    let storage: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
        .bind(source)
        .fetch_one(&world.db.pool)
        .await?;
    assert_ne!(storage, knowledge);
    assert_ne!(storage, recipient);
    world.app.call(NodeDefaultsUpdate, &[&knowledge]).token(&person.token)
        .raw_body(&json!({"rows":[{"permission":"thread.export","profile":"managed","inherit":false,"locked":true,
            "add":[{"holder":{"kind":"user","id":person.id()}}]}]})).send().await.ok();
    let request = AskKeepBody {
        title: Some("Two roots".into()),
        topic_id: Some(topic),
        ..Default::default()
    };
    policy(&world, person.id(), knowledge, false).await?;
    let refused = world
        .app
        .call(AskKeep, &[&source])
        .token(&person.token)
        .body(&request)
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 403, "{:?}", refused);
    assert_eq!(refused.problem().extra["source_root"], json!(knowledge));
    policy(&world, person.id(), knowledge, true).await?;
    policy(&world, person.id(), storage, false).await?;
    let refused = world
        .app
        .call(AskKeep, &[&source])
        .token(&person.token)
        .body(&request)
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 403, "{:?}", refused);
    assert_eq!(refused.problem().extra["source_root"], json!(storage));
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id=$1 AND kind='thread'")
            .bind(topic)
            .fetch_one(&world.db.pool)
            .await?;
    assert_eq!(before, 0);
    policy(&world, person.id(), storage, true).await?;
    let copied = world
        .app
        .call(AskKeep, &[&source])
        .token(&person.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_ne!(copied.thread_id, source);
    assert_eq!(copied.topic_id, topic);
    let metadata: (Id, String) = sqlx::query_as("SELECT org_id,name FROM nodes WHERE id=$1")
        .bind(copied.thread_id)
        .fetch_one(&world.db.pool)
        .await?;
    assert_eq!(
        metadata,
        (recipient, "Native conversation across two roots".into())
    );
    let audit: Vec<(Option<Id>, Json)> = sqlx::query_as("SELECT org_id,details FROM audit_log WHERE action='content.copied_out' AND object_id=$1 ORDER BY org_id")
        .bind(source).fetch_all(&world.db.pool).await?;
    assert_eq!(audit.len(), 2);
    assert!(audit.iter().any(|row| row.0 == Some(storage)));
    assert!(audit.iter().any(|row| row.0 == Some(knowledge)));
    for (_, detail) in audit {
        assert_eq!(detail["created_id"], json!(copied.thread_id));
        assert_eq!(detail["destination_org_id"], json!(recipient));
    }
    Ok(())
}

#[tokio::test]
async fn private_ask_ownership_cannot_replace_the_governing_export_row() -> Proof {
    let world = World::new().await;
    let person = places::person(&world, "root-export-person").await;
    let knowledge = places::make_org(&world, &person, "Knowledge", "knowledge")
        .await
        .base
        .id;
    let recipient = places::make_org(&world, &person, "Recipient", "recipient")
        .await
        .base
        .id;
    let project = places::make_project(&world, &person, recipient, "Copies")
        .await
        .base
        .id;
    let topic = places::make_topic(&world, &person, project, "Copied conversation")
        .await
        .base
        .id;
    let original = world
        .app
        .call(AskCreate, &[])
        .token(&person.token)
        .body(&AskCreateBody {
            root_org_id: Some(knowledge),
            context: None,
            model: None,
            retention_days: None,
        })
        .send()
        .await
        .ok();
    world.app.call(NodeDefaultsUpdate, &[&knowledge]).token(&person.token)
        .raw_body(&json!({"rows":[{"permission":"thread.export","profile":"managed","inherit":false,"locked":true},{"permission":"thread.manage","profile":"managed","inherit":false,"locked":true}]}))
        .send().await.ok();
    let request = AskKeepBody {
        topic_id: Some(topic),
        ..Default::default()
    };
    let refused = world
        .app
        .call(AskKeep, &[&original.conversation_id])
        .token(&person.token)
        .body(&request)
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 403, "{:?}", refused);
    let problem = refused.problem();
    assert_eq!(problem.extra["source_root"], json!(knowledge));
    assert_eq!(problem.extra["required"], "thread.export");
    assert_eq!(problem.extra["page"], "defaults");
    let copied: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id=$1 AND kind='thread'")
            .bind(topic)
            .fetch_one(&world.db.pool)
            .await?;
    assert_eq!(copied, 0);
    Ok(())
}

#[tokio::test]
async fn the_same_storage_and_knowledge_root_is_a_single_export_and_audit_boundary() -> Proof {
    let world = World::new().await;
    let person = places::person(&world, "one-root-person").await;
    let root: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(person.id())
        .fetch_one(&world.db.pool)
        .await?;
    let recipient = places::make_org(&world, &person, "Recipient", "recipient")
        .await
        .base
        .id;
    let project = places::make_project(&world, &person, recipient, "Copies")
        .await
        .base
        .id;
    let topic = places::make_topic(&world, &person, project, "Copied conversation")
        .await
        .base
        .id;
    let original = world
        .app
        .call(AskCreate, &[])
        .token(&person.token)
        .body(&AskCreateBody {
            root_org_id: Some(root),
            context: None,
            model: None,
            retention_days: None,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(NodeDefaultsUpdate, &[&root])
        .token(&person.token)
        .raw_body(
            &json!({"rows":[{"permission":"thread.export","profile":"managed","inherit":false,
            "add":[{"holder":{"kind":"user","id":person.id()}}]}]}),
        )
        .send()
        .await
        .ok();
    let copied = world
        .app
        .call(AskKeep, &[&original.conversation_id])
        .token(&person.token)
        .body(&AskKeepBody {
            topic_id: Some(topic),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let events: Vec<Option<Id>> = sqlx::query_scalar(
        "SELECT org_id FROM audit_log WHERE action='content.copied_out' AND object_id=$1",
    )
    .bind(original.conversation_id)
    .fetch_all(&world.db.pool)
    .await?;
    assert_eq!(events, vec![Some(root)]);
    let stored: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
        .bind(copied.thread_id)
        .fetch_one(&world.db.pool)
        .await?;
    assert_eq!(stored, recipient);
    Ok(())
}
