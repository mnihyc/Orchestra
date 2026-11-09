//! Real operation-scoped keys preserve pure gifts, explicit control and read-only sharing previews.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod rig;
mod support;

#[path = "share_operation_scopes/creation_replays.rs"]
mod creation_replays;

use async_trait::async_trait;
use orch_api_contract::access::{
    EffectiveAccessGet, EffectiveAccessGetQuery, SharePreviewBody, SharePreviewCreate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{Document, NewDocumentAccess, NodeDocumentCreate};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::share::{
    ShareAgentCreate, ShareAgentCreateReply, ShareAgentCreateResponse, ShareDocumentsLevel,
    ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatDocument, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_api_core::testing::Reply;
use orch_db::Conn;
use orch_db::queries::shares::CopyRow;
use orch_engine::copies::Files;
use orch_engine::sharing::{FileWritten, ParcelFault, Parcels};
use orch_types::{Id, Json, new_id};
use places::Who;
use rig::Scene;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &support::World, owner: Id, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let scope = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = world.now();
    let mut conn = world.conn().await;
    create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id: new_id(),
            user_id: owner,
            name: "Share operation control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            // Fixed-clock controls need only an ordinary short lifetime.
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}

async fn document(s: &Scene, owner: &Who, title: &str, text: &str) -> Document {
    let mut body = desk::body(title);
    body.access = Some(NewDocumentAccess::Private);
    body.blocks = Some(vec![desk::paragraph(text)]);
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&owner.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
}

fn request(source: Id, recipients: &[Id], mode: ShareMode) -> ShareWithAgents {
    ShareWithAgents {
        link_origin: None,
        viewers_read: None,
        read_first: None,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: source,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(if mode == ShareMode::Live {
                ShareDocumentsLevel::Manage
            } else {
                ShareDocumentsLevel::Read
            }),
            ..ShareParts::default()
        }),
        mode,
        to: ShareTo::Threads(
            recipients
                .iter()
                .map(|id| ShareToThread { thread_id: *id })
                .collect(),
        ),
        comment: None,
        expires_at: None,
        client_id: new_id().to_string(),
    }
}

async fn send(s: &Scene, token: &str, body: &ShareWithAgents) -> Reply<ShareAgentCreateResponse> {
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(token)
        .body(body)
        .send()
        .await
}

fn receipt(reply: Reply<ShareAgentCreateResponse>, body: &ShareWithAgents, status: u16) -> Vec<Id> {
    assert_eq!(reply.status.as_u16(), status, "{:?}", reply.body);
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    let ShareAgentCreateResponse::Receipt(receipt) = reply.ok() else {
        panic!("this operation cannot read every full response target");
    };
    let expected = json!({"projection":"receipt", "shares":receipt.shares.iter().map(|row|json!({
        "id":row.id, "to":{"kind":row.to.kind,"id":row.to.id},
        "mode":row.mode, "created":row.created, "input_id":row.input_id
    })).collect::<Vec<_>>()});
    assert_eq!(
        raw, expected,
        "no source payload, current entries, names or counts enter a receipt"
    );
    let ShareTo::Threads(recipients) = &body.to else {
        panic!("individual test recipients");
    };
    assert_eq!(receipt.shares.len(), recipients.len());
    for row in &receipt.shares {
        assert!(!row.id.is_nil());
        assert_eq!(row.mode, body.mode);
        assert_eq!(row.to.kind, NodeKind::Thread);
        assert!(
            recipients
                .iter()
                .any(|recipient| recipient.thread_id == row.to.id)
        );
        assert_eq!(row.created, status == 201);
        assert!(row.input_id.is_none(), "these controls submit no comment");
    }
    receipt.shares.into_iter().map(|row| row.id).collect()
}

fn full(reply: Reply<ShareAgentCreateResponse>, status: u16) -> ShareAgentCreateReply {
    assert_eq!(reply.status.as_u16(), status, "{:?}", reply.body);
    reply
        .ok()
        .into_full()
        .expect("the operation's full projection is permitted")
}

#[derive(Debug, PartialEq)]
struct Execution {
    state: String,
    current_turn: Option<Id>,
    notices: Json,
    inputs: Vec<Id>,
    turns: Vec<Id>,
    copies: Vec<Id>,
    commands: Vec<Json>,
}

fn names_thread(value: &Json, thread: &str) -> bool {
    match value {
        Json::Object(fields) => fields.iter().any(|(name, value)| {
            (name == "thread_id" && value.as_str() == Some(thread)) || names_thread(value, thread)
        }),
        Json::Array(items) => items.iter().any(|item| names_thread(item, thread)),
        _ => false,
    }
}

async fn execution(s: &Scene, thread: Id) -> Execution {
    let mut conn = s.rig.world.conn().await;
    let row = orch_db::queries::threads::thread(&mut conn, thread)
        .await
        .unwrap()
        .unwrap();
    let inputs = sqlx::query_scalar("SELECT id FROM thread_inputs WHERE thread_id=$1 ORDER BY id")
        .bind(thread)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let turns = sqlx::query_scalar("SELECT id FROM turns WHERE thread_id=$1 ORDER BY id")
        .bind(thread)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let copies = sqlx::query_scalar("SELECT id FROM agent_shares WHERE thread_id=$1 ORDER BY id")
        .bind(thread)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let commands = s
        .rig
        .seen
        .all()
        .iter()
        .map(|command| serde_json::to_value(command).unwrap())
        .filter(|command| names_thread(command, &thread.to_string()))
        .collect();
    Execution {
        state: row.state,
        current_turn: row.current_turn_id,
        notices: row.pending_notices,
        inputs,
        turns,
        copies,
        commands,
    }
}

async fn grants(s: &Scene, source: Id) -> Value {
    sqlx::query_scalar("SELECT COALESCE(jsonb_agg(jsonb_build_array(id,holder_kind,holder_id,permission,created_by,revision,removed_at) ORDER BY id),'[]'::jsonb) FROM permission_entries WHERE object_kind='document' AND object_id=$1")
        .bind(source).fetch_one(&s.rig.world.db.pool).await.unwrap()
}

async fn history(s: &Scene, thread: Id) -> Vec<Id> {
    sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 ORDER BY id")
        .bind(thread)
        .fetch_all(&s.rig.world.db.pool)
        .await
        .unwrap()
}

async fn is_viewer_only(s: &Scene, thread: Id, who: &Who) {
    let held = s
        .rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &thread])
        .token(&s.ada.token)
        .query(&EffectiveAccessGetQuery { user_id: who.id() })
        .send()
        .await
        .ok();
    assert!(
        held.permissions
            .iter()
            .any(|row| row.permission == "thread.view")
    );
    assert!(
        !held
            .permissions
            .iter()
            .any(|row| row.permission == "thread.interact")
    );
}

/// Retains real files in memory through the supported out-of-band parcel implementation.
/// Preview/pure-gift controls require all these hooks to remain unused.
#[derive(Default)]
struct ParcelCalls {
    kept: Mutex<Vec<(Id, String, Files)>>,
    withdrawn: Mutex<Vec<Id>>,
    writes: Mutex<Vec<(Id, String, Vec<u8>)>>,
}
#[async_trait]
impl Parcels for ParcelCalls {
    async fn keep(
        &self,
        _: &mut Conn,
        copy: Id,
        path: &str,
        files: &Files,
    ) -> Result<Option<Vec<u8>>, ParcelFault> {
        self.kept
            .lock()
            .unwrap()
            .push((copy, path.into(), files.clone()));
        Ok(None)
    }
    async fn withdraw(&self, _: &mut Conn, copy: &CopyRow) -> Result<(), ParcelFault> {
        self.withdrawn.lock().unwrap().push(copy.id);
        Ok(())
    }
    async fn withdraw_before(
        &self,
        _conn: &mut Conn,
        copy: &CopyRow,
        before: orch_engine::sharing::BeforeParcel<'_>,
    ) -> Result<(), orch_engine::sharing::ParcelFailure> {
        let mut withdrawn = self.withdrawn.lock().unwrap();
        before().map_err(orch_authz::pages::EffectFailure::BeforeEffect)?;
        withdrawn.push(copy.id);
        Ok(())
    }

    async fn write_file(
        &self,
        _: &mut Conn,
        thread: Id,
        path: &str,
        bytes: &[u8],
    ) -> Result<FileWritten, ParcelFault> {
        self.writes
            .lock()
            .unwrap()
            .push((thread, path.into(), bytes.to_vec()));
        Ok(FileWritten::Ready)
    }
}
impl ParcelCalls {
    fn empty(&self) -> bool {
        self.kept.lock().unwrap().is_empty()
            && self.withdrawn.lock().unwrap().is_empty()
            && self.writes.lock().unwrap().is_empty()
    }
}

#[tokio::test]
async fn a_source_owner_key_gives_manage_with_recipient_read_without_interact_or_work() {
    let s = Scene::new().await;
    s.rig.tools_of_agents(json!("mcp")).await;
    let target = s.thread().await.base.id;
    is_viewer_only(&s, target, &s.cai).await;
    let source = document(
        &s,
        &s.cai,
        "Private source title",
        "Private source material",
    )
    .await;
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let before = execution(&s, target).await;
    let body = request(source.base.id, &[target], ShareMode::Live);
    // Scope permits the hypothetical control, but this same user is only a recipient Viewer.
    // The ordinary trial must retain its preview envelope, not become a resolver-level 403.
    let preview_key = key(
        &s.rig.world,
        s.cai.id(),
        vec![
            rule(
                "SharePreviewCreate",
                "document",
                source.base.id,
                &["read", "manage"],
            ),
            rule("SharePreviewCreate", "thread", target, &["read", "write"]),
        ],
    )
    .await;
    let before_grants = grants(&s, source.base.id).await;
    let before_history = history(&s, target).await;
    let trial = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &source.base.id])
        .token(&preview_key)
        .body(&SharePreviewBody::Agents {
            share: ShareWithAgents {
                link_origin: None,
                read_first: Some(true),
                ..body.clone()
            },
        })
        .send()
        .await;
    assert_eq!(trial.status, 200, "{:?}", trial.body);
    let trial = trial.ok();
    assert!(!trial.allowed);
    assert_eq!(trial.refusal.as_ref().unwrap().code, ErrorCode::AuthzDenied);
    assert_eq!(execution(&s, target).await, before);
    assert_eq!(grants(&s, source.base.id).await, before_grants);
    assert_eq!(history(&s, target).await, before_history);
    assert!(parcels.empty());
    // An optional holder that no longer resolves retains the ordinary writer's outcome.
    // Credential scopes must not invent a separate holder-existence policy for this preview.
    let missing_change = json!({"kind":"page", "change":{"name":[{
        "holders":[{"kind":"user","id":new_id()}], "choice":"full_view"
    }]}});
    let ordinary_missing_holder = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &source.base.id])
        .token(&s.cai.token)
        .raw_body(&missing_change)
        .send()
        .await
        .ok();
    let missing_holder = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &source.base.id])
        .token(&preview_key)
        .raw_body(&missing_change)
        .send()
        .await;
    assert_eq!(missing_holder.status, 200, "{:?}", missing_holder.body);
    let missing_holder = missing_holder.ok();
    assert_eq!(missing_holder.allowed, ordinary_missing_holder.allowed);
    assert_eq!(
        missing_holder.refusal.as_ref().map(|problem| &problem.code),
        ordinary_missing_holder
            .refusal
            .as_ref()
            .map(|problem| &problem.code)
    );
    assert_eq!(grants(&s, source.base.id).await, before_grants);
    assert_eq!(execution(&s, target).await, before);
    assert_eq!(history(&s, target).await, before_history);
    let writer = key(
        &s.rig.world,
        s.cai.id(),
        vec![
            rule(
                "ShareAgentCreate.live",
                "document",
                source.base.id,
                &["manage"],
            ),
            rule("ShareAgentCreate.live", "thread", target, &["read"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s, &writer, &body).await, &body, 201);
    let loan: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND holder_id=$2 AND permission='doc.manage' AND created_by=$3 AND removed_at IS NULL)")
        .bind(source.base.id).bind(target).bind(s.cai.id()).fetch_one(&s.rig.world.db.pool).await.unwrap();
    assert!(
        loan,
        "the actual owner-powered Manage contribution was written"
    );
    assert_eq!(
        execution(&s, target).await,
        before,
        "the allowed history marker is not input or control"
    );
    assert!(parcels.empty());
    let kept_grants = grants(&s, source.base.id).await;
    assert_eq!(
        receipt(send(&s, &writer, &body).await, &body, 200),
        accepted
    );
    assert_eq!(grants(&s, source.base.id).await, kept_grants);
    assert_eq!(execution(&s, target).await, before);
    let ordinary = full(send(&s, &s.cai.token, &body).await, 200);
    assert_eq!(ordinary.shares[0].id, accepted[0]);
    assert_eq!(
        ordinary.shares[0].parts.documents,
        Some(ShareDocumentsLevel::Manage)
    );
    assert_eq!(ordinary.shares[0].by.as_ref().unwrap().id, s.cai.id());
}

#[tokio::test]
async fn missing_exact_source_manage_or_any_recipient_read_refuses_the_entire_live_gift() {
    let s = Scene::new().await;
    s.rig.tools_of_agents(json!("mcp")).await;
    let one = s
        .thread_by(&s.ada, "First viewer-only recipient")
        .await
        .base
        .id;
    let two = s
        .thread_by(&s.ada, "Second viewer-only recipient")
        .await
        .base
        .id;
    is_viewer_only(&s, one, &s.cai).await;
    is_viewer_only(&s, two, &s.cai).await;
    let source = document(&s, &s.cai, "Exact source", "Source contents").await;
    let other = document(&s, &s.cai, "Other manageable source", "Other contents").await;
    let body = request(source.base.id, &[one, two], ShareMode::Live);
    let before = (
        grants(&s, source.base.id).await,
        execution(&s, one).await,
        execution(&s, two).await,
        history(&s, one).await,
        history(&s, two).await,
    );
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    for rules in [
        vec![
            rule(
                "ShareAgentCreate.live",
                "document",
                other.base.id,
                &["manage"],
            ),
            rule("ShareAgentCreate.live", "thread", one, &["read"]),
            rule("ShareAgentCreate.live", "thread", two, &["read"]),
        ],
        vec![
            rule(
                "ShareAgentCreate.live",
                "document",
                source.base.id,
                &["manage"],
            ),
            rule("ShareAgentCreate.live", "thread", one, &["read"]),
        ],
    ] {
        let denied = key(&s.rig.world, s.cai.id(), rules).await;
        assert_eq!(
            send(&s, &denied, &body).await.problem().code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            (
                grants(&s, source.base.id).await,
                execution(&s, one).await,
                execution(&s, two).await,
                history(&s, one).await,
                history(&s, two).await
            ),
            before
        );
        assert!(parcels.empty());
    }
    let allowed = key(
        &s.rig.world,
        s.cai.id(),
        vec![
            rule(
                "ShareAgentCreate.live",
                "document",
                source.base.id,
                &["manage"],
            ),
            rule("ShareAgentCreate.live", "thread", one, &["read"]),
            rule("ShareAgentCreate.live", "thread", two, &["read"]),
        ],
    )
    .await;
    assert_eq!(
        receipt(send(&s, &allowed, &body).await, &body, 201).len(),
        2,
        "both denied requests were otherwise valid gifts"
    );
}

#[tokio::test]
async fn a_snapshot_key_reads_its_actual_source_and_writes_the_recipient_without_source_manage() {
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(&s, &s.ada, "Snapshot source", "Actual copied source phrase").await;
    let body = request(source.base.id, &[target], ShareMode::Snapshot);
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let before = (
        execution(&s, target).await,
        history(&s, target).await,
        grants(&s, source.base.id).await,
    );
    for rules in [
        vec![rule(
            "ShareAgentCreate.snapshot",
            "thread",
            target,
            &["write", "read"],
        )],
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "document",
                source.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["read"]),
        ],
    ] {
        let denied = key(&s.rig.world, s.ada.id(), rules).await;
        assert_eq!(
            send(&s, &denied, &body).await.problem().code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            (
                execution(&s, target).await,
                history(&s, target).await,
                grants(&s, source.base.id).await
            ),
            before
        );
        assert!(parcels.empty());
    }
    let writer = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "document",
                source.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s, &writer, &body).await, &body, 201);
    assert_eq!(accepted.len(), 1);
    {
        let kept = parcels.kept.lock().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, accepted[0]);
        assert!(kept[0].2.files.iter().any(|file| {
            String::from_utf8_lossy(&file.bytes).contains("Actual copied source phrase")
        }));
    }
    let copy = sqlx::query_as::<_, (Id, String, i64)>(
        "SELECT thread_id,state,total_bytes FROM agent_shares WHERE id=$1",
    )
    .bind(accepted[0])
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(copy.0, target);
    assert_eq!(copy.1, "pending");
    assert!(copy.2 > 0);
    assert_eq!(
        receipt(send(&s, &writer, &body).await, &body, 200),
        accepted
    );
    assert_eq!(
        parcels.kept.lock().unwrap().len(),
        1,
        "replay keeps no second parcel"
    );
    let reader = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "document",
                source.base.id,
                &["read"],
            ),
            rule(
                "ShareAgentCreate.snapshot",
                "thread",
                target,
                &["write", "read"],
            ),
        ],
    )
    .await;
    let shown = full(send(&s, &reader, &body).await, 200);
    assert_eq!(shown.shares[0].id, accepted[0]);
    assert!(shown.shares[0].snapshot.is_some());
    assert_eq!(
        full(send(&s, &s.ada.token, &body).await, 200).shares[0].id,
        accepted[0]
    );
}

#[tokio::test]
async fn snapshot_preview_needs_source_read_not_manage_and_keeps_no_trial_work_or_parcel() {
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(
        &s,
        &s.ada,
        "Preview source",
        "Material really measured by the copy preview",
    )
    .await;
    let request = request(source.base.id, &[target], ShareMode::Snapshot);
    let body = SharePreviewBody::Agents { share: request };
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let before = (
        execution(&s, target).await,
        history(&s, target).await,
        grants(&s, source.base.id).await,
    );
    let preview_key = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("SharePreviewCreate", "document", source.base.id, &["read"]),
            rule("SharePreviewCreate", "thread", target, &["write", "read"]),
        ],
    )
    .await;
    let shown = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &source.base.id])
        .token(&preview_key)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(shown.allowed, "{shown:?}");
    assert_eq!(shown.agents.len(), 1);
    let copy = shown.agents[0]
        .copy
        .as_ref()
        .expect("the ordinary preview measured a copy");
    assert!(copy.files > 0 && copy.bytes > 0);
    assert!(!copy.already_sent);
    assert_eq!(
        (
            execution(&s, target).await,
            history(&s, target).await,
            grants(&s, source.base.id).await
        ),
        before
    );
    assert!(
        parcels.empty(),
        "a preview must never keep, write or withdraw a parcel"
    );
    let no_source = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule(
            "SharePreviewCreate",
            "thread",
            target,
            &["write", "read"],
        )],
    )
    .await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(SharePreviewCreate, &[&"documents", &source.base.id])
            .token(&no_source)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        (
            execution(&s, target).await,
            history(&s, target).await,
            grants(&s, source.base.id).await
        ),
        before
    );
    assert!(parcels.empty());
    let ordinary = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &source.base.id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(ordinary.allowed && ordinary.agents[0].copy.is_some());
    assert_eq!(
        (
            execution(&s, target).await,
            history(&s, target).await,
            grants(&s, source.base.id).await
        ),
        before
    );
    assert!(parcels.empty());
}

async fn comment_target(s: &Scene, name: &str) -> Id {
    let target = s.thread_by(&s.ada, name).await;
    assert!(target.input_access.allowed);
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(target.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    target.base.id
}

async fn comment_key_id(s: &Scene, secret: &str) -> Id {
    sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash=$1")
        .bind(orch_api_core::middleware::authentication::token_hash(
            secret,
        ))
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

fn comment_receipt(
    reply: Reply<ShareAgentCreateResponse>,
    target: Id,
    status: u16,
) -> (Id, Option<Id>) {
    assert_eq!(reply.status.as_u16(), status, "{:?}", reply.body);
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    let ShareAgentCreateResponse::Receipt(reply) = reply.ok() else {
        panic!("recipient Read was not granted")
    };
    assert_eq!(reply.shares.len(), 1);
    let row = &reply.shares[0];
    assert_eq!(row.to.kind, NodeKind::Thread);
    assert_eq!(row.to.id, target);
    assert_eq!(row.mode, ShareMode::Snapshot);
    assert_eq!(row.created, status == 201);
    assert_eq!(
        raw,
        json!({"projection":"receipt","shares":[{"id":row.id,"to":{"kind":"thread","id":target},
        "mode":"snapshot","created":row.created,"input_id":row.input_id}]})
    );
    (row.id, row.input_id)
}

async fn release_comment(s: &Scene, target: Id) {
    use orch_api_contract::threads::{ThreadQueueResume, ThreadQueueResumeBody};
    // Current origin changes can cancel an input while the queue is held. Settle that
    // ordinary preflight before choosing the revision for the explicit resume action.
    s.rig.engine.preflight(target).await.unwrap();
    let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, target)
        .await
        .unwrap()
        .unwrap();
    s.rig
        .world
        .app
        .call(ThreadQueueResume, &[&target])
        .token(&s.ada.token)
        .body(&ThreadQueueResumeBody {
            revision: row.queue_revision,
        })
        .send()
        .await
        .ok();
    s.rig.engine.preflight(target).await.unwrap();
}

const COMMENT_OPERATION: &str = "ShareAgentCreate.snapshot_comment";

#[tokio::test]
async fn snapshot_comment_dispatches_under_its_copy_identity_after_source_and_read_are_gone() {
    use orch_api_contract::documents::DocumentDelete;
    let s = Scene::new().await;
    let target = comment_target(&s, "Deferred snapshot comment").await;
    let source = document(
        &s,
        &s.ada,
        "Original copied document",
        "These bytes were copied already",
    )
    .await;
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let mut body = request(source.base.id, &[target], ShareMode::Snapshot);
    body.comment = Some("Use the accepted snapshot".into());
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(COMMENT_OPERATION, "document", source.base.id, &["read"]),
            rule(COMMENT_OPERATION, "thread", target, &["write"]),
        ],
    )
    .await;
    let accepted = comment_receipt(send(&s, &secret, &body).await, target, 201);
    let input = accepted
        .1
        .expect("the explicit comment has its own actual input");
    assert_eq!(
        comment_receipt(send(&s, &secret, &body).await, target, 200),
        accepted
    );
    let kept = parcels.kept.lock().unwrap().clone();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].2.files.iter().any(|file| {
        String::from_utf8_lossy(&file.bytes).contains("These bytes were copied already")
    }));
    s.rig
        .world
        .app
        .call(DocumentDelete, &[&source.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let token = comment_key_id(&s, &secret).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token)
        .bind(json!({"rules":[rule(COMMENT_OPERATION,"thread",target,&["write"])]}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    release_comment(&s, target).await;
    let start = s.rig.turn_start(target, 0).await;
    assert!(start.text.ends_with("Use the accepted snapshot"));
    s.rig
        .answer(target, &start, "snapshot-comment-done", "done")
        .await;
    rig::eventually(
        "the snapshot comment's native delivery acknowledgement",
        || async {
            orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
                .await
                .unwrap()
                .unwrap()
                .state
                == "delivered"
        },
    )
    .await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "delivered");
    let authority = row.submission_authority.unwrap();
    assert_eq!(authority["origins"][0]["operation"], COMMENT_OPERATION);
    assert_eq!(
        authority["snapshot_comment"],
        json!({"copy_id":accepted.0,"source":{"kind":"document","id":source.base.id},"thread_id":target,"input_id":input})
    );
    assert!(!authority.to_string().contains("node_ancestors"));
    assert_eq!(
        *parcels.kept.lock().unwrap(),
        kept,
        "the accepted copy is not rebuilt from a deleted source"
    );
    s.rig
        .say(
            target,
            rig::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    s.rig.engine.stop();
}

#[tokio::test]
async fn snapshot_comment_rechecks_live_key_recipient_and_copy_creator() {
    for change in ["revoked", "target", "creator"] {
        let s = Scene::new().await;
        let target = comment_target(&s, "Bound snapshot comment").await;
        let other = comment_target(&s, "Unrelated target").await;
        let source = document(
            &s,
            &s.ada,
            "Bound source",
            "Copied before the boundary changed",
        )
        .await;
        let parcels = Arc::new(ParcelCalls::default());
        s.rig.engine.attach_parcels(parcels.clone());
        let mut body = request(source.base.id, &[target], ShareMode::Snapshot);
        body.comment = Some("Queued before current restriction".into());
        let secret = key(
            &s.rig.world,
            s.ada.id(),
            vec![
                rule(COMMENT_OPERATION, "document", source.base.id, &["read"]),
                rule(COMMENT_OPERATION, "thread", target, &["write"]),
                rule(COMMENT_OPERATION, "thread", other, &["write"]),
            ],
        )
        .await;
        let (copy, input) = comment_receipt(send(&s, &secret, &body).await, target, 201);
        let input = input.unwrap();
        let token = comment_key_id(&s, &secret).await;
        match change {
            "revoked" => {
                orch_db::queries::identity::revoke_api_token(
                    &mut *s.rig.world.conn().await,
                    token,
                    s.ada.id(),
                    s.rig.world.now(),
                )
                .await
                .unwrap();
            }
            "target" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(token)
                .bind(json!({"rules":[rule(COMMENT_OPERATION,"document",source.base.id,&["read"]),rule(COMMENT_OPERATION,"thread",other,&["write"])]}))
                .execute(s.rig.db.pool()).await.unwrap();
            }
            "creator" => {
                sqlx::query("UPDATE agent_shares SET created_by=$2 WHERE id=$1")
                    .bind(copy)
                    .bind(s.eve.id())
                    .execute(s.rig.db.pool())
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        release_comment(&s, target).await;
        assert!(s.rig.turn_starts(target).is_empty(), "{change}");
        let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(row.state, "delivered", "{change}");
        assert_eq!(
            row.content,
            json!([{"type":"text","text":"Queued before current restriction"}])
        );
        assert_eq!(parcels.kept.lock().unwrap().len(), 1);
        s.rig.engine.stop();
    }
}

#[tokio::test]
async fn snapshot_comment_preview_checks_the_input_without_persisting_authority_or_work() {
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(
        &s,
        &s.ada,
        "Preview comment source",
        "Prepared snapshot bytes",
    )
    .await;
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let mut share = request(source.base.id, &[target], ShareMode::Snapshot);
    share.comment = Some("Validate this without starting anything".into());
    let before = (
        execution(&s, target).await,
        history(&s, target).await,
        grants(&s, source.base.id).await,
    );
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("SharePreviewCreate", "document", source.base.id, &["read"]),
            rule("SharePreviewCreate", "thread", target, &["write", "read"]),
        ],
    )
    .await;
    let preview = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &source.base.id])
        .token(&secret)
        .body(&SharePreviewBody::Agents { share })
        .send()
        .await
        .ok();
    assert!(preview.allowed, "{preview:?}");
    assert!(
        preview.agents[0]
            .copy
            .as_ref()
            .is_some_and(|copy| copy.files > 0)
    );
    assert_eq!(
        (
            execution(&s, target).await,
            history(&s, target).await,
            grants(&s, source.base.id).await
        ),
        before
    );
    assert!(parcels.empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn replay_of_a_no_comment_copy_does_not_retrofit_an_authored_input() {
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(
        &s,
        &s.ada,
        "No comment source",
        "Copied without an instruction",
    )
    .await;
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let mut body = request(source.base.id, &[target], ShareMode::Snapshot);
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "document",
                source.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
            rule(COMMENT_OPERATION, "document", source.base.id, &["read"]),
            rule(COMMENT_OPERATION, "thread", target, &["write"]),
        ],
    )
    .await;
    let first = receipt(send(&s, &secret, &body).await, &body, 201);
    let before = execution(&s, target).await;
    assert!(before.inputs.is_empty() && before.turns.is_empty());
    body.comment = Some("This replacement is not a new instruction".into());
    assert_eq!(
        comment_receipt(send(&s, &secret, &body).await, target, 200),
        (first[0], None)
    );
    assert_eq!(execution(&s, target).await, before);
    assert_eq!(parcels.kept.lock().unwrap().len(), 1);
    s.rig.engine.stop();
}

#[tokio::test]
async fn topic_snapshot_filters_out_child_documents_outside_the_keys_exact_source_scope() {
    use orch_api_contract::share::ShareWhatNode;
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(
        &s,
        &s.ada,
        "Scoped child title",
        "Ordinarily readable child body",
    )
    .await;
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let mut body = request(source.base.id, &[target], ShareMode::Snapshot);
    body.what = ShareWhat::Node(ShareWhatNode {
        node_id: s.tree.topic.base.id,
    });
    body.inside = Some(true);
    full(send(&s, &s.ada.token, &body).await, 201);
    assert!(parcels.kept.lock().unwrap()[0].2.files.iter().any(|file| {
        String::from_utf8_lossy(&file.bytes).contains("Ordinarily readable child body")
    }));
    body.client_id = new_id().to_string();
    let narrow = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "topic",
                s.tree.topic.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
        ],
    )
    .await;
    receipt(send(&s, &narrow, &body).await, &body, 201);
    {
        let kept = parcels.kept.lock().unwrap();
        let files = &kept[1].2.files;
        assert!(files.iter().any(|file| file.path == "index.md"));
        assert!(
            !files
                .iter()
                .any(|file| file.path.contains("Scoped child title")
                    || String::from_utf8_lossy(&file.bytes).contains("Scoped child title")
                    || String::from_utf8_lossy(&file.bytes)
                        .contains("Ordinarily readable child body"))
        );
    }
    body.client_id = new_id().to_string();
    let readable = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "topic",
                s.tree.topic.base.id,
                &["read"],
            ),
            rule(
                "ShareAgentCreate.snapshot",
                "document",
                source.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
        ],
    )
    .await;
    receipt(send(&s, &readable, &body).await, &body, 201);
    assert!(parcels.kept.lock().unwrap()[2].2.files.iter().any(|file| {
        String::from_utf8_lossy(&file.bytes).contains("Ordinarily readable child body")
    }));
    assert!(execution(&s, target).await.inputs.is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn snapshot_comment_future_attachment_keeps_the_original_credentials_current_read_bound() {
    use orch_api_contract::threads::{
        ContentPart, FILENAME_HEADER, ThreadInputUpdate, ThreadInputUpdateBody, UploadCreate,
    };
    let s = Scene::new().await;
    let target = comment_target(&s, "Snapshot attachment boundary").await;
    let source = document(
        &s,
        &s.ada,
        "Completed snapshot source",
        "Snapshot content is separate from later input bytes",
    )
    .await;
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let mut body = request(source.base.id, &[target], ShareMode::Snapshot);
    body.comment = Some("An accepted comment before its attachment edit".into());
    let upload_read = json!({"target":{"kind":"account","id":s.ada.id(),"reach":"self"},"resources":["upload"],
        "allow":[{"operation":COMMENT_OPERATION,"actions":["read"]}]});
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(COMMENT_OPERATION, "document", source.base.id, &["read"]),
            rule(COMMENT_OPERATION, "thread", target, &["write"]),
            upload_read,
        ],
    )
    .await;
    let (copy, input) = comment_receipt(send(&s, &secret, &body).await, target, 201);
    let input = input.unwrap();
    let upload = s
        .rig
        .world
        .app
        .call(UploadCreate, &[])
        .token(&s.ada.token)
        .header("content-type", "image/jpeg")
        .header(FILENAME_HEADER, "later.jpg")
        .bytes(b"later accepted input bytes need a current readable witness")
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(ThreadInputUpdate, &[&target, &input])
        .token(&s.ada.token)
        .body(&ThreadInputUpdateBody {
            content: Some(vec![ContentPart::File {
                path: ".orch/attachments/later.jpg".into(),
                name: upload.name,
                size: upload.size,
                blob: Some(upload.blob),
            }]),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok();
    let token = comment_key_id(&s, &secret).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(token)
        .bind(json!({"rules":[rule(COMMENT_OPERATION,"document",source.base.id,&["read"]),rule(COMMENT_OPERATION,"thread",target,&["write"])]}))
        .execute(s.rig.db.pool()).await.unwrap();
    release_comment(&s, target).await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state, "cancelled",
        "the later editor cannot lend a missing read to the original key"
    );
    assert!(row.content.to_string().contains(&upload.blob.to_string()));
    assert!(s.rig.turn_starts(target).is_empty());
    assert_eq!(parcels.kept.lock().unwrap()[0].0, copy);
    assert_eq!(
        parcels.kept.lock().unwrap().len(),
        1,
        "the completed copy is not reconstructed"
    );
    s.rig.engine.stop();
}

#[tokio::test]
async fn a_direct_node_chat_copy_bounds_its_default_title_but_keeps_explicit_chat_names() {
    use orch_api_contract::chats::{
        ChatMessageCreate, ChatMessageCreateBody, ChatPart, OrgChatCreate, OrgChatCreateBody,
    };
    use orch_api_contract::share::{ShareLevel, ShareWhatChat};

    const NODE_TITLE: &str = "Protected node title in a default chat heading";
    const MESSAGE: &str = "The chat message itself is explicitly in the copy scope";
    let s = Scene::new().await;
    let source = s.thread_by(&s.ada, NODE_TITLE).await;
    let target = s.thread().await.base.id;
    let chat = source.base.chat_id;
    let row = orch_db::queries::chats::chat(&mut *s.rig.world.conn().await, chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.node_id, Some(source.base.id));
    assert!(
        row.name.is_none(),
        "the test exercises the actual native default label"
    );
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Text {
                text: MESSAGE.into(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let body = |chat| {
        let mut body = request(chat, &[target], ShareMode::Snapshot);
        body.what = ShareWhat::Chat(ShareWhatChat { chat_id: chat });
        body.parts = Some(ShareParts {
            chat: Some(ShareLevel::Read),
            ..ShareParts::default()
        });
        body
    };
    // The ordinary source proves that its real current heading is available to its owner.
    let ordinary = full(send(&s, &s.ada.token, &body(chat)).await, 201);
    let ordinary_id = ordinary.shares[0].id;
    {
        let kept = parcels.kept.lock().unwrap();
        let files = &kept.iter().find(|(id, _, _)| *id == ordinary_id).unwrap().2;
        assert!(
            files
                .files
                .iter()
                .any(|file| String::from_utf8_lossy(&file.bytes).contains(NODE_TITLE))
        );
    }
    for operation in [
        None,
        Some("ShareAgentCreate.snapshot"),
        Some("NodeGet.read"),
    ] {
        let mut rules = vec![
            rule("ShareAgentCreate.snapshot", "chat", chat, &["read"]),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
        ];
        if let Some(operation) = operation {
            rules.push(rule(operation, "thread", source.base.id, &["read"]));
        }
        let secret = key(&s.rig.world, s.ada.id(), rules).await;
        let request = body(chat);
        let accepted = receipt(send(&s, &secret, &request).await, &request, 201);
        let kept = parcels.kept.lock().unwrap();
        let files = &kept.iter().find(|(id, _, _)| *id == accepted[0]).unwrap().2;
        let text = files
            .files
            .iter()
            .map(|file| String::from_utf8_lossy(&file.bytes))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains(MESSAGE),
            "scoping the title must retain the allowed conversation"
        );
        assert_eq!(
            text.contains(NODE_TITLE),
            operation == Some("ShareAgentCreate.snapshot"),
            "a default chat heading/index needs this operation's Read on its actual node",
        );
    }
    // A real group's explicit name is its own Chat content, even if its words happen
    // to repeat an ungranted Node title. Scope projection must not scrub supplied text.
    let explicit = format!("Explicit chat content: {NODE_TITLE}");
    let group = s
        .rig
        .world
        .app
        .call(OrgChatCreate, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .body(&OrgChatCreateBody {
            visibility: None,
            home_id: None,
            // These fixture people are actual org members; Bea intentionally is not.
            user_ids: vec![s.dan.id(), s.eve.id()],
            name: Some(explicit.clone()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary chat fixture is full");
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "chat",
                group.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
        ],
    )
    .await;
    let request = body(group.base.id);
    let accepted = receipt(send(&s, &secret, &request).await, &request, 201);
    {
        let kept = parcels.kept.lock().unwrap();
        let files = &kept.iter().find(|(id, _, _)| *id == accepted[0]).unwrap().2;
        assert!(
            files
                .files
                .iter()
                .any(|file| String::from_utf8_lossy(&file.bytes).contains(&explicit))
        );
    }
}

// Follow-up coverage for existing persisted attachment witnesses and live chat-name projection.
fn chat_share(chat: Id, recipients: &[Id], mode: ShareMode) -> ShareWithAgents {
    let mut body = request(chat, recipients, mode);
    body.what = ShareWhat::Chat(orch_api_contract::share::ShareWhatChat { chat_id: chat });
    body.parts = Some(ShareParts {
        chat: Some(orch_api_contract::share::ShareLevel::Read),
        ..ShareParts::default()
    });
    body
}

fn parcel_text(parcels: &ParcelCalls, copy: Id) -> String {
    let kept = parcels.kept.lock().unwrap();
    let (_, _, files) = kept
        .iter()
        .find(|(id, _, _)| *id == copy)
        .expect("the actual parcel was kept");
    files
        .files
        .iter()
        .map(|file| String::from_utf8(file.bytes.clone()).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn snapshot_file_references_use_the_persisted_chat_witness_and_keep_recipient_read_authority()
{
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ChatMessageCreate, ChatMessageCreateBody, ChatPart,
    };
    use orch_api_contract::documents::{BlockContent, Inline, InlineRefKind};
    use orch_api_contract::share::ShareAgentDelete;
    use orch_api_contract::threads::FILENAME_HEADER;
    use orch_engine::tools::tree::Where;
    let s = Scene::new().await;
    let target = s
        .thread_by(&s.ada, "Attachment snapshot receiver")
        .await
        .base
        .id;
    let origin = places::make_topic(
        &s.rig.world,
        &s.ada,
        s.tree.project.base.id,
        "Attachment origin",
    )
    .await;
    let chat = origin.base.chat_id;
    let uploaded = s
        .rig
        .world
        .app
        .call(ChatAttachmentCreate, &[&chat])
        .token(&s.ada.token)
        .header(FILENAME_HEADER, "confidential-carried.txt")
        .bytes(b"Real attachment witness bytes")
        .send()
        .await
        .ok();
    let posted = s
        .rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Attachment {
                blob: uploaded.blob,
                name: uploaded.name.clone(),
                size: uploaded.size,
                media_type: uploaded.media_type.clone(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .message;
    let (_, before_tree, _) = s
        .rig
        .engine
        .agent_tree(target, None, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(
        before_tree.chat_origin(chat).is_none(),
        "the positive read will come from the explicit ordinary gift"
    );
    let live = chat_share(chat, &[target], ShareMode::Live);
    let gift = full(send(&s, &s.ada.token, &live).await, 201).shares[0].id;
    // The actual recipient tree lists the real file and persists the exact source tuple. No
    // reference UUID is guessed and no chat_file_references row is manufactured by the fixture.
    let (_, tree, _) = s
        .rig
        .engine
        .agent_tree(target, None, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tree.chat_origin(chat), Some(Some(origin.base.id)));
    let prefix = format!("{}chat.files/", tree.path_of_node(origin.base.id).unwrap());
    let mut conn = s.rig.world.conn().await;
    let listing = tree
        .list(&mut conn, Where::ChatFiles(chat), &prefix, 1, None, None)
        .await
        .unwrap();
    let entry = listing.entries.iter().find(|entry|matches!(entry.at,Where::ChatFile{chat:found,message,..} if found==chat&&message==posted.id)).unwrap();
    let reference = entry.id;
    let expected_link = orch_documents::references::agent_link(&entry.name, &entry.path, None);
    let tuple: (Id, Id, i32) =
        sqlx::query_as("SELECT chat_id,message_id,carried FROM chat_file_references WHERE id=$1")
            .bind(reference)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(tuple, (chat, posted.id, 1));
    drop(conn);
    let mut body = desk::body("Attachment reference source");
    body.access = Some(NewDocumentAccess::Private);
    let mut block = desk::paragraph("Before the saved attachment reference. ");
    let Some(BlockContent::Inline(parts)) = &mut block.content else {
        unreachable!()
    };
    parts.push(Inline::Ref {
        display: None,
        kind: InlineRefKind::File,
        id: reference,
        path: None,
        part: None,
        label: Some("Saved attachment label".into()),
        name: None,
    });
    body.blocks = Some(vec![block]);
    let source = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let ordinary = request(source.base.id, &[target], ShareMode::Snapshot);
    let ordinary_id = full(send(&s, &s.ada.token, &ordinary).await, 201).shares[0].id;
    let normal = parcel_text(&parcels, ordinary_id);
    assert!(
        normal.contains(&expected_link),
        "ordinary recipient filename/path must be real: {normal}"
    );
    let narrow = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "document",
                source.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
        ],
    )
    .await;
    let copy = request(source.base.id, &[target], ShareMode::Snapshot);
    let hidden = receipt(send(&s, &narrow, &copy).await, &copy, 201)[0];
    let hidden = parcel_text(&parcels, hidden);
    assert!(
        hidden.contains("Saved attachment label (no access)"),
        "optional scope loss hides the reference without refusing its document: {hidden}"
    );
    assert!(!hidden.contains(&uploaded.name));
    let sufficient = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(
                "ShareAgentCreate.snapshot",
                "document",
                source.base.id,
                &["read"],
            ),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
            rule("ShareAgentCreate.snapshot", "chat", chat, &["read"]),
            rule(
                "ShareAgentCreate.snapshot",
                "org",
                s.tree.org.base.id,
                &["read"],
            ),
            rule(
                "ShareAgentCreate.snapshot",
                "project",
                s.tree.project.base.id,
                &["read"],
            ),
            rule(
                "ShareAgentCreate.snapshot",
                "topic",
                origin.base.id,
                &["read"],
            ),
        ],
    )
    .await;
    let copy = request(source.base.id, &[target], ShareMode::Snapshot);
    let shown = receipt(send(&s, &sufficient, &copy).await, &copy, 201)[0];
    assert!(
        parcel_text(&parcels, shown).contains(&expected_link),
        "the key admits the actual persisted Chat and its live path sources"
    );
    // The same caller, key, source and reference remain; only the recipient's ordinary live read is removed.
    s.rig
        .world
        .app
        .call(ShareAgentDelete, &[&gift])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let (_, tree, _) = s
        .rig
        .engine
        .agent_tree(target, None, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(tree.chat_origin(chat).is_none());
    let copy = request(source.base.id, &[target], ShareMode::Snapshot);
    let hidden = receipt(send(&s, &sufficient, &copy).await, &copy, 201)[0];
    assert!(parcel_text(&parcels, hidden).contains("Saved attachment label (no access)"));
    // Restore the same ordinary recipient grant, then invalidate the live message behind the still
    // persisted tuple. A scope grant cannot authorize a deleted message or stale carried-file slot.
    full(
        send(
            &s,
            &s.ada.token,
            &chat_share(chat, &[target], ShareMode::Live),
        )
        .await,
        201,
    );
    s.rig
        .world
        .app
        .call(
            orch_api_contract::chats::ChatMessageDelete,
            &[&chat, &posted.id],
        )
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let tuple_still_present: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chat_file_references WHERE id=$1)")
            .bind(reference)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(tuple_still_present);
    let copy = request(source.base.id, &[target], ShareMode::Snapshot);
    let hidden = receipt(send(&s, &sufficient, &copy).await, &copy, 201)[0];
    assert!(parcel_text(&parcels, hidden).contains("Saved attachment label (no access)"));
    s.rig.engine.stop();
}

#[tokio::test]
async fn snapshot_chat_live_agent_names_obey_same_operation_read_without_scrubbing_saved_words() {
    use orch_api_contract::chats::{
        ChatMentionPreview, ChatMentionPreviewBody, ChatMentionTarget, ChatMessageCreate,
        ChatMessageCreateBody, ChatPart,
    };
    let s = Scene::new().await;
    let live_name = "External live agent label 742";
    let mentioned = s.thread_by(&s.ada, live_name).await.base.id;
    let target = s.thread_by(&s.ada, "Chat snapshot receiver").await.base.id;
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(mentioned)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let chat = s.tree.topic.base.chat_id;
    let literal = format!("Stored literal: {live_name}. Please ask ");
    let parts = vec![
        ChatPart::Text {
            text: literal.clone(),
        },
        ChatPart::Mention {
            target: ChatMentionTarget::Agent {
                thread_id: mentioned,
            },
        },
    ];
    let preview = s
        .rig
        .world
        .app
        .call(ChatMentionPreview, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMentionPreviewBody {
            parts: parts.clone(),
            reply_to: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(preview.agent_asks.len(), 1);
    assert_eq!(
        preview.agent_asks[0].thread.name.as_deref(),
        Some(live_name)
    );
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMessageCreateBody {
            parts,
            reply_to: None,
            agent_review: preview.agent_review,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let parcels = Arc::new(ParcelCalls::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let ordinary = chat_share(chat, &[target], ShareMode::Snapshot);
    let ordinary_id = full(send(&s, &s.ada.token, &ordinary).await, 201).shares[0].id;
    let normal = parcel_text(&parcels, ordinary_id);
    assert!(
        normal.contains(&format!("@{live_name}")),
        "the original body reader actually resolves this live name: {normal}"
    );
    assert!(normal.contains(&literal));
    assert!(
        normal.contains("@ada"),
        "the chat's public author handle is part of its ordinary written form"
    );
    let narrow = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("ShareAgentCreate.snapshot", "chat", chat, &["read"]),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
        ],
    )
    .await;
    let body = chat_share(chat, &[target], ShareMode::Snapshot);
    let hidden_id = receipt(send(&s, &narrow, &body).await, &body, 201)[0];
    let hidden = parcel_text(&parcels, hidden_id);
    assert!(
        hidden.contains(&literal),
        "stored words are not scrubbed because they resemble a protected live name"
    );
    assert!(hidden.contains("@ada"));
    assert!(hidden.contains("@an agent"));
    assert!(!hidden.contains(&format!("@{live_name}")));
    let sufficient = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("ShareAgentCreate.snapshot", "chat", chat, &["read"]),
            rule("ShareAgentCreate.snapshot", "thread", target, &["write"]),
            rule("ShareAgentCreate.snapshot", "thread", mentioned, &["read"]),
        ],
    )
    .await;
    let body = chat_share(chat, &[target], ShareMode::Snapshot);
    let shown_id = receipt(send(&s, &sufficient, &body).await, &body, 201)[0];
    let shown = parcel_text(&parcels, shown_id);
    assert!(shown.contains(&literal));
    assert!(shown.contains("@ada"));
    assert!(shown.contains(&format!("@{live_name}")));
    assert!(s.rig.turn_starts(mentioned).is_empty());
    assert!(s.rig.turn_starts(target).is_empty());
    s.rig.engine.stop();
}
