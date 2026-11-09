//! Deterministic snapshot boundaries with the native HTTP, machine and thread projections.

use super::{
    Inner,
    sync::{Cache, Frame, Rows},
    threads,
};
use crate::rig::{self, PATIENCE, Scene};
use crate::support::FullApprovalReply;
use crate::support::FullPermissionPageReply;
use orch_api_contract::approvals::{
    ApprovalDecide, ApprovalDecision, ApprovalState, HarnessOptionDecision,
};
use orch_api_contract::threads::{
    Delivery, ItemPayload, ThreadOpeningGet, ThreadOpeningGetQuery, ThreadState, ThreadSync,
    ThreadSyncGet, ThreadSyncGetQuery, TurnState,
};
use orch_link_contract::commands::TurnStart;
use orch_link_contract::events::TurnOutcome;
use orch_realtime_contract::feeds::{SyncData, ThreadData};
use orch_types::{Id, Seq};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::oneshot;

use crate::{desk as doc_desk, native_close_support as close_fixture, thread_history_scope};

#[tokio::test]
async fn document_changes_held_projection_rechecks_actual_sources_before_publication() -> Proof {
    use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use crate::routes::documents::build::WrittenLabelSource;
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_contract::common::ErrorCode;
    use orch_api_contract::documents::{
        BlockContent, DocumentChangesGet, DocumentChangesGetQuery, Inline, InlineRefKind,
    };
    use orch_db::queries::identity::{self, NewApiToken};
    use serde_json::json;

    const FOREIGN: &str = "Actual changes reference held before publication";
    let desk = doc_desk::Desk::new().await;
    let document = desk.make(&desk.ada, "Held comparison").await.base.id;
    let foreign = desk.make(&desk.ada, FOREIGN).await.base.id;
    let mut block = doc_desk::paragraph("Stored comparison words ");
    let Some(BlockContent::Inline(runs)) = &mut block.content else {
        return Err("paragraph absent".into());
    };
    runs.push(Inline::Ref {
        display: None,
        kind: InlineRefKind::Document,
        id: foreign,
        path: None,
        part: None,
        label: Some("Stored reference words".into()),
        name: None,
    });
    desk.write(&desk.ada, document, vec![doc_desk::put(block)])
        .await
        .ok();
    let rule = |id| {
        json!({
            "target":{"kind":"document","id":id,"reach":"self"},
            "allow":[{"operation":"DocumentChangesGet.read","actions":["read"]}]
        })
    };
    let scope: KeyScope = serde_json::from_value(json!({"rules":[rule(document),rule(foreign)]}))?;
    let token = orch_types::new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: token,
            user_id: desk.ada.id(),
            name: "Held native changes read",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope)?),
    )
    .await?;
    let query = DocumentChangesGetQuery {
        inspect_deleted: None,
        from: "0".into(),
        to: None,
    };
    let control = desk
        .world
        .app
        .call(DocumentChangesGet, &[&document])
        .token(&secret)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(control.status.as_u16(), 200);
    assert!(String::from_utf8_lossy(&control.raw).contains(FOREIGN));
    let (observed, held) = oneshot::channel();
    let read = desk
        .world
        .app
        .call(DocumentChangesGet, &[&document])
        .token(&secret)
        .query(&query)
        .send_bytes();
    let write = async {
        let witness: DocumentWitness = held.await?;
        assert_eq!(witness.document, document);
        assert!(witness.limited);
        assert!(
            witness
                .references
                .contains(&WrittenLabelSource::Document(foreign)),
            "the native changes projection must actually hold its foreign source"
        );
        let narrowed: KeyScope = serde_json::from_value(json!({"rules":[rule(document)]}))?;
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token)
            .bind(serde_json::to_value(narrowed)?)
            .execute(&desk.world.db.pool)
            .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (reply, ()) =
        interleave_with_document(Cut::DocumentResponse, read, write, Some(observed)).await?;
    assert_eq!(reply.status.as_u16(), 403);
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    assert!(!String::from_utf8_lossy(&reply.raw).contains(FOREIGN));
    Ok(())
}

#[tokio::test]
async fn document_list_held_page_rechecks_its_actual_named_sources() -> Proof {
    use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use crate::routes::documents::build::WrittenLabelSource;
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_contract::documents::{NodeDocumentList, NodeDocumentListQuery};
    use orch_db::queries::identity::{self, NewApiToken};
    use serde_json::json;

    let desk = doc_desk::Desk::new().await;
    let document = desk
        .make(&desk.ada, "Held selected list content")
        .await
        .base
        .id;
    let rule = |kind, id| {
        json!({
            "target":{"kind":kind,"id":id,"reach":"self"},
            "allow":[{"operation":"NodeDocumentList.read","actions":["read"]}]
        })
    };
    let full: KeyScope = serde_json::from_value(json!({"rules":[
        rule("document",document),rule("topic",desk.topic)
    ]}))?;
    let token = orch_types::new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: token,
            user_id: desk.ada.id(),
            name: "Held native list read",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(full)?),
    )
    .await?;
    let query = NodeDocumentListQuery::default();
    let control = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&secret)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(control.status.as_u16(), 200);
    let body: serde_json::Value = serde_json::from_slice(&control.raw)?;
    assert_eq!(body["items"][0]["node"]["name"], "Plans");
    let (observed, held) = oneshot::channel();
    let read = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&secret)
        .query(&query)
        .send_bytes();
    let write = async {
        let witness: DocumentWitness = held.await?;
        assert_eq!(witness.document, document);
        assert!(witness.limited);
        assert!(
            witness
                .references
                .contains(&WrittenLabelSource::Node(desk.topic)),
            "the held page must actually contain the native topic label source"
        );
        let narrow: KeyScope =
            serde_json::from_value(json!({"rules":[rule("document",document)]}))?;
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token)
            .bind(serde_json::to_value(narrow)?)
            .execute(&desk.world.db.pool)
            .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (reply, ()) =
        interleave_with_document(Cut::DocumentResponse, read, write, Some(observed)).await?;
    assert_eq!(reply.status.as_u16(), 403);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Held selected list content"));
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Plans"));
    Ok(())
}

#[tokio::test]
async fn legacy_document_reply_rechecks_held_sources_after_the_same_key_becomes_modern() -> Proof {
    use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use crate::routes::documents::build::WrittenLabelSource;
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_contract::common::ErrorCode;
    use orch_api_contract::documents::{
        BlockContent, DocumentSync, DocumentSyncGet, DocumentSyncGetQuery, Inline, InlineRefKind,
    };
    use orch_db::queries::identity::{self, NewApiToken};
    use serde_json::json;

    const FOREIGN: &str = "Foreign live title withheld after key mode conversion";
    fn live_name(sync: &DocumentSync, document: Id) -> Option<&str> {
        sync.blocks.iter().find_map(|block| {
            let BlockContent::Inline(runs) = &block.content else {
                return None;
            };
            runs.iter().find_map(|run| match run {
                Inline::Ref { id, name, .. } if *id == document => {
                    name.as_ref().and_then(|name| name.as_deref())
                }
                _ => None,
            })
        })
    }
    let desk = doc_desk::Desk::new().await;
    let document = desk
        .make(
            &desk.ada,
            "Legacy response held before its real source finish",
        )
        .await
        .base
        .id;
    let foreign = desk.make(&desk.ada, FOREIGN).await.base.id;
    let mut block = doc_desk::paragraph("The stored label is document content. ");
    let Some(BlockContent::Inline(runs)) = &mut block.content else {
        return Err("paragraph absent".into());
    };
    runs.push(Inline::Ref {
        display: None,
        kind: InlineRefKind::Document,
        id: foreign,
        path: None,
        part: None,
        label: Some("Stored copied label".into()),
        name: None,
    });
    desk.write(&desk.ada, document, vec![doc_desk::put(block)])
        .await
        .ok();

    let token = orch_types::new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let scopes = vec!["*".to_owned()];
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: token,
            user_id: desk.ada.id(),
            name: "Same-key read response mode conversion",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await?;
    let ordinary = desk
        .world
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&secret)
        .query(&DocumentSyncGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(ordinary.status.as_u16(), 200);
    let wide: DocumentSync = serde_json::from_slice(&ordinary.raw)?;
    assert_eq!(live_name(&wide, foreign), Some(FOREIGN));
    let stored_scope: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
            .bind(token)
            .fetch_one(&desk.world.db.pool)
            .await?;
    assert!(
        stored_scope.is_none(),
        "the actual original credential is legacy"
    );

    let (observed, held) = oneshot::channel();
    let request = async {
        desk.world
            .app
            .call(DocumentSyncGet, &[&document])
            .token(&secret)
            .query(&DocumentSyncGetQuery::default())
            .send_bytes()
            .await
    };
    let write = async {
        let witness: DocumentWitness = held.await?;
        assert_eq!(witness.document, document);
        assert!(
            !witness.limited,
            "the held response was actually prepared under legacy mode"
        );
        assert!(
            witness.nodes.contains(&desk.topic),
            "wide bounds record its actual parent labels"
        );
        assert!(
            witness
                .references
                .contains(&WrittenLabelSource::Document(foreign)),
            "the actual held foreign reference must be recorded, not guessed empty"
        );
        let scope: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"document","id":document,"reach":"self"},
            "allow":[{"operation":"DocumentSyncGet.read","actions":["read"]}]
        }]}))?;
        assert!(scope.validate().is_empty());
        let changed =
            sqlx::query("UPDATE api_tokens SET scopes='{}'::text[], operation_scope=$2 WHERE id=$1 AND user_id=$3 AND node_id IS NULL")
                .bind(token)
                .bind(serde_json::to_value(scope)?)
                .bind(desk.ada.id())
                .execute(&desk.world.db.pool)
                .await?;
        assert_eq!(changed.rows_affected(), 1);
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (reply, ()) =
        interleave_with_document(Cut::DocumentResponse, request, write, Some(observed)).await?;
    assert_eq!(
        reply.status.as_u16(),
        403,
        "the old rich response cannot borrow its newly prepared narrow replacement: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    assert!(!String::from_utf8_lossy(&reply.raw).contains(FOREIGN));
    Ok(())
}

#[tokio::test]
async fn an_open_file_cannot_combine_old_scope_placement_with_new_ordinary_document_access() -> Proof
{
    use http_body_util::BodyExt;
    use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::access::PermissionPageUpdate;
    use orch_api_contract::documents::{
        DocumentFile, DocumentFileCreate, DocumentFileCreateQuery, DocumentFileGet, DocumentMove,
        DocumentMoveBody,
    };
    use orch_api_contract::threads::FILENAME_HEADER;
    use serde_json::json;

    const CONTENT: &[u8] = b"Original authenticated open document file bytes.\n";
    let desk = doc_desk::Desk::new().await;
    let viewer = desk.member("file_source_viewer", "viewer").await;
    let project_q = crate::places::make_project(&desk.world, &desk.ada, desk.org, "Project Q")
        .await
        .base
        .id;
    let topic_q = crate::places::make_topic(&desk.world, &desk.ada, project_q, "Topic Q")
        .await
        .base
        .id;
    let document = desk
        .make(&desk.ada, "Opened document moves from P to Q")
        .await
        .base
        .id;
    let file: DocumentFile = desk
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, "original.txt")
        .bytes(CONTENT)
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response");
    let ordinary = desk
        .world
        .app
        .call(DocumentFileGet, &[&document, &file.id])
        .token(&viewer.token)
        .send_bytes()
        .await;
    assert_eq!(ordinary.status.as_u16(), 200);
    assert_eq!(ordinary.raw.as_ref(), CONTENT);
    let key = thread_history_scope::key(
        &desk.world,
        viewer.id(),
        vec![thread_history_scope::rule(
            "DocumentFileGet.read",
            "project",
            desk.project,
            "subtree",
            "read",
        )],
    )
    .await?;
    let response = desk
        .world
        .app
        .call(DocumentFileGet, &[&document, &file.id])
        .token(&key.secret)
        .send_response()
        .await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "the actual file was opened under P"
    );
    let mut body = response.into_body();

    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .full_permission_page()
        .ok();
    let private = desk
        .world
        .app
        .call(DocumentFileGet, &[&document, &file.id])
        .token(&viewer.token)
        .send_bytes()
        .await;
    assert_eq!(private.status.as_u16(), 403);
    thread_history_scope::denied(&private);

    let (observed, held) = oneshot::channel();
    let read = DOCUMENT_AUTHORITY.scope(Mutex::new(Some(observed)), async { body.frame().await });
    let write = async {
        let witness: DocumentAuthorityWitness = held.await?;
        assert_eq!(witness.document, document);
        let document_facts: Vec<_> = witness
            .facets
            .iter()
            .filter(|facet| {
                facet.operation == "DocumentFileGet.read"
                    && facet.action == KeyAction::Read
                    && facet.resource_kind == KeyTargetKind::Object(ObjectKind::Document)
                    && facet.target.object.id == Some(document)
            })
            .collect();
        assert_eq!(
            document_facts.len(),
            1,
            "observe the actual governing root facet"
        );
        let old_path = document_facts[0]
            .target
            .node_path()
            .ok_or("root path absent")?;
        assert!(old_path.contains(&desk.project));
        assert!(!old_path.contains(&project_q));
        let moved = desk
            .world
            .app
            .call(DocumentMove, &[&document])
            .token(&desk.ada.token)
            .body(&DocumentMoveBody { node_id: topic_q })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        assert_eq!(moved.base.node.id, topic_q);
        desk.world
            .app
            .call(PermissionPageUpdate, &[&"documents", &document])
            .token(&desk.ada.token)
            .raw_body(&json!({"whole":"reset"}))
            .send()
            .await
            .full_permission_page()
            .ok();
        Ok(())
    };
    let (frame, ()) = interleave(Cut::DocumentAuthority, read, write).await?;
    let after = desk
        .world
        .app
        .call(DocumentFileGet, &[&document, &file.id])
        .token(&viewer.token)
        .send_bytes()
        .await;
    assert_eq!(
        after.status.as_u16(),
        200,
        "ordinary View is restored under actual Q"
    );
    assert_eq!(after.raw.as_ref(), CONTENT);
    let scoped_after = desk
        .world
        .app
        .call(DocumentFileGet, &[&document, &file.id])
        .token(&key.secret)
        .send_bytes()
        .await;
    assert_eq!(scoped_after.status.as_u16(), 403);
    thread_history_scope::denied(&scoped_after);
    // P has no ordinary View in the old snapshot; Q has View but is outside this key.
    // No coherent check authorizes these bytes, even though an RC cross-product would.
    assert!(
        matches!(frame, Some(Err(_))),
        "mixed old scope/new native rights exposed a frame"
    );
    Ok(())
}

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cut {
    HttpHead,
    HttpHistoryRows,
    CloseNameSources,
    RawContentReturn,
    SocketRows,
    DocumentResponse,
    DocumentAuthority,
}

struct Boundary {
    cut: Cut,
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
    document_witness: Option<oneshot::Sender<DocumentWitness>>,
    close_witness: Option<oneshot::Sender<CloseWitness>>,
}

tokio::task_local! {
    // A barrier belongs to this single test reader, never a process-wide request switch.
    static BOUNDARY: Mutex<Option<Boundary>>;
    // A separate witness slot leaves the existing response/row barriers unchanged.
    static DOCUMENT_AUTHORITY: Mutex<Option<oneshot::Sender<DocumentAuthorityWitness>>>;
}

fn take_boundary(cut: Cut) -> Option<Boundary> {
    BOUNDARY
        .try_with(|slot| {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|boundary| boundary.cut == cut) {
                slot.take()
            } else {
                None
            }
        })
        .ok()
        .flatten()
}

pub(crate) async fn pause(cut: Cut) {
    if let Some(boundary) = take_boundary(cut) {
        let _ = boundary.reached.send(());
        let _ = boundary.resume.await;
    }
}

#[derive(Debug)]
struct DocumentWitness {
    document: Id,
    limited: bool,
    nodes: Vec<Id>,
    references: Vec<crate::routes::documents::build::WrittenLabelSource>,
}

struct DocumentAuthorityWitness {
    document: Id,
    facets: Vec<orch_access_contract::key_scope::KeyFacet>,
}

/// Observes the actual admitted root facts before the ordinary document authorization query.
pub(crate) async fn document_authority_cut(document: Id, caller: &orch_authz::Caller) {
    let witness = DOCUMENT_AUTHORITY
        .try_with(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take())
        .ok()
        .flatten();
    if let Some(witness) = witness {
        let _ = witness.send(DocumentAuthorityWitness {
            document,
            facets: caller.admitted_facets().unwrap_or(&[]).to_vec(),
        });
        pause(Cut::DocumentAuthority).await;
    }
}

/// Records the actual held projection bounds at this test's one response boundary.
pub(crate) async fn document_response_cut(
    document: Id,
    limited: bool,
    nodes: Vec<Id>,
    references: Vec<crate::routes::documents::build::WrittenLabelSource>,
) {
    if let Some(boundary) = take_boundary(Cut::DocumentResponse) {
        if let Some(witness) = boundary.document_witness {
            let _ = witness.send(DocumentWitness {
                document,
                limited,
                nodes,
                references,
            });
        }
        let _ = boundary.reached.send(());
        let _ = boundary.resume.await;
    }
}

async fn interleave<R, W>(
    cut: Cut,
    read: impl Future<Output = R>,
    write: impl Future<Output = Proof<W>>,
) -> Proof<(R, W)> {
    interleave_with_document(cut, read, write, None).await
}

async fn interleave_with_document<R, W>(
    cut: Cut,
    read: impl Future<Output = R>,
    write: impl Future<Output = Proof<W>>,
    document_witness: Option<oneshot::Sender<DocumentWitness>>,
) -> Proof<(R, W)> {
    interleave_with_sources(cut, read, write, document_witness, None).await
}

async fn interleave_with_sources<R, W>(
    cut: Cut,
    read: impl Future<Output = R>,
    write: impl Future<Output = Proof<W>>,
    document_witness: Option<oneshot::Sender<DocumentWitness>>,
    close_witness: Option<oneshot::Sender<CloseWitness>>,
) -> Proof<(R, W)> {
    let (reached, wait) = oneshot::channel();
    let (resume, blocked) = oneshot::channel();
    let reader = BOUNDARY.scope(
        Mutex::new(Some(Boundary {
            cut,
            reached,
            resume: blocked,
            document_witness,
            close_witness,
        })),
        read,
    );
    let writer = async {
        wait.await?;
        let written = write.await?;
        resume
            .send(())
            .map_err(|()| "the paused reader ended before its writer committed")?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(written)
    };
    let (read, written) =
        tokio::time::timeout(PATIENCE, async { tokio::join!(reader, writer) }).await?;
    Ok((read, written?))
}

async fn running() -> (Scene, Id, TurnStart) {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .say(
            &scene.ada,
            thread,
            "Show the snapshot boundary",
            Delivery::Queue,
            "read-cut-input",
        )
        .await
        .ok();
    let start = scene.rig.turn_start(thread, 0).await;
    scene
        .rig
        .answer(
            thread,
            &start,
            "read-cut-answer",
            "Before the concurrent commit",
        )
        .await;
    scene.rig.wait_turn(start.turn_id, "running").await;
    scene.rig.wait_state(thread, "running").await;
    // Publication and Running are earlier than the last message's committed delivery and
    // the turn-start command's receipt. None of that setup belongs to the tested interval.
    rig::eventually("the initial native answer and input receipt are committed", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (
            SELECT 1 FROM items i JOIN model_inputs m ON m.node_id=i.thread_id AND m.turn_id=i.turn_id
            WHERE i.thread_id=$1 AND i.turn_id=$2 AND i.native_ref='read-cut-answer'
              AND i.kind='agent_message' AND i.status='completed'
              AND i.payload->>'text'='Before the concurrent commit'
              AND m.purpose='input' AND m.state='sent'
              AND NOT EXISTS (SELECT 1 FROM model_inputs pending
                  WHERE pending.node_id=$1 AND pending.state='prepared'))")
            .bind(thread).bind(start.turn_id).fetch_one(scene.rig.db.pool()).await
            .unwrap_or_else(|error| panic!("could not read the native setup's committed state: {error}"))
    }).await;
    (scene, thread, start)
}

async fn sync(scene: &Scene, thread: Id, since: Option<Seq>) -> ThreadSync {
    scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery {
            since,
            ..Default::default()
        })
        .send()
        .await
        .ok()
}

async fn complete(scene: &Scene, thread: Id, start: &TurnStart) -> Proof<Seq> {
    scene
        .rig
        .say(
            thread,
            rig::turn_ended(start.turn_id, TurnOutcome::Completed),
        )
        .await;
    scene.rig.wait_turn(start.turn_id, "completed").await;
    scene.rig.wait_state(thread, "idle").await;
    let mut conn = scene.rig.world.conn().await;
    orch_db::sequences::current(&mut conn, thread)
        .await?
        .ok_or_else(|| "thread disappeared".into())
}

fn bounded(sync: &ThreadSync) {
    assert_eq!(sync.seq, sync.thread.seq);
    assert!(sync.items.iter().all(|row| row.seq_updated <= sync.seq));
    assert!(sync.turns.iter().all(|row| row.seq_updated <= sync.seq));
    assert!(sync.inputs.iter().all(|row| row.seq_updated <= sync.seq));
    assert!(sync.segments.iter().all(|row| row.seq_updated <= sync.seq));
}

fn socket_inner(scene: &Scene) -> Arc<Inner> {
    let app = &scene.rig.world.app.app;
    app.realtime.attach_app(app);
    // Projection tests need the real engine, but no fan-out timers or server loop.
    assert!(
        app.realtime
            .inner
            .engine
            .set(scene.rig.engine.clone())
            .is_ok()
    );
    Arc::clone(&app.realtime.inner)
}

/// The direct row controls use the same real session login and admission as delivery.
/// Keep the original reader identity, selector, cursor/cache and source producer intact.
async fn authenticated_socket_rows(
    scene: &Scene,
    inner: &Arc<Inner>,
    reader: (Id, &str),
    thread: Id,
    after: Seq,
    upto: Seq,
    cache: &mut Cache,
) -> Result<Rows, super::Fail> {
    let connection =
        super::server::epoch_test_connection(&scene.rig.world.app.app, inner, reader.1)
            .await
            .map_err(|(code, _)| {
                super::Fail::other(format!(
                    "the actual reader session could not log in: {code}"
                ))
            })?;
    if connection.user != reader.0 {
        return Err(super::Fail::other(
            "the row fixture changed its actual reader",
        ));
    }
    let caller = super::key_reads::admit(
        inner,
        &connection,
        &orch_realtime_contract::feeds::Feed::Thread(thread),
    )
    .await?
    .ok_or_else(|| super::Fail::other("the actual reader did not admit this Thread feed"))?;
    super::subscriptions::viewing_admitted(
        &connection,
        caller,
        threads::read(inner, reader.0, thread, after, upto, cache),
    )
    .await?
}

async fn history_scope_shrink_at_snapshot_tail<E>(endpoint: E) -> Proof
where
    E: orch_api_contract::routes::Endpoint<
            Query = orch_api_contract::threads::ThreadHistoryListQuery,
            Body = (),
        > + Copy,
{
    use orch_api_contract::threads::ThreadHistoryListQuery;
    use thread_history_scope::{denied, key, narrow, ok, request, rule};
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let world = &scene.rig.world;
    let operation = format!("{}.read", E::ROUTE.name);
    let selected = key(
        world,
        scene.ada.id(),
        vec![rule(&operation, "thread", thread, "self", "read")],
    )
    .await?;
    let query = ThreadHistoryListQuery::default();
    // Refuse an unbound baseline immediately, before installing a barrier it cannot reach.
    ok(&request(world, endpoint, thread, &selected.secret, &query).await)?;
    let (reply, ()) = interleave(
        Cut::HttpHistoryRows,
        request(world, endpoint, thread, &selected.secret, &query),
        narrow(
            world,
            &selected,
            vec![rule(
                "ThreadOutlineGet.read",
                "thread",
                thread,
                "self",
                "read",
            )],
        ),
    )
    .await?;
    denied(&reply);
    Ok(())
}

#[tokio::test]
async fn scoped_history_segment_shrink_after_snapshot_refuses_stale_content() -> Proof {
    history_scope_shrink_at_snapshot_tail(orch_api_contract::threads::ThreadSegmentList).await
}

#[tokio::test]
async fn scoped_history_turn_shrink_after_snapshot_refuses_stale_content() -> Proof {
    history_scope_shrink_at_snapshot_tail(orch_api_contract::threads::ThreadTurnList).await
}

fn frame(rows: Rows) -> Proof<Box<Frame>> {
    // Observe the actual producer envelope without logging content or credential identities.
    let kind = match &rows {
        Rows::Revoked => "revoked",
        Rows::ProjectionChanged => "projection_changed",
        Rows::Nothing => "nothing",
        Rows::TooMany => "too_many",
        Rows::Gone => "gone",
        Rows::Frame(_) => "frame",
    };
    eprintln!(
        "history socket diagnostic rows_kind={kind} rows_results=1 frame_count={}",
        usize::from(matches!(&rows, Rows::Frame(_)))
    );
    if let Rows::Frame(frame) = &rows
        && let SyncData::Thread(data) = &frame.data
    {
        eprintln!(
            "history socket diagnostic highest={} items={} turns={} inputs={} segments={} projection_revision={:?} execution_revision={:?}",
            frame.highest,
            data.items.as_ref().map_or(0, Vec::len),
            data.turns.as_ref().map_or(0, Vec::len),
            data.inputs.as_ref().map_or(0, Vec::len),
            data.segments.as_ref().map_or(0, Vec::len),
            data.projection
                .as_ref()
                .map(|projection| projection.projection_revision),
            data.projection
                .as_ref()
                .map(|projection| projection.execution_revision),
        );
    }
    match rows {
        Rows::Frame(frame) => Ok(frame),
        _ => Err("the native thread change did not produce a frame".into()),
    }
}

fn data(frame: &Frame) -> Proof<&ThreadData> {
    match &frame.data {
        SyncData::Thread(data) => {
            assert!(
                data.items
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            assert!(
                data.turns
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            assert!(
                data.inputs
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            assert!(
                data.segments
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            if let Some(thread) = &data.thread {
                assert_eq!(thread.seq, frame.highest);
            }
            Ok(data)
        }
        _ => Err("a different feed was returned for a thread".into()),
    }
}

#[tokio::test]
async fn http_sync_head_count_and_rows_stay_on_one_cut_across_a_terminal_commit() -> Proof {
    for since in [None, Some(0)] {
        let (scene, thread, start) = running().await;
        let (reply, terminal) = interleave(
            Cut::HttpHead,
            sync(&scene, thread, since),
            complete(&scene, thread, &start),
        )
        .await?;
        bounded(&reply);
        assert!(reply.complete);
        assert!(reply.seq < terminal);
        assert_eq!(reply.thread.state, ThreadState::Running);
        assert_eq!(reply.thread.current_turn_id, Some(start.turn_id));
        assert_eq!(
            reply
                .turns
                .iter()
                .find(|turn| turn.id == start.turn_id)
                .ok_or("running turn absent")?
                .state,
            TurnState::Running
        );
        let after = sync(&scene, thread, Some(reply.seq)).await;
        bounded(&after);
        assert!(after.seq >= terminal);
        assert_eq!(after.thread.state, ThreadState::Idle);
        assert_eq!(after.thread.current_turn_id, None);
        assert_eq!(
            after
                .turns
                .iter()
                .find(|turn| turn.id == start.turn_id)
                .ok_or("terminal turn absent")?
                .state,
            TurnState::Completed
        );
    }
    Ok(())
}

#[tokio::test]
async fn socket_rows_and_personalized_thread_do_not_mix_two_commits() -> Proof {
    let (scene, thread, start) = running().await;
    let inner = socket_inner(&scene);
    let wake = sync(&scene, thread, None).await.seq;
    let mut cache = Cache::default();
    let (rows, terminal) = interleave(
        Cut::SocketRows,
        authenticated_socket_rows(
            &scene,
            &inner,
            (scene.ada.id(), &scene.ada.token),
            thread,
            0,
            wake,
            &mut cache,
        ),
        complete(&scene, thread, &start),
    )
    .await?;
    let old = frame(rows?)?;
    let old_data = data(&old)?;
    assert!(old.highest < terminal);
    let old_thread = old_data
        .thread
        .as_ref()
        .ok_or("personalized thread absent")?;
    assert_eq!(old_thread.state, ThreadState::Running);
    assert_eq!(old_thread.current_turn_id, Some(start.turn_id));
    assert_eq!(
        old_data
            .turns
            .iter()
            .flatten()
            .find(|turn| turn.id == start.turn_id)
            .ok_or("running turn absent")?
            .state,
        TurnState::Running
    );
    let next = frame(
        authenticated_socket_rows(
            &scene,
            &inner,
            (scene.ada.id(), &scene.ada.token),
            thread,
            old.highest,
            terminal,
            &mut cache,
        )
        .await?,
    )?;
    let next_data = data(&next)?;
    let next_thread = next_data
        .thread
        .as_ref()
        .ok_or("terminal personalized thread absent")?;
    assert_eq!(next_thread.state, ThreadState::Idle);
    assert_eq!(next_thread.current_turn_id, None);
    assert_eq!(
        next_data
            .turns
            .iter()
            .flatten()
            .find(|turn| turn.id == start.turn_id)
            .ok_or("terminal turn absent")?
            .state,
        TurnState::Completed
    );
    Ok(())
}

#[tokio::test]
async fn a_late_wake_carries_every_changed_row_through_the_actual_snapshot_head() -> Proof {
    let (scene, thread, start) = running().await;
    let inner = socket_inner(&scene);
    let wake = sync(&scene, thread, None).await.seq;
    complete(&scene, thread, &start).await?;
    // One controlled transaction changes all four row families after the old notification.
    // These are persisted synthetic reports; no provider or machine call is held inside it.
    let mut tx = scene.rig.db.begin().await?;
    let segment: Id = sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&mut *tx)
        .await?;
    let item: Id = sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind='agent_message' ORDER BY seq_created DESC LIMIT 1")
        .bind(thread).fetch_one(&mut *tx).await?;
    let input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(start.turn_id)
        .fetch_one(&mut *tx)
        .await?;
    let item_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE items SET payload=payload || '{\"text\":\"After the old wake\",\"text_len\":18}'::jsonb,seq_updated=$2 WHERE id=$1")
        .bind(item).bind(item_seq).execute(&mut *tx).await?;
    let turn_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE turns SET seq_updated=$2 WHERE id=$1")
        .bind(start.turn_id)
        .bind(turn_seq)
        .execute(&mut *tx)
        .await?;
    let input_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE thread_inputs SET seq_updated=$2 WHERE id=$1")
        .bind(input)
        .bind(input_seq)
        .execute(&mut *tx)
        .await?;
    let segment_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE segments SET seq_updated=$2 WHERE id=$1")
        .bind(segment)
        .bind(segment_seq)
        .execute(&mut *tx)
        .await?;
    let cut = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    orch_db::queries::threads::set_seq_state(&mut tx, thread, cut).await?;
    tx.commit().await?;
    let mut cache = Cache::default();
    let read = frame(
        authenticated_socket_rows(
            &scene,
            &inner,
            (scene.ada.id(), &scene.ada.token),
            thread,
            wake - 1,
            wake,
            &mut cache,
        )
        .await?,
    )?;
    assert!(read.highest >= cut);
    let read_data = data(&read)?;
    assert!(
        read_data
            .items
            .iter()
            .flatten()
            .any(|row| row.id == item && row.seq_updated == item_seq)
    );
    assert!(
        read_data
            .turns
            .iter()
            .flatten()
            .any(|row| row.id == start.turn_id
                && row.seq_updated >= turn_seq
                && row.state == TurnState::Completed)
    );
    assert!(
        read_data
            .inputs
            .iter()
            .flatten()
            .any(|row| row.id == input && row.seq_updated == input_seq)
    );
    assert!(
        read_data
            .segments
            .iter()
            .flatten()
            .any(|row| row.id == segment && row.seq_updated == segment_seq)
    );
    assert_eq!(
        read_data
            .thread
            .as_ref()
            .ok_or("personalized thread absent")?
            .state,
        ThreadState::Idle
    );
    Ok(())
}

#[tokio::test]
async fn http_incremental_budget_and_rows_use_the_same_snapshot_as_its_head() -> Proof {
    let (scene, thread, _) = running().await;
    let before = sync(&scene, thread, None).await;
    let write = async {
        let mut tx = scene.rig.db.begin().await?;
        let segment = before
            .thread
            .current_segment_id
            .ok_or("current segment absent")?;
        // One more than the existing incremental budget, with a real unique creation stamp per row.
        let ids: Vec<_> = (0..=orch_api_contract::threads::THREAD_SYNC_MAX_CHANGES)
            .map(|_| orch_types::new_id())
            .collect();
        let count = i64::try_from(ids.len())?;
        let first: Seq = sqlx::query_scalar(
            "UPDATE threads SET next_seq=next_seq+$2 WHERE node_id=$1 RETURNING next_seq-$2",
        )
        .bind(thread)
        .bind(count)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO items(id,thread_id,segment_id,stream,kind,status,payload,seq_created,seq_updated,created_at,updated_at)
            SELECT id,$1,$2,'main','agent_message','completed','{\"text\":\"Count boundary\",\"text_len\":14}'::jsonb,
                $3+ordinal-1,$3+ordinal-1,$5,$5 FROM unnest($4::uuid[]) WITH ORDINALITY AS added(id,ordinal)")
            .bind(thread).bind(segment).bind(first).bind(&ids).bind(scene.rig.world.now())
            .execute(&mut *tx).await?;
        let last = first + count - 1;
        orch_db::queries::threads::set_seq_state(&mut tx, thread, last).await?;
        tx.commit().await?;
        Ok(last)
    };
    let (old, last) =
        interleave(Cut::HttpHead, sync(&scene, thread, Some(before.seq)), write).await?;
    bounded(&old);
    assert_eq!(old.since, Some(before.seq));
    assert!(old.seq >= before.seq);
    assert!(
        old.complete,
        "a later large commit cannot change the earlier count gate"
    );
    assert!(old.items.is_empty());
    // An incremental page also carries the exact turn/segment references needed
    // to interpret its current thread and rows. Those references may predate since.
    let referenced_turns: std::collections::BTreeSet<_> = old
        .items
        .iter()
        .filter_map(|row| row.turn_id)
        .chain(old.inputs.iter().filter_map(|row| row.turn_id))
        .chain(old.thread.current_turn_id)
        .collect();
    assert!(
        old.turns
            .iter()
            .all(|row| row.seq_updated > before.seq || referenced_turns.contains(&row.id))
    );
    assert!(old.inputs.iter().all(|row| row.seq_updated > before.seq));
    let referenced_segments: std::collections::BTreeSet<_> = old
        .items
        .iter()
        .map(|row| row.segment_id)
        .chain(old.turns.iter().map(|row| row.segment_id))
        .chain(old.thread.current_segment_id)
        .collect();
    assert!(
        old.segments
            .iter()
            .all(|row| row.seq_updated > before.seq || referenced_segments.contains(&row.id))
    );
    assert!(old.seq < last);
    let next = sync(&scene, thread, Some(old.seq)).await;
    bounded(&next);
    assert_eq!(next.since, Some(old.seq));
    assert!(next.seq >= last);
    assert!(
        !next.complete,
        "the next coherent cut explicitly asks for a reload"
    );
    assert!(next.items.is_empty());
    Ok(())
}

#[tokio::test]
async fn opening_outline_and_conversation_keep_the_same_cut_while_the_native_turn_ends() -> Proof {
    let (scene, thread, start) = running().await;
    let read = async {
        scene
            .rig
            .world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&scene.ada.token)
            .query(&ThreadOpeningGetQuery {
                limit: Some(5),
                ..Default::default()
            })
            .send()
            .await
            .ok()
    };
    let (opening, terminal) =
        interleave(Cut::HttpHead, read, complete(&scene, thread, &start)).await?;
    bounded(&opening.sync);
    assert_eq!(opening.outline.seq, opening.sync.seq);
    assert!(
        opening
            .outline
            .entries
            .iter()
            .all(|entry| entry.seq <= opening.sync.seq)
    );
    assert!(opening.sync.seq < terminal);
    assert_eq!(opening.sync.thread.current_turn_id, Some(start.turn_id));
    Ok(())
}

#[tokio::test]
async fn socket_request_cards_share_the_row_cut_and_never_share_another_readers_authority() -> Proof
{
    let (scene, thread, start) = running().await;
    scene
        .rig
        .say(
            thread,
            rig::command_asked(start.turn_id, "read-cut-approval"),
        )
        .await;
    scene.rig.wait_state(thread, "waiting_approval").await;
    let waiting = sync(&scene, thread, None).await;
    let approval = waiting
        .items
        .iter()
        .find_map(|item| match &item.payload {
            ItemPayload::Approval(payload) => Some(payload.approval_id),
            _ => None,
        })
        .ok_or("approval absent")?;
    let inner = socket_inner(&scene);
    let mut cache = Cache::default();
    let card = |rows: &ThreadData| {
        rows.items
            .iter()
            .flatten()
            .find_map(|item| match &item.payload {
                ItemPayload::Approval(payload) if payload.approval_id == approval => {
                    payload.approval.as_deref()
                }
                _ => None,
            })
            .cloned()
    };
    let viewer = frame(
        authenticated_socket_rows(
            &scene,
            &inner,
            (scene.cai.id(), &scene.cai.token),
            thread,
            0,
            waiting.seq,
            &mut cache,
        )
        .await?,
    )?;
    assert!(!card(data(&viewer)?).ok_or("viewer card absent")?.can_decide);
    let decide = async {
        let answer = scene
            .rig
            .world
            .app
            .call(ApprovalDecide, &[&approval])
            .token(&scene.ada.token)
            .body(&ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "allow_once".into(),
                note: None,
            }))
            .send()
            .await
            .full_approval()
            .ok();
        assert_eq!(answer.state, ApprovalState::Approved);
        Ok(answer)
    };
    let (rows, _) = interleave(
        Cut::SocketRows,
        authenticated_socket_rows(
            &scene,
            &inner,
            (scene.ada.id(), &scene.ada.token),
            thread,
            0,
            waiting.seq,
            &mut cache,
        ),
        decide,
    )
    .await?;
    let old = frame(rows?)?;
    let old_data = data(&old)?;
    let old_card = card(old_data).ok_or("owner card absent")?;
    assert_eq!(old_card.state, ApprovalState::Pending);
    assert!(old_card.can_decide);
    assert_eq!(
        old_data
            .thread
            .as_ref()
            .ok_or("waiting thread absent")?
            .state,
        ThreadState::WaitingApproval
    );
    let after = sync(&scene, thread, Some(old.highest)).await;
    let next = frame(
        authenticated_socket_rows(
            &scene,
            &inner,
            (scene.ada.id(), &scene.ada.token),
            thread,
            old.highest,
            after.seq,
            &mut cache,
        )
        .await?,
    )?;
    assert_eq!(
        card(data(&next)?).ok_or("decided card absent")?.state,
        ApprovalState::Approved
    );
    Ok(())
}

/// Actual raw content is opened before the same key changes, then refused before it leaves.
async fn raw_thread_content_rechecks_same_key_before_bytes(output: bool) -> Proof {
    use orch_api_contract::routes::Endpoint as _;
    use orch_api_contract::threads::{ThreadBlobGet, ThreadItemOutputGet, UploadCreate};
    use orch_db::queries::threads::{self as rows, NewItem};
    use thread_history_scope::{denied, key, narrow, rule};
    const BODY: &[u8] = b"Raw thread bytes withheld after this key changes";
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let world = &scene.rig.world;
    let hash = orch_types::Hash::of(BODY);
    if !output {
        let stored = world
            .app
            .call(UploadCreate, &[])
            .token(&scene.ada.token)
            .header("content-type", "image/png")
            .bytes(BODY)
            .send()
            .await
            .ok();
        assert_eq!(stored.blob, hash);
    }
    let payload = if output {
        serde_json::json!({"tool":{"kind":"command","output":{"inline":String::from_utf8_lossy(BODY),"total_len":BODY.len(),"truncated":false,"blob":null}}})
    } else {
        serde_json::json!({"tool":{"kind":"image","blob":hash.to_string(),"media_type":"image/png","name":"same-key-image.png"}})
    };
    let item = orch_types::new_id();
    {
        let mut tx = scene.rig.db.begin().await?;
        let segment = rows::thread(&mut tx, thread)
            .await?
            .ok_or("Thread absent")?
            .current_segment_id
            .ok_or("bound segment absent")?;
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("Thread absent")?;
        rows::insert_item(
            &mut tx,
            &NewItem {
                id: item,
                thread_id: thread,
                segment_id: segment,
                turn_id: None,
                stream: "main",
                parent_item_id: None,
                kind: "tool_call",
                status: "completed",
                author_id: None,
                payload: &payload,
                native_ref: None,
                search_text: None,
                at: world.now(),
            },
            seq,
        )
        .await?
        .ok_or("stored raw source absent")?;
        tx.commit().await?;
    }
    let name = if output {
        ThreadItemOutputGet::ROUTE.name
    } else {
        ThreadBlobGet::ROUTE.name
    };
    let selected = key(
        world,
        scene.ada.id(),
        vec![rule(
            &format!("{name}.read"),
            "thread",
            thread,
            "self",
            "read",
        )],
    )
    .await?;
    let read = || async {
        if output {
            world
                .app
                .call(ThreadItemOutputGet, &[&thread, &item])
                .token(&selected.secret)
                .send_bytes()
                .await
        } else {
            world
                .app
                .call(ThreadBlobGet, &[&thread, &hash])
                .token(&selected.secret)
                .send_bytes()
                .await
        }
    };
    let ordinary = read().await;
    assert_eq!(
        ordinary.status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&ordinary.raw)
    );
    assert_eq!(ordinary.raw.as_ref(), BODY, "real raw body premise");
    let (reply, ()) = interleave(
        Cut::RawContentReturn,
        read(),
        narrow(
            world,
            &selected,
            vec![rule("ThreadRawList", "thread", thread, "self", "manage")],
        ),
    )
    .await?;
    denied(&reply);
    assert!(!reply.raw.windows(BODY.len()).any(|bytes| bytes == BODY));
    for header in [
        orch_api_contract::threads::TOTAL_LENGTH_HEADER,
        "etag",
        "content-disposition",
        "content-range",
    ] {
        assert!(
            !reply.headers.contains_key(header),
            "old raw metadata escaped in {header}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn scoped_raw_thread_output_rechecks_same_key_after_open_before_bytes() -> Proof {
    raw_thread_content_rechecks_same_key_before_bytes(true).await
}

#[tokio::test]
async fn scoped_raw_thread_blob_rechecks_same_key_after_open_before_bytes() -> Proof {
    raw_thread_content_rechecks_same_key_before_bytes(false).await
}

struct CloseWitness {
    operation: String,
    threads: Vec<orch_access_contract::key_scope::KeyResolvedTarget>,
}
/// Captures actual admitted source targets, not a guessed path or a cached DTO.
pub(crate) async fn close_name_sources_cut(
    plan: &crate::registry::OperationPlan,
) -> crate::middleware::requests::ApiResult<()> {
    if let Some(boundary) = take_boundary(Cut::CloseNameSources) {
        let facts = plan
            .shared()
            .response_facets()
            .map_err(crate::key_operations::admission::binding_error)?;
        let witness = CloseWitness {
            operation: plan.variant().operation.clone(),
            threads: facts
                .into_iter()
                .filter(|facet| {
                    facet.resource_kind
                        == orch_access_contract::key_scope::KeyTargetKind::Object(
                            orch_access_contract::permissions::ObjectKind::Thread,
                        )
                })
                .map(|facet| facet.target)
                .collect(),
        };
        if let Some(sender) = boundary.close_witness {
            let _ = sender.send(witness);
        }
        let _ = boundary.reached.send(());
        let _ = boundary.resume.await;
    }
    Ok(())
}

async fn close_names_keep_one_scope_and_name_cut(surface: close_fixture::Surface) -> Proof {
    use orch_api_contract::thread_move::{
        ThreadMove, ThreadMoveBody, ThreadMovePreview, ThreadMovePreviewBody,
    };
    use thread_history_scope::{key, rule};
    const OLD: &str = "Thread name originally licensed in P";
    const NEW: &str = "PRIVATE Q NAME MUST NEVER CROSS OLD P SCOPE";
    let scene = close_fixture::scene(surface).await?;
    let thread = scene.thread_by(&scene.ada, OLD).await.base.id;
    let destination = crate::places::make_topic(
        &scene.rig.world,
        &scene.ada,
        scene.tree.project.base.id,
        "Q outside P",
    )
    .await;
    let ordinary_id = surface.make_for(&scene, Some(thread)).await?;
    let ordinary =
        close_fixture::body(&surface.close(&scene, ordinary_id, &scene.cai.token).await)?;
    assert_eq!(
        ordinary["thread"]["name"], OLD,
        "ordinary same-reader name premise"
    );
    let selected = key(
        &scene.rig.world,
        scene.cai.id(),
        vec![
            rule(
                surface.operation(),
                "environment",
                scene.on.environment,
                "self",
                "manage",
            ),
            rule(
                surface.operation(),
                "environment",
                scene.on.environment,
                "self",
                "read",
            ),
            rule(
                surface.operation(),
                "topic",
                scene.tree.topic.base.id,
                "subtree",
                "read",
            ),
        ],
    )
    .await?;
    let preflight = surface.make_for(&scene, Some(thread)).await?;
    let pre = close_fixture::body(&surface.close(&scene, preflight, &selected.secret).await)?;
    assert_eq!(
        pre["thread"]["name"], OLD,
        "actual key can read original P source before barrier"
    );
    let id = surface.make_for(&scene, Some(thread)).await?;
    let (send, held) = oneshot::channel();
    let write = async {
        let witness: CloseWitness = held.await?;
        assert_eq!(witness.operation, surface.operation());
        assert_eq!(
            witness.threads.len(),
            1,
            "real optional source target must be bound before move"
        );
        assert_eq!(witness.threads[0].object.id, Some(thread));
        assert!(
            witness.threads[0]
                .node_ancestors
                .iter()
                .any(|parent| parent.id == Some(scene.tree.topic.base.id)),
            "actual source binding witnessed P before projection"
        );
        let plan = scene
            .rig
            .world
            .app
            .call(ThreadMovePreview, &[&thread])
            .token(&scene.ada.token)
            .body(&ThreadMovePreviewBody {
                parent_id: destination.base.id,
            })
            .send()
            .await
            .ok()
            .into_full()
            .expect("the mover's own full preview");
        assert!(plan.movement.blockers.is_empty());
        let moved = scene
            .rig
            .world
            .app
            .call(ThreadMove, &[&thread])
            .token(&scene.ada.token)
            .body(&ThreadMoveBody {
                parent_id: destination.base.id,
                expected_revision: plan.expected_revision,
                preflight_id: plan.movement.preflight_id,
            })
            .send_bytes()
            .await;
        assert_eq!(
            moved.status.as_u16(),
            200,
            "authorized real ThreadMove: {}",
            String::from_utf8_lossy(&moved.raw)
        );
        let current = crate::places::read(&scene.rig.world, &scene.ada, thread).await;
        let orch_api_contract::nodes::Node::Thread(current) = current else {
            panic!("the moved source remains a real Thread");
        };
        let renamed = scene
            .rig
            .world
            .app
            .call(orch_api_contract::nodes::NodeUpdate, &[&thread])
            .token(&scene.ada.token)
            .raw_body(&serde_json::json!({"name":NEW,"expected_revision":current.base.revision}))
            .send_bytes()
            .await;
        assert_eq!(
            renamed.status.as_u16(),
            200,
            "authorized real rename: {}",
            String::from_utf8_lossy(&renamed.raw)
        );
        let actual: (Id, String) = sqlx::query_as("SELECT parent_id,name FROM nodes WHERE id=$1")
            .bind(thread)
            .fetch_one(&scene.rig.world.db.pool)
            .await?;
        assert_eq!(actual, (destination.base.id, NEW.to_owned()));
        Ok(())
    };
    let (reply, ()) = interleave_with_sources(
        Cut::CloseNameSources,
        surface.close(&scene, id, &selected.secret),
        write,
        None,
        Some(send),
    )
    .await?;
    let value = close_fixture::body(&reply)?;
    assert!(
        value.get("projection").is_none(),
        "Env Full remains admitted independently"
    );
    assert_eq!(
        value["environment"]["id"],
        serde_json::json!(scene.on.environment)
    );
    assert_eq!(value["state"], "closed");
    assert!(
        !String::from_utf8_lossy(&reply.raw).contains(NEW),
        "new Q content escaped through source facts previously validated only in P"
    );
    assert!(
        value["thread"].is_null() || value["thread"]["name"] == OLD,
        "one coherent licensed source cut or canonical optional omission"
    );
    let fresh = close_fixture::body(&surface.close(&scene, id, &selected.secret).await)?;
    assert!(fresh.get("projection").is_none());
    assert!(
        fresh["thread"].is_null(),
        "fresh current source in Q is outside this key's P read bound"
    );
    Ok(())
}
#[tokio::test]
async fn browser_close_name_source_and_projection_never_mix_p_with_q() -> Proof {
    close_names_keep_one_scope_and_name_cut(close_fixture::Surface::Browser).await
}
#[tokio::test]
async fn display_close_name_source_and_projection_never_mix_p_with_q() -> Proof {
    close_names_keep_one_scope_and_name_cut(close_fixture::Surface::Display).await
}
