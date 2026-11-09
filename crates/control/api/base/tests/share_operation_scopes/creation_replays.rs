//! Every native share cut replays accepted identities through current response Reads only.
use super::*;

async fn send_key(
    s: &Scene,
    token: &str,
    header: &str,
    body: &ShareWithAgents,
) -> Reply<ShareAgentCreateResponse> {
    let call = s
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(token)
        .header("Idempotency-Key", header)
        .body(body);
    call.send().await
}

async fn journal(s: &Scene, header: &str) -> (Json, String) {
    sqlx::query_as("SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2")
        .bind(s.ada.id())
        .bind(header)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

struct Accepted {
    scene: Scene,
    target: Id,
    source: Id,
    body: ShareWithAgents,
    parcels: Arc<ParcelCalls>,
    id: Id,
    header: String,
}

async fn accept(operation: &str) -> Accepted {
    let s = Scene::new().await;
    let target = comment_target(&s, "Accepted share replay").await;
    let source = document(&s, &s.ada, "Original accepted source", "Native copy bytes").await;
    let snapshot = operation.contains("snapshot");
    let mut body = request(
        source.base.id,
        &[target],
        if snapshot {
            ShareMode::Snapshot
        } else {
            ShareMode::Live
        },
    );
    if operation.contains("read_first") {
        body.read_first = Some(true);
    }
    if operation.contains("viewers_read") {
        body.viewers_read = Some(true);
    }
    if operation.ends_with("snapshot_comment") {
        body.comment = Some("Use this accepted copy".into());
    }
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let targets = [
        ("document", source.base.id),
        ("thread", target),
        ("org", s.tree.org.base.id),
        ("project", s.tree.project.base.id),
        ("topic", s.tree.topic.base.id),
    ];
    let writer = key(
        &s.rig.world,
        s.ada.id(),
        targets
            .iter()
            .map(|(kind, id)| rule(operation, kind, *id, &["read", "write", "manage"]))
            .collect(),
    )
    .await;
    let header = new_id().to_string();
    assert_ne!(
        header, body.client_id,
        "the two native fences are independent"
    );
    let first = full(send_key(&s, &writer, &header, &body).await, 201);
    assert_eq!(first.shares.len(), 1);
    let id = first.shares[0].id;
    let token = comment_key_id(&s, &writer).await;
    orch_db::queries::identity::revoke_api_token(
        &mut *s.rig.world.conn().await,
        token,
        s.ada.id(),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE documents SET title=$2 WHERE id=$1")
        .bind(source.base.id)
        .bind("Current source title")
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    Accepted {
        scene: s,
        target,
        source: source.base.id,
        body,
        parcels,
        id,
        header,
    }
}

async fn unreadable_receipt(
    s: &Scene,
    operation: &str,
    header: &str,
    body: &ShareWithAgents,
    id: Id,
) {
    let ShareWhat::Document(source) = &body.what else {
        panic!("the actual document source");
    };
    let neighbour = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule(
            "AgentShareList",
            "document",
            source.document_id,
            &["read"],
        )],
    )
    .await;
    let receipt = send_key(s, &neighbour, header, body).await;
    assert_eq!(receipt.status, 201);
    let raw: Json = serde_json::from_slice(&receipt.raw).unwrap();
    let ShareAgentCreateResponse::Receipt(receipt) = receipt.ok() else {
        panic!("a neighbouring Read cannot disclose the accepted share");
    };
    assert_eq!(receipt.shares.len(), 1);
    assert_eq!(receipt.shares[0].id, id);
    assert!(receipt.shares[0].created);
    assert_eq!(
        receipt.shares[0].input_id.is_some(),
        operation.ends_with("snapshot_comment")
    );
    assert_eq!(
        raw,
        serde_json::to_value(receipt).unwrap(),
        "the acknowledgement contains only the typed receipt"
    );
}

async fn native_cut(operation: &str) {
    let Accepted {
        scene: s,
        target,
        source,
        body,
        parcels,
        id,
        header,
    } = accept(operation).await;
    let snapshot = operation.contains("snapshot");
    let targets = [
        ("document", source),
        ("thread", target),
        ("org", s.tree.org.base.id),
        ("project", s.tree.project.base.id),
        ("topic", s.tree.topic.base.id),
    ];
    let stored = journal(&s, &header).await;
    assert_eq!(stored.0["outcome"]["data"]["selection"], operation);
    let before = execution(&s, target).await;
    let entries = grants(&s, source).await;
    let lines = history(&s, target).await;
    let copies = parcels.kept.lock().unwrap().len();
    let writes = parcels.writes.lock().unwrap().len();
    let read = key(
        &s.rig.world,
        s.ada.id(),
        targets
            .iter()
            .map(|(kind, id)| rule(operation, kind, *id, &["read"]))
            .collect(),
    )
    .await;
    let current = full(send_key(&s, &read, &header, &body).await, 201);
    assert_eq!(current.shares.len(), 1);
    assert_eq!(current.shares[0].id, id);
    assert_eq!(current.shares[0].to.id, target);
    let rendered = serde_json::to_value(&current).unwrap();
    assert_eq!(
        rendered["shares"][0]["what"]["title"], "Current source title",
        "replay uses the native current source reference"
    );
    let fresh = new_id().to_string();
    assert_eq!(
        send_key(&s, &read, &fresh, &body).await.status,
        403,
        "response Read cannot admit a new creation"
    );
    unreadable_receipt(&s, operation, &header, &body, id).await;
    let mut changed = body.clone();
    changed.download = Some(true);
    assert_eq!(send_key(&s, &read, &header, &changed).await.status, 409);
    assert_eq!(journal(&s, &header).await, stored);
    assert_eq!(execution(&s, target).await, before);
    assert_eq!(grants(&s, source).await, entries);
    assert_eq!(history(&s, target).await, lines);
    assert_eq!(parcels.kept.lock().unwrap().len(), copies);
    assert_eq!(parcels.writes.lock().unwrap().len(), writes);
    assert!(parcels.withdrawn.lock().unwrap().is_empty());
    assert_eq!(copies, usize::from(snapshot));
    s.rig.engine.stop();
}

#[tokio::test]
async fn live_replays_under_a_new_current_read_key_without_regranting() {
    native_cut("ShareAgentCreate.live").await;
}
#[tokio::test]
async fn live_viewers_read_replays_without_repeating_viewer_grants() {
    native_cut("ShareAgentCreate.live_viewers_read").await;
}
#[tokio::test]
async fn live_read_first_replays_without_recipient_control() {
    native_cut("ShareAgentCreate.live_read_first").await;
}
#[tokio::test]
async fn live_read_first_viewers_read_replays_without_either_write() {
    native_cut("ShareAgentCreate.live_read_first_viewers_read").await;
}
#[tokio::test]
async fn snapshot_replays_current_metadata_without_a_second_copy() {
    native_cut("ShareAgentCreate.snapshot").await;
}
#[tokio::test]
async fn snapshot_comment_replays_without_a_second_copy_or_input() {
    native_cut("ShareAgentCreate.snapshot_comment").await;
}
