//! One exact download target proof is consumed by the native write's admission transaction.

use super::*;
use crate::{
    sharing::FileWritten,
    tools::{
        Answer, CallContext,
        definitions::{Code, Refusal},
        shared::{Resolved, Target},
    },
};
use orch_db::queries::threads;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Delivery {
    pub blob: Hash,
    pub size: i64,
    pub to: String,
    pub path: String,
    pub ready: Json,
    pub downloading: Json,
    kind: String,
    object: Id,
    node: Option<Id>,
    root: Id,
}

pub(crate) struct Proof {
    thread: Id,
    asked: String,
    workspace: String,
    expected: Json,
    arguments: Json,
    delivery: Delivery,
}

struct Scope {
    proof: Proof,
    selected: Mutex<Option<Delivery>>,
    refused: Mutex<Option<Refusal>>,
}

tokio::task_local! {static DOWNLOAD:Arc<Scope>;}

pub(crate) struct Checked<T> {
    pub result: T,
    pub delivery: Option<Delivery>,
    pub refused: Option<Refusal>,
}

pub(crate) async fn scope<T>(
    proof: Proof,
    work: impl std::future::Future<Output = T>,
) -> Checked<T> {
    let owner = Arc::new(Scope {
        proof,
        selected: Mutex::new(None),
        refused: Mutex::new(None),
    });
    let result = DOWNLOAD.scope(owner.clone(), work).await;
    let delivery = owner
        .selected
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let refused = owner
        .refused
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    Checked {
        result,
        delivery,
        refused,
    }
}

pub(crate) fn retained(value: Option<Json>) -> Result<(), EngineError> {
    if let Some(value) = value {
        let delivery: Delivery = serde_json::from_value(value)?;
        let _ = DOWNLOAD.try_with(|scope| {
            *scope
                .selected
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(delivery)
        });
    }
    Ok(())
}

fn refused(scope: &Scope, refusal: Refusal) -> EngineError {
    *scope.refused.lock().unwrap_or_else(PoisonError::into_inner) = Some(refusal);
    EngineError::unreadable("The prepared download no longer meets its admission proof.")
}

fn owner(resolved: &Resolved) -> Result<(String, Id, Option<Id>), EngineError> {
    Ok(match resolved.target {
        Target::Conversation { thread } => ("thread".into(), thread, Some(thread)),
        Target::NodeChat { chat, node } => ("chat".into(), chat, Some(node)),
        Target::PeopleChat { chat } => ("chat".into(), chat, None),
        Target::ChatFile { chat, node, .. } => ("chat".into(), chat, node),
        Target::Document { document }
        | Target::DocumentFile { document, .. }
        | Target::Version { document, .. } => ("document".into(), document, None),
        Target::Directory(_) => return Err(EngineError::ToolCallIdentity),
    })
}

/// Every identity/counter comes from actual rows or the exact written attachment slot.
/// Immutable version rows retain their actual number and the owning document's current metadata.
pub(crate) async fn stamp(conn: &mut Conn, resolved: &Resolved) -> Result<Json, EngineError> {
    let (kind, id, _) = owner(resolved)?;
    let row: Option<Json> = match kind.as_str() {
        "document" => sqlx::query_scalar(
            "SELECT jsonb_build_object('id',id,'node',node_id,'org',org_id,'seq',doc_seq)
            FROM documents WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?,
        "chat" => sqlx::query_scalar(
            "SELECT jsonb_build_object('id',id,'node',node_id,'org',org_id,'seq',next_seq)
            FROM chats WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?,
        "thread" => sqlx::query_scalar(
            "SELECT jsonb_build_object('id',node_id,'view',active_continuation_id,
            'revision',execution_revision,'seq',next_seq) FROM threads WHERE node_id=$1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?,
        _ => return Err(EngineError::ToolCallIdentity),
    };
    let identity = match resolved.target {
        Target::Conversation { thread } => json!(["conversation", thread]),
        Target::NodeChat { chat, node } => json!(["node_chat", chat, node]),
        Target::PeopleChat { chat } => json!(["people_chat", chat]),
        Target::Document { document } => json!(["document", document]),
        Target::Version { document, number } => {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_versions WHERE document_id=$1 AND version=$2)")
                .bind(document).bind(number).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
            json!(["version", document, number, exists])
        }
        Target::DocumentFile { document, file } => json!(["document_file", document, file]),
        Target::ChatFile {
            chat,
            node,
            message,
            carried,
        } => json!(["chat_file", chat, node, message, carried]),
        Target::Directory(_) => return Err(EngineError::ToolCallIdentity),
    };
    let attachment = if matches!(
        resolved.target,
        Target::DocumentFile { .. } | Target::ChatFile { .. }
    ) {
        match crate::tools::download::attachment_to_download(conn, resolved).await? {
            Ok(file) => Some(serde_json::to_value(file)?),
            Err(_) => return Ok(Json::Null),
        }
    } else {
        None
    };
    Ok(json!({"identity":identity,"row":row,"attachment":attachment}))
}

pub(crate) fn proof(
    call: &CallContext,
    thread: &threads::ThreadRow,
    resolved: &Resolved,
    expected: Json,
    to: String,
    blob: Hash,
    size: i64,
) -> Result<Proof, EngineError> {
    let (kind, object, node) = owner(resolved)?;
    Ok(Proof {
        thread: thread.node_id,
        asked: call.arguments["path"].as_str().unwrap_or_default().into(),
        workspace: thread.workspace_dir.clone(),
        expected,
        arguments: call.arguments.clone(),
        delivery: Delivery {
            blob,
            size,
            to: to.clone(),
            path: resolved.path.clone(),
            ready: resolved
                .annotate(json!({"status":"ready","path":to,"size":size,"received":size})),
            downloading: resolved
                .annotate(json!({"status":"downloading","path":to,"size":size,"received":0})),
            kind,
            object,
            node,
            root: resolved.asking.agent.root,
        },
    })
}

/// This callback runs only on the source-locked command admission transaction, after all store I/O.
pub(crate) async fn admit(
    engine: &Engine,
    conn: &mut Conn,
    body: &Json,
) -> Result<Option<Json>, EngineError> {
    let Some(scope) = DOWNLOAD.try_with(Arc::clone).ok() else {
        return Err(EngineError::ToolCallIdentity);
    };
    let fresh = match engine
        .resolve(conn, scope.proof.thread, &scope.proof.asked, true)
        .await?
    {
        Ok(fresh) => fresh,
        Err(_) => {
            return Err(refused(
                &scope,
                Refusal::new(
                    Code::NotFound,
                    "This shared entry is no longer available. Nothing was written.",
                ),
            ));
        }
    };
    if !crate::tools::download::may_take_away(&fresh) {
        return Err(refused(
            &scope,
            Refusal::new(
                Code::NotDownloadable,
                "This entry no longer grants the Read and Export access needed for this download. Nothing was written.",
            ),
        ));
    }
    let current = threads::thread(conn, scope.proof.thread)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let largest = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::STORAGE_MAX_BLOB_BYTES,
    )
    .await?
    .ok_or_else(|| EngineError::unreadable("The download size ceiling has no resolved value."))?;
    if stamp(conn, &fresh).await? != scope.proof.expected
        || current.workspace_dir != scope.proof.workspace
        || body["path"]
            != json!(crate::parcels::within(
                &current.workspace_dir,
                &scope.proof.delivery.to
            ))
        || body["blob"] != serde_json::to_value(scope.proof.delivery.blob)?
        || scope.proof.delivery.size > largest
    {
        return Err(refused(
            &scope,
            Refusal::new(
                Code::Changed,
                "The shared entry, its bytes or the destination changed during preparation. Nothing was written. Read it again.",
            ),
        ));
    }
    let mut delivery = scope.proof.delivery.clone();
    delivery.path = fresh.path.clone();
    let (kind, object, node) = owner(&fresh)?;
    delivery.kind = kind;
    delivery.object = object;
    delivery.node = node;
    delivery.root = fresh.asking.agent.root;
    delivery.ready = fresh.annotate(
        json!({"status":"ready","path":delivery.to,"size":delivery.size,"received":delivery.size}),
    );
    delivery.downloading = fresh.annotate(
        json!({"status":"downloading","path":delivery.to,"size":delivery.size,"received":i64::MAX}),
    );
    if !orch_agent_contract::tool_answers::fits(
        "download_shared",
        &scope.proof.arguments,
        &delivery.ready,
    ) || !orch_agent_contract::tool_answers::fits(
        "download_shared",
        &scope.proof.arguments,
        &delivery.downloading,
    ) {
        return Err(refused(
            &scope,
            Refusal::new(
                Code::TooLarge,
                "The complete current download outcome exceeds max_bytes. Nothing was written.",
            ),
        ));
    }
    delivery.downloading["received"] = json!(0);
    *scope
        .selected
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(delivery.clone());
    Ok(Some(serde_json::to_value(delivery)?))
}

/// Once-only factual audit retains the original admission's target owner and agent identity.
pub(crate) async fn audit(
    engine: &Engine,
    call: &CallContext,
    delivery: &Delivery,
) -> Result<(), EngineError> {
    use orch_access_contract::audit::AuditAction;
    let action = match delivery.kind.as_str() {
        "thread" => AuditAction::ThreadReadByAgent,
        "chat" => AuditAction::ChatReadByAgent,
        "document" => AuditAction::DocumentReadByAgent,
        _ => return Err(EngineError::ToolCallIdentity),
    };
    let mut tx = engine.db().begin().await?;
    let first = sqlx::query(
        "UPDATE thread_tool_calls SET plan=jsonb_set(plan,'{download_audited}','true')
        WHERE answer_id=$1 AND NOT (plan ? 'download_audited')",
    )
    .bind(crate::ids::tool_result(call.thread, &call.call_ref))
    .execute(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?
    .rows_affected()
        == 1;
    if first {
        for (action, summary, details) in [
            (
                action,
                "An agent read something to copy it into its workspace.",
                json!({"path":delivery.path,"downloaded":true}),
            ),
            (
                AuditAction::ShareFileDownloaded,
                "An agent copied something it was shared into its workspace.",
                json!({"path":delivery.path,"to":delivery.to,"bytes":delivery.size}),
            ),
        ] {
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::Agent(call.thread),
                action.as_str(),
                &delivery.kind,
                Some(delivery.object),
                summary,
            );
            entry.history = action.is_history_line();
            entry.node_id = delivery.node;
            entry.org_id = Some(delivery.root);
            entry.details = details;
            orch_db::audit::append(
                &mut tx,
                &entry,
                &orch_db::audit::Origin::default(),
                engine.inner.clock.now(),
            )
            .await?;
        }
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

pub(crate) async fn outcome(
    engine: &Engine,
    call: &CallContext,
    delivery: &Delivery,
    written: FileWritten,
) -> Result<Answer, EngineError> {
    let body=match written {
        FileWritten::Ready=>delivery.ready.clone(),
        FileWritten::Downloading {received}=>{let mut body=delivery.downloading.clone();body["received"]=json!(received);body},
        FileWritten::Differs=>return Ok(Refusal::new(Code::Exists,
            "The destination already contains different bytes. A download replaces nothing; choose another save_to." ).into()),
    };
    audit(engine, call, delivery).await?;
    Ok(Answer::Done(body))
}
