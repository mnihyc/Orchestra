//! Direct summary requests keep current native source Read before any refresh work.
use super::*;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::summaries::SummaryModelChoice;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn state(scene: &rig::Scene, document: Id) -> Result<Json, sqlx::Error> {
    sqlx::query_scalar("SELECT jsonb_build_object('document',(SELECT to_jsonb(d) FROM documents d WHERE id=$1),'summaries',(SELECT count(*) FROM resource_summaries),'requests',(SELECT count(*) FROM resource_summary_requests),'inputs',(SELECT count(*) FROM model_inputs),'usage',(SELECT count(*) FROM usage_events))")
        .bind(document).fetch_one(scene.rig.db.pool()).await
}

#[tokio::test]
async fn summary_read_and_refresh_require_native_document_read_before_any_work() -> Proof {
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let app = &scene.rig.world.app;
    let document = app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Current source permissions".into(),
            blocks: None,
            markdown: Some("The actual source stays unchanged by a refused refresh.".into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "the document fixture has no full reply")?;
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,protocols=ARRAY['openai-responses'],upstreams=$2,key_machine_id=$3,status='ready' WHERE id=$1")
        .bind(scene.on.provider).bind(json!([{"protocol":"openai-responses","base_url":"https://fixture.example.invalid/v1","options":{"auth":"none"}}]))
        .bind(scene.rig.machine_id).execute(scene.rig.db.pool()).await?;
    let outsider = places::person(&scene.rig.world, "summary-nonmember").await;
    let body = SummaryRefreshBody {
        client_id: new_id(),
        model: Some(SummaryModelChoice {
            provider_id: scene.on.provider,
            model: rig::MODEL.into(),
            params: None,
        }),
        range: None,
    };
    let before = state(&scene, document.base.id).await?;
    for refused in [
        app.call(SummaryGet, &[&"document", &document.base.id])
            .token(&outsider.token)
            .send_bytes()
            .await,
        app.call(SummaryRefresh, &[&"document", &document.base.id])
            .token(&outsider.token)
            .body(&body)
            .send_bytes()
            .await,
    ] {
        assert_eq!(refused.status, http::StatusCode::NOT_FOUND);
        assert_eq!(refused.problem().code, ErrorCode::NotFound);
        assert!(!String::from_utf8_lossy(&refused.raw).contains("actual source"));
        assert_eq!(state(&scene, document.base.id).await?, before);
    }
    let shown = app
        .call(SummaryGet, &[&"document", &document.base.id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        shown.target,
        SummaryTarget::Document {
            id: document.base.id
        }
    );
    assert_eq!(
        state(&scene, document.base.id).await?,
        before,
        "a permitted read starts no refresh"
    );
    let accepted = app
        .call(SummaryRefresh, &[&"document", &document.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(accepted.status, http::StatusCode::ACCEPTED);
    let accepted: orch_api_contract::summaries::SummaryReceipt =
        serde_json::from_slice(&accepted.raw)?;
    assert_eq!(accepted.projection, ReceiptProjection::Receipt);
    assert_eq!(
        accepted.target,
        SummaryTarget::Document {
            id: document.base.id
        }
    );
    let mut conn = scene.rig.world.conn().await;
    let request = rows::request(&mut conn, accepted.id)
        .await?
        .ok_or("the accepted refresh has no native request")?;
    assert_eq!(request.requested_by, Some(scene.ada.id()));
    drop(conn);
    let after = state(&scene, document.base.id).await?;
    assert_eq!(after["document"], before["document"]);
    assert_eq!(
        after["requests"].as_i64(),
        before["requests"].as_i64().map(|n| n + 1)
    );
    assert_eq!(after["inputs"], before["inputs"]);
    assert_eq!(after["usage"], before["usage"]);
    Ok(())
}
