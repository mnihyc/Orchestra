//! Pure live access gifts use the real Share producer without directing the recipient agent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod rig;
mod support;

use async_trait::async_trait;
use orch_api_contract::access::{
    AgentLevel, EffectiveAccessGet, EffectiveAccessGetQuery, PermissionPageGet, SharePreviewBody,
    SharePreviewCreate,
};
use orch_api_contract::documents::{NewDocumentAccess, NodeDocumentCreate};
use orch_api_contract::share::{
    ShareAgentCreate, ShareAgentDelete, ShareDocumentsLevel, ShareParts, ShareTo, ShareToThread,
    ShareWhat, ShareWhatDocument, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_db::Conn;
use orch_db::queries::shares::CopyRow;
use orch_engine::copies::Files;
use orch_engine::sharing::{FileWritten, ParcelFault, Parcels};
use orch_types::{Id, Json};
use rig::Scene;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// An access-only act has no reason to retain, write or remove workspace files.
#[derive(Default)]
struct WorkspaceCalls(AtomicUsize);

#[async_trait]
impl Parcels for WorkspaceCalls {
    async fn keep(
        &self,
        _conn: &mut Conn,
        _copy: Id,
        _path: &str,
        _files: &Files,
    ) -> Result<Option<Vec<u8>>, ParcelFault> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ParcelFault(
            "an access gift must not keep workspace files".into(),
        ))
    }

    async fn withdraw(&self, _conn: &mut Conn, _copy: &CopyRow) -> Result<(), ParcelFault> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ParcelFault(
            "an access gift must not remove workspace files".into(),
        ))
    }

    async fn withdraw_before(
        &self,
        _conn: &mut Conn,
        _copy: &CopyRow,
        before: orch_engine::sharing::BeforeParcel<'_>,
    ) -> Result<(), orch_engine::sharing::ParcelFailure> {
        before().map_err(orch_authz::pages::EffectFailure::BeforeEffect)?;
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(orch_authz::pages::EffectFailure::Native(ParcelFault(
            "an access gift must not remove workspace files".into(),
        )))
    }

    async fn write_file(
        &self,
        _conn: &mut Conn,
        _thread: Id,
        _path: &str,
        _bytes: &[u8],
    ) -> Result<FileWritten, ParcelFault> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ParcelFault(
            "an access gift must not write workspace files".into(),
        ))
    }
}

#[tokio::test]
async fn selected_viewer_read_is_a_separate_standing_grant_and_does_not_direct_the_agent() {
    let s = Scene::new().await;
    s.rig.tools_of_agents(json!("mcp")).await;
    let thread = s.thread().await.base.id;
    let recipient = s
        .rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &thread])
        .token(&s.ada.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.cai.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        recipient
            .permissions
            .iter()
            .any(|row| row.permission == "thread.view")
    );
    assert!(
        !recipient
            .permissions
            .iter()
            .any(|row| row.permission == "thread.interact")
    );
    let mut body = desk::body("A separately disclosed original");
    body.access = Some(NewDocumentAccess::Private);
    let document = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let human_before = s
        .rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"documents", &document.base.id])
        .token(&s.cai.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.ada.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        !human_before
            .permissions
            .iter()
            .any(|row| row.permission == "doc.view")
    );
    let workspace = Arc::new(WorkspaceCalls::default());
    s.rig.engine.attach_parcels(workspace.clone());
    let before = execution_state(&s, thread).await;
    let request = ShareWithAgents {
        link_origin: None,
        viewers_read: Some(true),
        read_first: None,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: document.base.id,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Manage),
            ..ShareParts::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: thread }]),
        comment: None,
        expires_at: None,
        client_id: "optional-standing-read".into(),
    };
    let directing = s
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.cai.token)
        .body(&ShareWithAgents {
            link_origin: None,
            read_first: Some(true),
            ..request.clone()
        })
        .send()
        .await;
    assert_eq!(
        directing.status, 403,
        "a first-reading directive still requires recipient Interact"
    );
    let copying = s
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.cai.token)
        .body(&ShareWithAgents {
            link_origin: None,
            viewers_read: None,
            mode: ShareMode::Snapshot,
            parts: Some(ShareParts {
                documents: Some(ShareDocumentsLevel::Read),
                ..ShareParts::default()
            }),
            ..request.clone()
        })
        .send()
        .await;
    assert_eq!(
        copying.status, 403,
        "workspace delivery still requires recipient Interact"
    );
    assert_eq!(execution_state(&s, thread).await, before);
    let preview = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &document.base.id])
        .token(&s.cai.token)
        .body(&SharePreviewBody::Agents {
            share: request.clone(),
        })
        .send()
        .await
        .ok();
    assert!(preview.allowed);
    assert_eq!(preview.agents.len(), 1);
    assert_eq!(preview.agents[0].viewer_entries.len(), 1);
    assert_eq!(
        execution_state(&s, thread).await,
        before,
        "preview accepts no work"
    );
    let not_yet = s
        .rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"documents", &document.base.id])
        .token(&s.cai.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.ada.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        !not_yet
            .permissions
            .iter()
            .any(|row| row.permission == "doc.view"),
        "the ordinary human preview rolls back"
    );
    let made = s
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.cai.token)
        .body(&request)
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary session receives the full share projection");
    assert_eq!(made.shares.len(), 1);
    let share = &made.shares[0];
    assert_eq!(share.viewer_entries.len(), 1);
    assert!(
        matches!(&share.viewer_entries[0].holder, orch_api_contract::access::Holder::Role {
        role:orch_api_contract::access::Role::Viewer, at:Some(at), ..
    } if at.id == thread)
    );
    let viewer_entry = share.viewer_entries[0].id;
    let human = s
        .rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"documents", &document.base.id])
        .token(&s.cai.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.ada.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        human
            .permissions
            .iter()
            .any(|row| row.permission == "doc.view")
    );
    for withheld in ["doc.edit", "doc.manage"] {
        assert!(
            !human
                .permissions
                .iter()
                .any(|row| row.permission == withheld)
        );
    }
    s.rig
        .world
        .app
        .call(ShareAgentDelete, &[&share.id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    let remaining:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM permission_entries WHERE id=$1 AND holder_kind='role' AND holder_role='viewer' AND permission='doc.view' AND removed_at IS NULL)")
        .bind(viewer_entry).fetch_one(&s.rig.world.db.pool).await.unwrap();
    assert!(
        remaining,
        "taking back the agent loan must leave the normal human grant intact"
    );
    let human_after = s
        .rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"documents", &document.base.id])
        .token(&s.cai.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.ada.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        human_after
            .permissions
            .iter()
            .any(|row| row.permission == "doc.view")
    );
    assert_eq!(execution_state(&s, thread).await, before);
    assert_eq!(workspace.0.load(Ordering::SeqCst), 0);
    assert!(s.rig.turn_starts(thread).is_empty());
}

#[derive(Debug, PartialEq)]
struct ExecutionState {
    state: String,
    current_turn: Option<Id>,
    inputs: Vec<Id>,
    turns: Vec<Id>,
    notices: Json,
    copies: Vec<Id>,
    target_commands: Vec<Json>,
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

async fn execution_state(s: &Scene, thread: Id) -> ExecutionState {
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
    let target_commands = s
        .rig
        .seen
        .all()
        .iter()
        .map(|command| serde_json::to_value(command).unwrap())
        .filter(|command| names_thread(command, &thread.to_string()))
        .collect();
    ExecutionState {
        state: row.state,
        current_turn: row.current_turn_id,
        inputs,
        turns,
        notices: row.pending_notices,
        copies,
        target_commands,
    }
}

#[tokio::test]
async fn a_thread_viewer_who_owns_a_document_can_give_its_agent_manage_without_directing_it() {
    let s = Scene::new().await;
    s.rig.tools_of_agents(json!("mcp")).await;
    let recipient = s.thread().await;
    let thread = recipient.base.id;
    let access = s
        .rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &thread])
        .token(&s.ada.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.cai.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        access
            .permissions
            .iter()
            .any(|row| row.permission == "thread.view")
    );
    assert!(
        !access
            .permissions
            .iter()
            .any(|row| row.permission == "thread.interact")
    );

    let mut body = desk::body("A deliberate access gift");
    body.access = Some(NewDocumentAccess::Private);
    let document = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(document.base.owner.id, s.cai.id());
    let page = s
        .rig
        .world
        .app
        .call(PermissionPageGet, &[&"documents", &document.base.id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert!(page.gives.agents.contains(&AgentLevel::Manage));
    let workspace = Arc::new(WorkspaceCalls::default());
    s.rig.engine.attach_parcels(workspace.clone());
    let before = execution_state(&s, thread).await;

    let reply = s
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.cai.token)
        .body(&ShareWithAgents {
            link_origin: None,
            viewers_read: None,
            what: ShareWhat::Document(ShareWhatDocument {
                document_id: document.base.id,
            }),
            inside: None,
            parts: Some(ShareParts {
                documents: Some(ShareDocumentsLevel::Manage),
                ..ShareParts::default()
            }),
            mode: ShareMode::Live,
            to: ShareTo::Threads(vec![ShareToThread { thread_id: thread }]),
            download: Some(false),
            read_first: None,
            comment: None,
            expires_at: None,
            client_id: "pure-live-gift".into(),
        })
        .send()
        .await;
    assert_eq!(
        reply.status, 201,
        "the normal pure grant must not require recipient Interact: {:?}",
        reply.body
    );
    let made = reply
        .ok()
        .into_full()
        .expect("the ordinary session receives the full share projection");
    assert_eq!(made.shares.len(), 1);
    assert_eq!(made.shares[0].to.id, thread);
    assert_eq!(made.shares[0].by.as_ref().map(|by| by.id), Some(s.cai.id()));
    assert_eq!(
        made.shares[0].parts.documents,
        Some(ShareDocumentsLevel::Manage)
    );
    let manage_entry: Id = sqlx::query_scalar(
        "SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1
         AND permission='doc.manage' AND holder_kind='agents' AND holder_id=$2
         AND created_by=$3 AND removed_at IS NULL",
    )
    .bind(document.base.id)
    .bind(thread)
    .bind(s.cai.id())
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert!(
        made.shares[0]
            .entries
            .iter()
            .any(|entry| entry.id == manage_entry)
    );
    assert!(!made.shares[0].read_first);
    assert!(made.shares[0].input.is_none());
    tokio::task::yield_now().await;
    assert_eq!(
        execution_state(&s, thread).await,
        before,
        "access-only gifting must not inject or deliver work; a human-only timeline marker may change the sequence"
    );
    assert_eq!(workspace.0.load(Ordering::SeqCst), 0);
    assert!(s.rig.turn_starts(thread).is_empty());
}
