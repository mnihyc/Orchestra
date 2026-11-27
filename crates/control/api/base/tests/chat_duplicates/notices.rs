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
            address_seed: orch_api_contract::public_addresses::PublicAddresses {
                public_url: "https://orch.test".into(),
                alternate_urls: Vec::new(),
            },
        },
        retry: orch_notifier_contract::delivery::DEFAULT_RETRY_SCHEDULE.to_vec(),
        metrics: Arc::new(Metrics::new()),
    });
    Ok((worker, sink))
}
