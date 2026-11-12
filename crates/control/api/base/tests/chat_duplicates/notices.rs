//! Real native source notices are delivered by the standalone outbox under original raw identity and current copy Read.
use super::*;
use async_trait::async_trait;
use orch_db::{
    queries::notifications::{channels, routes},
    secrets::MasterKey,
};
use orch_notifier_contract::{
    delivery::{Config, DeliverError, Delivery},
    descriptor::NotifierDescriptor,
    notifier::Notifier,
};
use orch_worker_core::{
    channels::{Notifiers, Surroundings},
    heartbeat::Metrics,
    outbox::{Outbox, OutboxParts},
};
use std::sync::{Arc, Mutex};
pub(super) struct Sink(pub(super) Mutex<Vec<Delivery>>);
#[async_trait]
impl Notifier for Sink {
    fn describe(&self) -> NotifierDescriptor {
        serde_json::from_value(json!({"name":"copy-notice-script","title":"Copy notice script","summary":"Native copy notice delivery","targets":["person"],"fields":[],"owners":["user"],"single":false,"reaches":"channel","body":"text","templates":{"*":{"single":{"subject":"{{title}}","body":"{{body}}"},"digest":{"subject":"{{count}}","body":"{{#notifications}}{{title}}\n{{/notifications}}"}}}})).unwrap()
    }
    async fn check(&self, _: &Config) -> Result<Json, DeliverError> {
        Ok(Json::Null)
    }
    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        self.0.lock().unwrap().push(delivery);
        Ok(())
    }
}
pub(super) async fn worker(r: &Room, receiver: Id) -> Proof<(Outbox, Arc<Sink>)> {
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let mut notifiers = Notifiers::new();
    notifiers.register(sink.clone())?;
    let channel = channels::insert_channel(
        &mut *r.world.conn().await,
        &channels::NewChannel {
            id: new_id(),
            org_id: None,
            user_id: Some(receiver),
            kind: "copy-notice-script",
            name: "Copy notice route",
            config: &json!({}),
            templates: &json!({}),
            enabled: true,
            created_by: receiver,
            at: r.world.now(),
        },
    )
    .await?;
    routes::put_route(
        &mut *r.world.conn().await,
        new_id(),
        receiver,
        channel.id,
        &routes::RouteChange {
            enabled: Some(true),
            ..Default::default()
        },
        r.world.now(),
    )
    .await?;
    let worker = Outbox::new(OutboxParts {
        db: orch_db::Db::from_pool(r.world.db.pool.clone()),
        clock: r.world.app.app.clock.clone(),
        notifiers,
        master: MasterKey::new([7; 32]),
        surroundings: Surroundings {
            system_name: "Copy notices".into(),
            public_url: "https://orch.test".into(),
        },
        retry: orch_notifier_contract::delivery::DEFAULT_RETRY_SCHEDULE.to_vec(),
        metrics: Arc::new(Metrics::new()),
    });
    Ok((worker, sink))
}
#[tokio::test]
async fn copy_notices_recheck_the_original_key_and_source_read_without_repeating_creation() -> Proof
{
    use orch_api_contract::duplicates::ChatDuplicatePreview;
    for change in ["positive", "read_only", "borrowed", "revoked", "scope_lost"] {
        let r = Room::new().await;
        let source = r
            .group(&r.ada, &[&r.bea], Some("Native original source"))
            .await
            .base
            .id;
        r.say(&r.ada, source, "Contributor work").await;
        let (worker, sink) = worker(&r, r.ada.id()).await?;
        let review = r
            .world
            .app
            .call(ChatDuplicatePreview, &[&source])
            .token(&r.bea.token)
            .send()
            .await
            .ok();
        let actor = key(
            &r.world,
            r.bea.id(),
            vec![
                rule("ChatDuplicate", "chat", source, "self", "read"),
                rule("ChatDuplicate", "org", r.root, "self", "write"),
            ],
        )
        .await?;
        let reply = r
            .world
            .app
            .call(ChatDuplicate, &[&source])
            .token(&actor.secret)
            .body(&ChatDuplicateBody {
                copy: DuplicateBody::default(),
                notify: Some(true),
                notify_preview_id: review.preview_id,
            })
            .send_bytes()
            .await;
        id(&reply)?;
        match change {
            "read_only" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":[rule("ChatDuplicate","chat",source,"self","read")]}))
                    .execute(&r.world.db.pool)
                    .await?;
            }
            "borrowed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":[rule("ChatGet","chat",source,"self","read")]}))
                    .execute(&r.world.db.pool)
                    .await?;
            }
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(r.world.now())
                    .execute(&r.world.db.pool)
                    .await?;
            }
            "scope_lost" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":[]}))
                    .execute(&r.world.db.pool)
                    .await?;
            }
            _ => {}
        }
        r.world.advance(chrono::Duration::minutes(5));
        worker.run().await?;
        let delivered = sink.0.lock().unwrap().len();
        assert_eq!(
            delivered,
            usize::from(matches!(change, "positive" | "read_only")),
            "{change}"
        );
        assert_eq!(count(&r).await?, 2, "delivery never makes another group");
    }
    Ok(())
}
#[tokio::test]
async fn browser_copy_notice_keeps_the_real_raw_session_and_refuses_its_revocation_or_extended_old_lifetime()
-> Proof {
    use orch_api_contract::duplicates::ChatDuplicatePreview;
    for change in ["positive", "revoked", "expired"] {
        let r = Room::new().await;
        let source = r
            .group(&r.ada, &[&r.bea], Some("Raw browser source"))
            .await
            .base
            .id;
        r.say(&r.ada, source, "Browser source work").await;
        let (worker, sink) = worker(&r, r.ada.id()).await?;
        if change == "expired" {
            sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                .bind(orch_api_core::middleware::authentication::token_hash(
                    &r.bea.token,
                ))
                .bind(r.world.now() + chrono::Duration::minutes(1))
                .execute(&r.world.db.pool)
                .await?;
        }
        let review = r
            .world
            .app
            .call(ChatDuplicatePreview, &[&source])
            .token(&r.bea.token)
            .send()
            .await
            .ok();
        let reply = r
            .world
            .app
            .call(ChatDuplicate, &[&source])
            .token(&r.bea.token)
            .body(&ChatDuplicateBody {
                copy: DuplicateBody::default(),
                notify: Some(true),
                notify_preview_id: review.preview_id,
            })
            .send_bytes()
            .await;
        id(&reply)?;
        if change == "revoked" {
            sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)").bind(orch_api_core::middleware::authentication::token_hash(&r.bea.token)).bind(r.world.now()).execute(&r.world.db.pool).await?;
        }
        if change == "expired" {
            sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                .bind(orch_api_core::middleware::authentication::token_hash(
                    &r.bea.token,
                ))
                .bind(r.world.now() + chrono::Duration::hours(1))
                .execute(&r.world.db.pool)
                .await?;
        }
        r.world.advance(chrono::Duration::minutes(5));
        worker.run().await?;
        assert_eq!(
            sink.0.lock().unwrap().len(),
            usize::from(change == "positive"),
            "{change}"
        );
    }
    Ok(())
}
