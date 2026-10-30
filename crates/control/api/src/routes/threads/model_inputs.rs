//! What Orch supplied to a model for a node, as it was sent: the records the engine writes before each sending
//! ([`orch_engine::model_inputs`]), the newest first, a page at a time.
//!
//! Reading takes what reading the node takes (a thread's view, which for a side thread follows its parent), which
//! the access engine decides before the handler runs; it takes no managing, and it changes nothing: no read mark, no
//! audit row, no notification. A record's text is whole: what the model was given, block by block, never cut.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_api_contract::common::Page;
use orch_api_contract::model_inputs::{
    ModelInput, ModelInputListQuery, ModelTextBlock, NodeModelInputBlobGet, NodeModelInputList,
};
use orch_api_contract::threads::OutputRef;
use orch_db::queries::model_inputs::{self as rows, ModelInputFilter, ModelInputRow};
use orch_types::Json;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// How many records a page holds where the caller does not say: the ordinary bound of a list.
pub const PAGE_DEFAULT: i64 = 50;
/// The most a page holds.
pub const PAGE_MAX: i64 = 200;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(NodeModelInputList, list);
    registry.on_raw(NodeModelInputBlobGet, blob);
}

async fn blob(ctx: Ctx, (): (), _: axum::body::Body) -> ApiResult<axum::response::Response> {
    let node = ctx.id("id")?;
    let record = ctx.id("model_input_id")?;
    let hash: orch_types::Hash = ctx
        .param("hash")?
        .parse()
        .map_err(|_| ApiError::not_found())?;
    let mut conn = ctx.app.db.acquire().await?;
    let held:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_inputs m CROSS JOIN LATERAL jsonb_array_elements(m.blocks) b WHERE m.node_id=$1 AND m.id=$2 AND b->>'blob'=$3)")
        .bind(node).bind(record).bind(hash.to_string()).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if !held {
        return Err(ApiError::not_found());
    }
    drop(conn);
    let response = crate::routes::workspace::files::read_for(
        &ctx,
        &hash,
        "model-input.txt",
        "text/plain; charset=utf-8",
        false,
    )
    .await?;
    // Opening content holds no authority lock; the response is released only under current access.
    let (mut conn, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let held: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_inputs m CROSS JOIN LATERAL jsonb_array_elements(m.blocks) b WHERE m.node_id=$1 AND m.id=$2 AND b->>'blob'=$3)")
        .bind(node).bind(record).bind(hash.to_string()).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if !held {
        return Err(ApiError::not_found());
    }
    if let Some(document) = rows::input_document(&mut conn, record).await? {
        let live = orch_db::queries::documents::get(&mut conn, document)
            .await?
            .is_some_and(|row| row.deleted_at.is_none());
        if !live {
            return Err(ApiError::not_found());
        }
        let allowed = orch_authz::documents::check(
            &mut conn,
            guard.principal(),
            document,
            orch_access_contract::permissions::Permission::DocView,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => ApiError::not_found(),
            other => ApiError::internal(other),
        })?
        .is_allowed();
        if !allowed {
            return Err(ApiError::not_found());
        }
    }
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(response)
}

/// A page of the records of a node, the newest first: one by its identifier, those of one turn, those before or
/// after a sequence. `next_cursor` is the `before` of the page after this one.
async fn list(ctx: Ctx, query: ModelInputListQuery, (): ()) -> ApiResult<Page<ModelInput>> {
    let node = ctx.id("id")?;
    let limit = match query.limit {
        None => PAGE_DEFAULT,
        Some(limit) if limit < 1 => return Err(ApiError::invalid("limit", "must be at least 1")),
        Some(limit) => limit.min(PAGE_MAX),
    };
    let (mut conn, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let documents = rows::input_documents(&mut conn, node).await?;
    let readable: Vec<orch_types::Id> = orch_authz::documents::listed(
        &mut conn,
        guard.principal(),
        &documents,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .filter(|row| {
        row.holds(orch_access_contract::permissions::Permission::DocView) && !row.document.deleted
    })
    .map(|row| row.document.id)
    .collect();
    // One more than the page, to know whether another follows.
    let mut found = rows::list_for_documents(
        &mut conn,
        node,
        &ModelInputFilter {
            id: query.id,
            turn: query.turn_id,
            before: query.before,
            after: query.after,
            limit: limit + 1,
        },
        &readable,
    )
    .await?;
    let page = usize::try_from(limit).unwrap_or(usize::MAX);
    let more = found.len() > page;
    found.truncate(page);
    let next_cursor = if more {
        found.last().map(|row| row.seq_created.to_string())
    } else {
        None
    };
    let items = found
        .into_iter()
        .map(record)
        .collect::<ApiResult<Vec<ModelInput>>>()?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Page { items, next_cursor })
}

/// A block as the record keeps it.
#[derive(Debug, Deserialize)]
struct KeptBlock {
    key: String,
    source: String,
    text: String,
    #[serde(default)]
    blob: Option<orch_types::Hash>,
    #[serde(default)]
    total_len: Option<i64>,
    #[serde(default)]
    truncated: bool,
}

/// A record as the wire carries it.
fn record(row: ModelInputRow) -> ApiResult<ModelInput> {
    let blocks: Vec<KeptBlock> = serde_json::from_value(row.blocks).map_err(ApiError::internal)?;
    Ok(ModelInput {
        id: row.id,
        node_id: row.node_id,
        segment_id: row.segment_id,
        turn_id: row.turn_id,
        input_id: row.input_id,
        item_id: row.item_id,
        purpose: word(&row.purpose)?,
        reference: row.reference,
        blocks: blocks
            .into_iter()
            .map(|block| ModelTextBlock {
                key: block.key,
                source: block.source,
                text: if let Some(blob) = block.blob {
                    OutputRef {
                        inline: block.text,
                        total_len: block.total_len.unwrap_or_default(),
                        truncated: block.truncated,
                        blob: Some(blob),
                    }
                } else {
                    whole(block.text)
                },
            })
            .collect(),
        state: word(&row.state)?,
        created_at: row.created_at.into(),
        sent_at: row.sent_at.map(Into::into),
        seq_created: row.seq_created,
        seq_updated: row.seq_updated,
    })
}

/// A stored word as the value it names.
fn word<T: DeserializeOwned>(word: &str) -> ApiResult<T> {
    serde_json::from_value(Json::String(word.to_owned())).map_err(ApiError::internal)
}

/// A text whole, inline.
fn whole(text: String) -> OutputRef {
    OutputRef {
        total_len: i64::try_from(text.len()).unwrap_or(i64::MAX),
        inline: text,
        truncated: false,
        blob: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::model_inputs::{ModelInputPurpose, ModelInputState};

    #[test]
    fn a_record_is_carried_whole_with_its_words_named() {
        let long = "x".repeat(40_000);
        let row = ModelInputRow {
            id: orch_types::new_id(),
            node_id: orch_types::new_id(),
            segment_id: None,
            turn_id: None,
            input_id: None,
            item_id: None,
            purpose: "session".into(),
            reference: "opening".into(),
            blocks: serde_json::json!([
                { "key": "preamble", "source": "orch", "text": "You work inside Orch." },
                { "key": "standing_heading", "source": "standing", "text": long },
            ]),
            state: "uncertain".into(),
            created_at: chrono::Utc::now(),
            sent_at: None,
            seq_created: 4,
            seq_updated: 5,
        };
        let carried = record(row).expect("a record");
        assert_eq!(carried.purpose, ModelInputPurpose::Session);
        assert_eq!(carried.state, ModelInputState::Uncertain);
        assert_eq!(carried.blocks.len(), 2);
        let standing = &carried.blocks[1];
        assert_eq!(standing.source, "standing");
        assert_eq!(standing.text.inline.len(), 40_000);
        assert_eq!(standing.text.total_len, 40_000);
        assert!(!standing.text.truncated);
    }

    #[tokio::test]
    async fn document_summary_inputs_and_their_blobs_need_the_documents_current_view_permission() {
        use crate::rig::Scene;
        use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
        use orch_db::queries::model_inputs::NewModelInput;
        use serde_json::json;
        let scene = Scene::new().await;
        let app = &scene.rig.world.app;
        let node = scene.tree.topic.base.id;
        let doc = app
            .call(NodeDocumentCreate, &[&node])
            .token(&scene.ada.token)
            .body(&NodeDocumentCreateBody {
                title: "Private material".into(),
                blocks: None,
                markdown: Some("Secret document text".into()),
                access: None,
            })
            .send()
            .await
            .ok();
        let meta = app
            .app
            .blobs
            .put_bytes(bytes::Bytes::from_static(b"Secret full model input"))
            .await
            .unwrap();
        let mut conn = scene.rig.db.acquire().await.unwrap();
        orch_db::queries::access::testing::set_row(
            &mut conn,
            &orch_db::queries::access::PageKey::new("document", doc.base.id, "live"),
            "any",
            "doc.view",
            false,
            false,
        )
        .await
        .unwrap();
        let normal = rows::prepare(&mut conn,&NewModelInput { id:orch_types::new_id(),node,segment:None,turn:None,input:None,purpose:"summary",reference:"node-summary",blocks:&json!([{ "key":"summary_input","source":"summary","text":"Public node status" }]),seq:1 }).await.unwrap();
        let private = rows::prepare(&mut conn,&NewModelInput { id:orch_types::new_id(),node,segment:None,turn:None,input:None,purpose:"summary",reference:"private-document-summary",blocks:&json!([{ "key":"summary_input","source":"summary","text":"Secret document tail","blob":meta.hash,"total_len":23,"truncated":true }]),seq:2 }).await.unwrap();
        sqlx::query("INSERT INTO document_model_inputs(model_input_id,document_id) VALUES($1,$2)")
            .bind(private)
            .bind(doc.base.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let page = app
            .call(NodeModelInputList, &[&node])
            .token(&scene.cai.token)
            .query(&ModelInputListQuery {
                limit: Some(1),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(
            page.items.iter().map(|row| row.id).collect::<Vec<_>>(),
            [normal]
        );
        assert_eq!(
            page.next_cursor, None,
            "private records are filtered before pagination"
        );
        let exact = app
            .call(NodeModelInputList, &[&node])
            .token(&scene.cai.token)
            .query(&ModelInputListQuery {
                id: Some(private),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(exact.items.is_empty());
        let owner = app
            .call(NodeModelInputList, &[&node])
            .token(&scene.ada.token)
            .send()
            .await
            .ok();
        assert_eq!(
            owner.items.iter().map(|row| row.id).collect::<Vec<_>>(),
            [private, normal]
        );
        let refused = app
            .call(NodeModelInputBlobGet, &[&node, &private, &meta.hash])
            .token(&scene.cai.token)
            .send_bytes()
            .await;
        assert_eq!(refused.status, 404);
        assert!(!String::from_utf8_lossy(&refused.raw).contains("Secret"));
        let full = app
            .call(NodeModelInputBlobGet, &[&node, &private, &meta.hash])
            .token(&scene.ada.token)
            .send_bytes()
            .await;
        assert_eq!(full.status, 200);
        assert_eq!(
            full.raw,
            bytes::Bytes::from_static(b"Secret full model input")
        );
        sqlx::query("UPDATE documents SET deleted_at=$2 WHERE id=$1")
            .bind(doc.base.id)
            .bind(app.app.clock.now())
            .execute(scene.rig.db.pool())
            .await
            .unwrap();
        let gone = app
            .call(NodeModelInputList, &[&node])
            .token(&scene.ada.token)
            .query(&ModelInputListQuery {
                id: Some(private),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(
            gone.items.is_empty(),
            "a removed document never turns its prompt into an ordinary node record"
        );
        let gone_blob = app
            .call(NodeModelInputBlobGet, &[&node, &private, &meta.hash])
            .token(&scene.ada.token)
            .send_bytes()
            .await;
        assert_eq!(gone_blob.status, 404);
    }
}
