//! Real webhook delivery follows discussion renewal and drops document words after current reading is withdrawn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use axum::{Router, extract::State, http::StatusCode, routing::post};
use desk::{Desk, text};
use orch_api_contract::access::{PageChange, PageChangeWhole, PermissionPageUpdate};
use orch_api_contract::documents::{DocumentDiscussionCreate, DocumentDiscussionCreateBody};
use orch_db::Db;
use orch_db::queries::notifications::channels::{self, NewChannel};
use orch_db::queries::notifications::routes::{self, RouteChange};
use orch_db::secrets::MasterKey;
use orch_notifier_contract::delivery::DEFAULT_RETRY_SCHEDULE;
use orch_notifier_webhook::WebhookNotifier;
use orch_types::new_id;
use orch_worker_core::channels::{Notifiers, Surroundings};
use orch_worker_core::heartbeat::Metrics;
use orch_worker_core::outbox::{Outbox, OutboxParts};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

/// This test takes one of twenty package-local loopback ports and excludes the shared world.
const RECEIVER_PORTS: std::ops::RangeInclusive<u16> = 41630..=41649;

async fn receive(State(messages): State<Arc<Mutex<Vec<String>>>>, body: String) -> StatusCode {
    messages.lock().await.push(body);
    StatusCode::OK
}

#[tokio::test]
async fn pw_01_a_real_route_delivers_each_accepted_discussion_once_and_drops_a_revoked_readers_pending_words()
 {
    let desk = Desk::new().await;
    let writer = desk.member("writer", "interactor").await;
    let reader = desk.member("reader", "viewer").await;
    let doc = desk.make(&desk.ada, "Plan").await;
    let blocks = desk
        .paragraphs(&writer, doc.base.id, &["The plan is ready."])
        .await;
    let mut listener = None;
    for port in RECEIVER_PORTS {
        if let Ok(bound) =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await
        {
            listener = Some(bound);
            break;
        }
    }
    let listener = listener.expect("a package-local loopback port");
    let port = listener.local_addr().unwrap().port();
    let messages = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/", post(receive))
        .with_state(messages.clone());
    let (finish, done) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = done.await;
            })
            .await
            .unwrap();
    });
    desk.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    let mut conn = desk.world.conn().await;
    // Only the local transport is seeded: the real notifier still checks the declared private-host policy.
    let channel = channels::insert_channel(
        &mut conn,
        &NewChannel {
            id: new_id(),
            org_id: None,
            user_id: Some(writer.id()),
            kind: "webhook",
            name: "Writer's webhook",
            config: &json!({ "url": format!("http://127.0.0.1:{port}/") }),
            templates: &json!({}),
            enabled: true,
            created_by: writer.id(),
            at: desk.world.now(),
        },
    )
    .await
    .unwrap();
    routes::put_route(
        &mut conn,
        new_id(),
        writer.id(),
        channel.id,
        &RouteChange {
            enabled: Some(true),
            kinds: None,
            unread_delay_minutes: None,
            min_interval_minutes: None,
        },
        desk.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let mut notifiers = Notifiers::new();
    notifiers
        .register(Arc::new(WebhookNotifier::new()))
        .unwrap();
    let outbox = Outbox::new(OutboxParts {
        db: Db::from_pool(desk.world.db.pool.clone()),
        clock: desk.world.app.app.clock.clone(),
        notifiers,
        master: MasterKey::new([7; 32]),
        surroundings: Surroundings {
            system_name: "Orch".into(),
            public_url: "https://orch.example".into(),
        },
        retry: DEFAULT_RETRY_SCHEDULE.to_vec(),
        metrics: Arc::new(Metrics::new()),
    });
    let body = DocumentDiscussionCreateBody {
        block_id: blocks[0].id,
        quote: None,
        content: text("Why this plan?"),
        client_id: new_id().to_string(),
    };
    let open = |body: DocumentDiscussionCreateBody| {
        let request = desk
            .world
            .app
            .call(DocumentDiscussionCreate, &[&doc.base.id])
            .token(&reader.token)
            .body(&body);
        async move { request.send().await.ok() }
    };
    let first = open(body.clone()).await;
    let repeat = open(body.clone()).await;
    assert_eq!(first.id, repeat.id);
    let delivered = outbox.run().await.unwrap();
    assert_eq!((delivered.sent, delivered.failed), (1, 0));
    assert_eq!(messages.lock().await.len(), 1);
    assert!(messages.lock().await[0].contains("Why this plan?"));
    assert_eq!(
        outbox.run().await.unwrap().total(),
        0,
        "a replay produces no second delivery"
    );
    let mut second = body.clone();
    second.client_id = new_id().to_string();
    second.content = text("This pending question stays private");
    open(second).await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let dropped = outbox.run().await.unwrap();
    // What the reader may no longer read is cancelled, which is no failed delivery.
    assert_eq!((dropped.sent, dropped.failed), (0, 0));
    let cancelled: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT o.cancel_reason FROM outbox o JOIN notification_routes r ON r.id = o.route_id
          WHERE r.user_id = $1 AND o.state = 'cancelled'",
    )
    .bind(writer.id())
    .fetch_all(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(cancelled, [Some("lost_access".to_owned())]);
    assert_eq!(
        messages.lock().await.len(),
        1,
        "no pending document body crosses lost View"
    );
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Reset),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let mut third = body;
    third.client_id = new_id().to_string();
    third.content = text("A new discussion after reading returns");
    open(third).await;
    assert_eq!(outbox.run().await.unwrap().sent, 1);
    assert_eq!(
        messages.lock().await.len(),
        2,
        "restored access admits new work without replaying dropped work"
    );
    assert!(
        messages
            .lock()
            .await
            .iter()
            .all(|body| !body.contains("This pending question stays private"))
    );
    finish.send(()).unwrap();
    server.await.unwrap();
}
