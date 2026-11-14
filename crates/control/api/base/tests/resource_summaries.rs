//! Native summary reads are free; every paragraph keeps all of its current source governors.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]
#[path = "resource_summaries/batch.rs"]
mod batch;
mod desk;
#[path = "resource_summaries/permission_rules.rs"]
mod permission_rules;
mod places;
mod rig;
mod support;
use desk::Desk;
use orch_api_contract::{
    common::{ErrorCode, ReceiptProjection},
    summaries::{
        SummaryGet, SummaryReadQuery, SummaryRefresh, SummaryRefreshBody, SummaryState,
        SummaryTarget,
    },
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::{Boundaries, follow};
use orch_db::queries::{
    access,
    identity::{self, NewApiToken},
    summaries as rows,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

async fn key(desk: &Desk, target: Id) -> String {
    let scope:orch_access_contract::key_scope::KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"topic","id":target,"reach":"self"},"resources":["topic"],"allow":[{"operation":"SummaryGet","actions":["read"]}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "Summary paragraph",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
async fn cache(
    desk: &Desk,
    target: SummaryTarget,
    viewer: Option<Id>,
    text: &str,
    refs: Json,
) -> Id {
    cache_in(&desk.world, target, viewer, text, refs).await
}
/// The same stored paragraph for a world that is not a desk, such as a scene with threads.
async fn cache_in(
    world: &support::World,
    target: SummaryTarget,
    viewer: Option<Id>,
    text: &str,
    refs: Json,
) -> Id {
    let mut conn = world.conn().await;
    let row = rows::ensure(&mut conn, target, viewer, world.now())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE resource_summaries SET text=$2,producer='{}',source_snapshot='{}',source_refs=$3,completed_request_id=$4,completed_at=$5,completed_dirty_revision=dirty_revision WHERE id=$1")
        .bind(row.id).bind(text).bind(refs).bind(new_id()).bind(world.now()).execute(&mut *conn).await.unwrap();
    row.id
}

#[tokio::test]
async fn a_future_summary_exposes_its_actual_wait_without_claiming_or_retrying_work() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Scheduled source").await.base.id;
    let target = SummaryTarget::Document { id: doc };
    let summary = cache(
        &desk,
        target,
        None,
        "Previous readable paragraph.",
        json!([target]),
    )
    .await;
    let now = desk.world.now();
    let due = now + chrono::Duration::minutes(15);
    // enqueue accepts only inside the caller's transaction.
    let mut tx = orch_db::Db::from_pool(desk.world.db.pool.clone())
        .begin()
        .await
        .unwrap();
    let result = rows::enqueue(
        &mut tx,
        summary,
        rows::Origin::Automatic {
            writer: desk.ada.id(),
        },
        &json!({"client_id":new_id(),"model":null,"range":null}),
        &json!({"target":target}),
        now,
        due,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let rows::Enqueued::New(pending) = result else {
        panic!("one native queued fixture");
    };
    for _ in 0..2 {
        let view = desk
            .world
            .app
            .call(SummaryGet, &[&"document", &doc])
            .token(&desk.ada.token)
            .query(&SummaryReadQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(view.state, SummaryState::Scheduled);
        assert_eq!(view.text.as_deref(), Some("Previous readable paragraph."));
        assert_eq!(view.poll_after_ms, Some(900_000));
        let wait = view.wait.unwrap();
        assert_eq!(
            wait.reason,
            orch_api_contract::summaries::SummaryWaitReason::Interval
        );
        assert_eq!(wait.until.map(|until| until.get()), Some(due));
    }
    let mut conn = desk.world.conn().await;
    let kept = rows::request(&mut conn, pending.id).await.unwrap().unwrap();
    assert_eq!(kept.state, "queued");
    assert_eq!(kept.due_at, due);
    assert!(kept.lease_token.is_none());
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM summary_calls")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(calls, 0);
}

#[tokio::test]
async fn opening_a_document_summary_creates_no_cache_job_or_model_input() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Written source").await.base.id;
    desk.paragraphs(&desk.ada, doc, &["The written text stays as written."])
        .await;
    for _ in 0..2 {
        let view = desk
            .world
            .app
            .call(SummaryGet, &[&"document", &doc])
            .token(&desk.ada.token)
            .query(&SummaryReadQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(view.target, SummaryTarget::Document { id: doc });
        assert_eq!(view.root_org_id, desk.org);
        assert_eq!(view.state, SummaryState::NoSummarizer);
        assert_eq!(view.text, None);
        assert_eq!(view.revision, 0);
    }
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM resource_summaries),(SELECT count(*) FROM resource_summary_requests),(SELECT count(*) FROM model_inputs)").fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(counts, (0, 0, 0));
    let refused = desk
        .world
        .app
        .call(SummaryRefresh, &[&"document", &doc])
        .token(&desk.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: None,
            range: None,
        })
        .send()
        .await;
    assert_eq!(refused.problem().code, ErrorCode::ProviderUnavailable);
    assert!(
        rows::get(
            &mut *desk.world.conn().await,
            SummaryTarget::Document { id: doc },
            Some(desk.ada.id())
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn a_plain_reader_with_model_use_can_refresh_without_configuring_automation() {
    use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let app = &scene.rig.world.app;
    let doc = app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Manual summary source".into(),
            blocks: None,
            markdown: Some("A current readable document.".into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,protocols=ARRAY['openai-responses'],upstreams=$2,key_machine_id=$3,status='ready' WHERE id=$1")
        .bind(scene.on.provider).bind(json!([{"protocol":"openai-responses","base_url":"https://fixture.example.invalid/v1","options":{"auth":"none"}}])).bind(scene.rig.machine_id).execute(scene.rig.db.pool()).await.unwrap();
    let unavailable = app
        .call(SummaryGet, &[&"document", &doc.base.id])
        .token(&scene.cai.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(unavailable.state, SummaryState::NoSummarizer);
    assert_eq!(
        unavailable.manual_problem,
        Some(orch_api_contract::summaries::SummaryManualProblem::NoModel)
    );
    scene
        .rig
        .lets_use(
            "provider",
            scene.on.provider,
            "provider.use",
            &scene.cai,
            json!({}),
        )
        .await;
    let view = app
        .call(SummaryGet, &[&"document", &doc.base.id])
        .token(&scene.cai.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(view.state, SummaryState::Empty);
    assert!(view.manual_problem.is_none());
    let nonce = new_id();
    let body = SummaryRefreshBody {
        client_id: nonce,
        model: None,
        range: None,
    };
    let first = app
        .call(SummaryRefresh, &[&"document", &doc.base.id])
        .token(&scene.cai.token)
        .body(&body)
        .send()
        .await
        .ok();
    let again = app
        .call(SummaryRefresh, &[&"document", &doc.base.id])
        .token(&scene.cai.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(first.id, again.id);
    let accepted = rows::request(&mut *scene.rig.world.conn().await, first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.requested_by, Some(scene.cai.id()));
    assert_eq!(accepted.paying_user, Some(scene.cai.id()));
    assert_eq!(
        accepted.accepted_source["model"]["provider_id"],
        json!(scene.on.provider)
    );
    assert_eq!(
        accepted.accepted_source["model"]["model"],
        json!(rig::MODEL)
    );
    let configured: i64 =
        sqlx::query_scalar("SELECT count(*) FROM settings WHERE key='auto.summarizer'")
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        configured, 0,
        "a direct paid act does not opt anyone into automation"
    );
    let automatic: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM resource_summary_requests WHERE origin='automatic'",
    )
    .fetch_one(scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(automatic, 0);
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$1::jsonb) WHERE provider_id=$2")
        .bind(json!({"input_per_mtok":"1","cached_input_per_mtok":"1","output_per_mtok":"2"})).bind(scene.on.provider).execute(scene.rig.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,warn_percent,created_by) VALUES($1,$2,'members',$3,'each','credits','week',0,ARRAY['summary'],50,$4)")
        .bind(new_id()).bind(scene.tree.topic.base.id).bind(scene.tree.org.base.id).bind(scene.ada.id()).execute(scene.rig.db.pool()).await.unwrap();
    let capped = app
        .call(SummaryGet, &[&"document", &doc.base.id])
        .token(&scene.cai.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        capped.manual_problem,
        Some(orch_api_contract::summaries::SummaryManualProblem::CapReached)
    );
    let blocked = app
        .call(SummaryRefresh, &[&"document", &doc.base.id])
        .token(&scene.cai.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: None,
            range: None,
        })
        .send()
        .await;
    assert_eq!(blocked.problem().code, ErrorCode::ProviderUnavailable);
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM resource_summary_requests),(SELECT count(*) FROM usage_events WHERE purpose='summary')").fetch_one(scene.rig.db.pool()).await.unwrap();
    assert_eq!(
        counts,
        (1, 0),
        "a refused default creates neither another request nor billed use"
    );
}

#[tokio::test]
async fn stored_chat_summaries_and_their_prompt_blobs_use_the_native_once_only_inspection_door() {
    use orch_api_contract::{
        chats::{OrgChatCreate, OrgChatCreateBody},
        model_inputs::{ModelInputListQuery, NodeModelInputBlobGet, NodeModelInputList},
    };
    use orch_db::queries::{chats, model_inputs};
    let desk = Desk::new().await;
    let first = desk.member("summary-chat-first", "viewer").await;
    let second = desk.member("summary-chat-second", "viewer").await;
    let chat = desk
        .world
        .app
        .call(OrgChatCreate, &[&desk.org])
        .token(&first.token)
        .body(&OrgChatCreateBody {
            visibility: None,
            home_id: None,
            user_ids: vec![second.id()],
            name: Some("A group the inspector has not joined".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    let target = SummaryTarget::ChatTitle { id: chat };
    cache(
        &desk,
        target,
        None,
        "An existing chat subject.",
        json!([target]),
    )
    .await;
    let blob = desk
        .world
        .app
        .app
        .blobs
        .put_bytes(bytes::Bytes::from_static(b"Full retained chat prompt"))
        .await
        .unwrap();
    let mut conn = desk.world.conn().await;
    let model_input = model_inputs::prepare(&mut conn, &model_inputs::NewModelInput {
        id: new_id(), node: desk.topic, segment: None, turn: None, input: None,
        purpose: "summary", reference: "retained-chat-summary",
        blocks: &json!([{"key":"summary","source":"summary","text":"Retained chat prompt", "blob":blob.hash,"total_len":25,"truncated":true}]), seq: 1,
    }).await.unwrap();
    sqlx::query("INSERT INTO resource_summary_model_inputs(model_input_id,viewer_id,source_refs) VALUES($1,NULL,$2)")
        .bind(model_input).bind(json!([target])).execute(&mut *conn).await.unwrap();
    let private_input = model_inputs::prepare(&mut conn, &model_inputs::NewModelInput {
        id: new_id(), node: desk.topic, segment: None, turn: None, input: None,
        purpose: "summary", reference: "another-persons-chat-summary",
        blocks: &json!([{"key":"summary","source":"summary","text":"A different person's private prompt", "blob":blob.hash,"total_len":25,"truncated":true}]), seq: 2,
    }).await.unwrap();
    sqlx::query("INSERT INTO resource_summary_model_inputs(model_input_id,viewer_id,source_refs) VALUES($1,$2,$3)")
        .bind(private_input).bind(first.id()).bind(json!([target])).execute(&mut *conn).await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(desk.ada.id()),
        orch_types::Time::new(desk.world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_engine::summaries::sources::viewable(&mut conn, &principal, target)
            .await
            .unwrap()
    );
    assert!(
        !orch_engine::summaries::sources::readable(&mut conn, &principal, target)
            .await
            .unwrap(),
        "passive inspection is never model source authority"
    );
    drop(conn);
    let opening = new_id().to_string();
    for _ in 0..3 {
        let reply = desk
            .world
            .app
            .call(SummaryGet, &[&"chat_title", &chat])
            .token(&desk.ada.token)
            .header("x-orch-opening", &opening)
            .send()
            .await
            .ok();
        assert_eq!(reply.text.as_deref(), Some("An existing chat subject."));
        assert!(reply.inspecting);
    }
    let page = desk
        .world
        .app
        .call(NodeModelInputList, &[&desk.topic])
        .token(&desk.ada.token)
        .header("x-orch-opening", &opening)
        .query(&ModelInputListQuery {
            limit: Some(1),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        page.items.iter().map(|row| row.id).collect::<Vec<_>>(),
        [model_input]
    );
    assert!(
        page.next_cursor.is_none(),
        "another person's private prompt is excluded before pagination"
    );
    let full = desk
        .world
        .app
        .call(
            NodeModelInputBlobGet,
            &[&desk.topic, &model_input, &blob.hash],
        )
        .token(&desk.ada.token)
        .header("x-orch-opening", &opening)
        .send_bytes()
        .await;
    assert_eq!(full.status, 200);
    assert_eq!(full.raw.as_ref(), b"Full retained chat prompt");
    let private_blob = desk
        .world
        .app
        .call(
            NodeModelInputBlobGet,
            &[&desk.topic, &private_input, &blob.hash],
        )
        .token(&desk.ada.token)
        .header("x-orch-opening", &opening)
        .send_bytes()
        .await;
    assert_eq!(
        private_blob.status, 404,
        "chat inspection does not give another person's private summary input"
    );
    let refused = desk
        .world
        .app
        .call(SummaryRefresh, &[&"chat_title", &chat])
        .token(&desk.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: None,
            range: None,
        })
        .send()
        .await;
    assert!(
        !refused.status.is_success(),
        "reading an existing subject cannot admit paid inspection work"
    );
    let mut conn = desk.world.conn().await;
    let traces: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='chat.inspected' AND object_id=$1 AND actor_id=$2")
        .bind(chat).bind(desk.ada.id()).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        traces, 1,
        "paragraph/list/blob share the actual logical chat opening"
    );
    assert!(
        chats::state(&mut conn, desk.ada.id(), chat)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !chats::members(&mut conn, chat)
            .await
            .unwrap()
            .iter()
            .any(|member| member.user_id == desk.ada.id())
    );
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(work, 0);
}

#[tokio::test]
async fn an_existing_paragraph_needs_every_current_source_and_scope_not_only_its_container() {
    let desk = Desk::new().await;
    let reader = desk.member("paragraph-reader", "viewer").await;
    let doc = desk.make(&desk.ada, "Private contributor").await.base.id;
    let topic = SummaryTarget::Topic { id: desk.topic };
    cache(
        &desk,
        topic,
        None,
        "A paragraph containing private document words.",
        json!([topic, SummaryTarget::Document { id: doc }]),
    )
    .await;
    let owner = desk
        .world
        .app
        .call(SummaryGet, &[&"topic", &desk.topic])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(owner.text.as_deref().unwrap().contains("private document"));
    let narrow = key(&desk, desk.topic).await;
    let scoped = desk
        .world
        .app
        .call(SummaryGet, &[&"topic", &desk.topic])
        .token(&narrow)
        .send()
        .await
        .ok();
    assert_eq!(
        scoped.text, None,
        "primary Read cannot borrow an omitted document Read facet"
    );
    access::testing::set_row(
        &mut *desk.world.conn().await,
        &access::PageKey::new("document", doc, "live"),
        "any",
        "doc.view",
        false,
        false,
    )
    .await
    .unwrap();
    let current = desk
        .world
        .app
        .call(SummaryGet, &[&"topic", &desk.topic])
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert_eq!(
        current.text, None,
        "changed ordinary document Read drops the whole opaque paragraph"
    );
    assert_eq!(current.producer, None);
    assert_eq!(current.updated_at, None);
    let raw = serde_json::to_value(current).unwrap().to_string();
    assert!(!raw.contains("Private contributor") && !raw.contains("private document words"));
}

#[tokio::test]
async fn one_readers_paragraph_never_becomes_another_readers_cache() {
    let desk = Desk::new().await;
    let reader = desk.member("second-paragraph-reader", "viewer").await;
    let doc = desk.make(&desk.ada, "Shared document").await.base.id;
    let target = SummaryTarget::Document { id: doc };
    cache(
        &desk,
        target,
        None,
        "Shared automatic paragraph.",
        json!([target]),
    )
    .await;
    cache(
        &desk,
        target,
        Some(desk.ada.id()),
        "First reader's explicit paragraph.",
        json!([target]),
    )
    .await;
    let first = desk
        .world
        .app
        .call(SummaryGet, &[&"document", &doc])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let second = desk
        .world
        .app
        .call(SummaryGet, &[&"document", &doc])
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert_eq!(
        first.text.as_deref(),
        Some("First reader's explicit paragraph.")
    );
    assert_eq!(second.text.as_deref(), Some("Shared automatic paragraph."));
    assert!(
        rows::get(&mut *desk.world.conn().await, target, Some(reader.id()))
            .await
            .unwrap()
            .is_none(),
        "reading the shared fallback makes no personal row"
    );
}

#[tokio::test]
async fn a_document_id_does_not_masquerade_as_a_thread_and_ranges_stay_native() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Native document").await.base.id;
    assert_eq!(
        desk.world
            .app
            .call(SummaryGet, &[&"thread", &doc])
            .token(&desk.ada.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let invalid = SummaryRefreshBody {
        client_id: new_id(),
        model: None,
        range: Some(orch_api_contract::summaries::SummaryChatRange {
            after: 0,
            through: 1,
        }),
    };
    assert_eq!(
        desk.world
            .app
            .call(SummaryRefresh, &[&"document", &doc])
            .token(&desk.ada.token)
            .body(&invalid)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let receipt = serde_json::to_value(orch_api_contract::summaries::SummaryReceipt {
        projection: ReceiptProjection::Receipt,
        id: new_id(),
        target: SummaryTarget::Document { id: doc },
        revision: 1,
    })
    .unwrap();
    assert_eq!(receipt.as_object().unwrap().len(), 4);
    assert!(receipt.get("text").is_none() && receipt.get("producer").is_none());
}

#[tokio::test]
async fn a_real_document_scoped_refresh_retains_its_original_http_token_and_closes_on_revocation() {
    use orch_api_contract::{
        documents::{NodeDocumentCreate, NodeDocumentCreateBody},
        summaries::SummaryModelChoice,
    };
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let app = &scene.rig.world.app;
    let document = app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "A document-scoped refresh".into(),
            blocks: None,
            markdown: Some("Only this document is selected.".into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,protocols=ARRAY['openai-responses'],upstreams=$2,key_machine_id=$3,status='ready' WHERE id=$1")
        .bind(scene.on.provider).bind(json!([{"protocol":"openai-responses","base_url":"https://fixture.example.invalid/v1","options":{"auth":"none"}}])).bind(scene.rig.machine_id).execute(scene.rig.db.pool()).await.unwrap();
    let scope:orch_access_contract::key_scope::KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"document","id":document.base.id,"reach":"self"},"resources":["document"],"allow":[{"operation":"SummaryRefresh","actions":["read","write"]}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let token = new_id();
    identity::create_api_token_scoped(
        &mut *scene.rig.world.conn().await,
        &NewApiToken {
            id: token,
            user_id: scene.ada.id(),
            name: "One document summary act",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: scene.rig.world.now(),
            expires_at: Some(scene.rig.world.now() + chrono::Duration::minutes(5)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    let receipt = app
        .call(SummaryRefresh, &[&"document", &document.base.id])
        .token(&secret)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(SummaryModelChoice {
                provider_id: scene.on.provider,
                model: rig::MODEL.into(),
                params: None,
            }),
            range: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(receipt.projection, ReceiptProjection::Receipt);
    assert_eq!(
        receipt.target,
        SummaryTarget::Document {
            id: document.base.id
        }
    );
    let request = rows::request(&mut *scene.rig.world.conn().await, receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.requested_by, Some(scene.ada.id()));
    assert_eq!(request.paying_user, Some(scene.ada.id()));
    let http: orch_engine::operation_scope::http_origin::HttpOrigin =
        serde_json::from_value(request.accepted_source["http"].clone()).unwrap();
    assert_eq!(http.origin().operation(), "SummaryRefresh");
    let mut tx = scene.rig.db.begin().await.unwrap();
    assert!(http.current(&mut tx, app.app.clock.as_ref()).await.is_ok());
    tx.commit().await.unwrap();
    assert!(
        identity::revoke_api_token(
            &mut *scene.rig.world.conn().await,
            token,
            scene.ada.id(),
            scene.rig.world.now()
        )
        .await
        .unwrap()
    );
    let mut tx = scene.rig.db.begin().await.unwrap();
    assert!(
        http.current(&mut tx, app.app.clock.as_ref()).await.is_err(),
        "an admitted document act cannot regain a Session after its actual bearer closed"
    );
    tx.commit().await.unwrap();
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM model_inputs),(SELECT count(*) FROM usage_events WHERE purpose='summary')").fetch_one(scene.rig.db.pool()).await.unwrap();
    assert_eq!(
        counts,
        (0, 0),
        "acceptance and raw identity checks make no model request"
    );
}

#[tokio::test]
async fn a_failed_refresh_says_what_stopped_it_and_that_the_previous_paragraph_is_kept() {
    use orch_api_contract::{
        documents::{NodeDocumentCreate, NodeDocumentCreateBody},
        summaries::SummaryModelChoice,
    };
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let app = &scene.rig.world.app;
    let document = app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "A refresh that fails".into(),
            blocks: None,
            markdown: Some("Text that was summarized before.".into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,protocols=ARRAY['openai-responses'],upstreams=$2,key_machine_id=$3,status='ready' WHERE id=$1")
        .bind(scene.on.provider).bind(json!([{"protocol":"openai-responses","base_url":"https://fixture.example.invalid/v1","options":{"auth":"none"}}])).bind(scene.rig.machine_id).execute(scene.rig.db.pool()).await.unwrap();
    let target = SummaryTarget::Document {
        id: document.base.id,
    };
    let receipt = app
        .call(SummaryRefresh, &[&"document", &document.base.id])
        .token(&scene.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(SummaryModelChoice {
                provider_id: scene.on.provider,
                model: rig::MODEL.into(),
                params: None,
            }),
            range: None,
        })
        .send()
        .await
        .ok();
    cache_in(
        &scene.rig.world,
        target,
        Some(scene.ada.id()),
        "An earlier paragraph.",
        json!([target]),
    )
    .await;
    // As the engine leaves a request that could not run: failed, with the reason it stored.
    sqlx::query("UPDATE resource_summary_requests SET state='failed',problem=$2,finished_at=now(),lease_token=NULL,lease_until=NULL WHERE id=$1")
        .bind(receipt.id)
        .bind(json!({"code":"summary_unavailable","message":"The machine that keeps the key of the provider is offline. Bring it online or move the key.","reason":"keeper_offline"}))
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    let view = app
        .call(SummaryGet, &[&"document", &document.base.id])
        .token(&scene.ada.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(view.state, SummaryState::Failed);
    assert_eq!(view.text.as_deref(), Some("An earlier paragraph."));
    let problem = view.problem.expect("a failed refresh carries its problem");
    assert_eq!(
        problem.extra.get("reason"),
        Some(&json!("keeper_offline")),
        "the page is told which thing stopped it, not only that something did"
    );
    assert!(
        problem.detail.starts_with("The machine that keeps the key")
            && problem.detail.ends_with("The previous paragraph is kept."),
        "{}",
        problem.detail
    );
}

/// A request of the summary that waits for its turn, as the sweep leaves a queued one.
async fn queued_request(desk: &Desk, summary: Id, target: SummaryTarget) -> Id {
    let now = desk.world.now();
    let mut tx = orch_db::Db::from_pool(desk.world.db.pool.clone())
        .begin()
        .await
        .unwrap();
    let result = rows::enqueue(
        &mut tx,
        summary,
        rows::Origin::Automatic {
            writer: desk.ada.id(),
        },
        &json!({"client_id":new_id(),"model":null,"range":null}),
        &json!({"target":target}),
        now,
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let rows::Enqueued::New(pending) = result else {
        panic!("one native queued fixture");
    };
    pending.id
}

/// Runs `moving` once, at the place where the read of `route` is about to be checked again after its
/// read: what the person sees is then answered while something about it moves, and always at that moment.
fn when_the_read_is_checked_again<Fut>(
    log: &Boundaries,
    route: &'static str,
    phase: &'static str,
    moving: impl Fn() -> Fut + Send + Sync + 'static,
) -> Arc<AtomicBool>
where
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let moved = Arc::new(AtomicBool::new(false));
    let seen = moved.clone();
    log.hold(move |operation, at| {
        let work =
            (operation == route && at == phase && !seen.swap(true, Ordering::SeqCst)).then(&moving);
        Box::pin(async move {
            if let Some(work) = work {
                work.await;
            }
        })
    });
    moved
}

/// The queued request is taken up by the sweep.
async fn take_up(pool: sqlx::PgPool, request: Id) {
    sqlx::query("UPDATE resource_summary_requests SET state='running',lease_token=$2,lease_until=now()+interval '5 minutes' WHERE id=$1")
        .bind(request)
        .bind(new_id())
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_read_during_which_the_summarys_request_is_taken_up_is_answered_with_the_newer_state_not_refused()
 {
    let desk = Desk::new().await;
    let doc = desk
        .make(&desk.ada, "A summary that is being written")
        .await
        .base
        .id;
    let target = SummaryTarget::Document { id: doc };
    let summary = cache(
        &desk,
        target,
        None,
        "The earlier paragraph.",
        json!([target]),
    )
    .await;
    let request = queued_request(&desk, summary, target).await;
    let waiting = desk
        .world
        .app
        .call(SummaryGet, &[&"document", &doc])
        .token(&desk.ada.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        waiting.wait.map(|wait| wait.reason),
        Some(orch_api_contract::summaries::SummaryWaitReason::Queue),
        "a queued request is waiting for its turn"
    );
    let log = Boundaries::new();
    let pool = desk.world.db.pool.clone();
    let moved = when_the_read_is_checked_again(&log, "SummaryGet", "selected.settle", move || {
        take_up(pool.clone(), request)
    });
    let view = follow(log, async {
        desk.world
            .app
            .call(SummaryGet, &[&"document", &doc])
            .token(&desk.ada.token)
            .query(&SummaryReadQuery::default())
            .send()
            .await
    })
    .await
    .ok();
    assert!(
        moved.load(Ordering::SeqCst),
        "the request was taken up between the read and its check"
    );
    assert_eq!(view.state, SummaryState::Updating);
    assert_eq!(view.text.as_deref(), Some("The earlier paragraph."));
    assert!(
        view.wait.is_none(),
        "the answer is the newer state, in which the request is no longer waiting for its turn"
    );
}

#[tokio::test]
async fn a_read_whose_source_the_person_lost_between_the_read_and_its_check_is_refused() {
    let desk = Desk::new().await;
    let reader = desk.member("summary-reader-who-loses", "viewer").await;
    let doc = desk
        .make(&desk.ada, "A document that is withdrawn")
        .await
        .base
        .id;
    let target = SummaryTarget::Document { id: doc };
    cache(
        &desk,
        target,
        None,
        "A paragraph of a document that is withdrawn.",
        json!([target]),
    )
    .await;
    let before = desk
        .world
        .app
        .call(SummaryGet, &[&"document", &doc])
        .token(&reader.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert!(
        before.text.is_some(),
        "the reader may read it to begin with"
    );
    let log = Boundaries::new();
    let pool = desk.world.db.pool.clone();
    let moved = when_the_read_is_checked_again(&log, "SummaryGet", "selected.settle", move || {
        let pool = pool.clone();
        async move {
            let mut conn = pool.acquire().await.unwrap();
            access::testing::set_row(
                &mut conn,
                &access::PageKey::new("document", doc, "live"),
                "any",
                "doc.view",
                false,
                false,
            )
            .await
            .unwrap();
        }
    });
    let refused = follow(log, async {
        desk.world
            .app
            .call(SummaryGet, &[&"document", &doc])
            .token(&reader.token)
            .query(&SummaryReadQuery::default())
            .send()
            .await
    })
    .await;
    assert!(moved.load(Ordering::SeqCst));
    let problem = refused.problem();
    assert_eq!(
        problem.code,
        ErrorCode::NotFound,
        "a paragraph of what the person may no longer read is not given: {}",
        problem.detail
    );
}

#[tokio::test]
async fn the_first_read_of_a_chats_summary_is_answered_for_a_member_and_for_an_inspector() {
    use orch_api_contract::chats::{OrgChatCreate, OrgChatCreateBody};
    let desk = Desk::new().await;
    let first = desk.member("summary-chat-opener", "viewer").await;
    let second = desk.member("summary-chat-partner", "viewer").await;
    let chat = desk
        .world
        .app
        .call(OrgChatCreate, &[&desk.org])
        .token(&first.token)
        .body(&OrgChatCreateBody {
            visibility: None,
            home_id: None,
            user_ids: vec![second.id()],
            name: Some("A chat that is opened for the first time".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    let member = desk
        .world
        .app
        .call(SummaryGet, &[&"chat", &chat])
        .token(&first.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert!(!member.inspecting);
    // An administrator who is not in the chat has the opening written between the read and its check.
    let opening = new_id().to_string();
    for _ in 0..2 {
        let inspector = desk
            .world
            .app
            .call(SummaryGet, &[&"chat", &chat])
            .token(&desk.ada.token)
            .header("x-orch-opening", &opening)
            .query(&SummaryReadQuery::default())
            .send()
            .await
            .ok();
        assert!(inspector.inspecting);
    }
}

#[tokio::test]
async fn a_refresh_of_a_source_with_nothing_in_it_is_answered_at_once_and_makes_no_request() {
    use orch_api_contract::{
        documents::{NodeDocumentCreate, NodeDocumentCreateBody},
        summaries::SummaryModelChoice,
    };
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let app = &scene.rig.world.app;
    let document = app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Nothing written yet".into(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,protocols=ARRAY['openai-responses'],upstreams=$2,key_machine_id=$3,status='ready' WHERE id=$1")
        .bind(scene.on.provider).bind(json!([{"protocol":"openai-responses","base_url":"https://fixture.example.invalid/v1","options":{"auth":"none"}}])).bind(scene.rig.machine_id).execute(scene.rig.db.pool()).await.unwrap();
    let refused = app
        .call(SummaryRefresh, &[&"document", &document.base.id])
        .token(&scene.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(SummaryModelChoice {
                provider_id: scene.on.provider,
                model: rig::MODEL.into(),
                params: None,
            }),
            range: None,
        })
        .send()
        .await;
    assert_eq!(refused.status, http::StatusCode::CONFLICT);
    let problem = refused.problem();
    assert_eq!(
        problem.code,
        ErrorCode::SummaryNothingNew,
        "a source with nothing in it is no failed request but an answer: {}",
        problem.detail
    );
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM model_inputs)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(work, 0, "no request row, no model input and no use is made");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_unread_needs_no_model_but_an_explicit_nonempty_range_still_gets_its_summary() {
    use orch_api_contract::{
        chats::{
            ChatMessageCreate, ChatMessageCreateBody, ChatPart, OrgChatCreate, OrgChatCreateBody,
        },
        summaries::{SummaryChatRange, SummaryModelChoice},
    };
    use orch_link_contract::commands::{
        Command, CommandKind, CommandResult, ProviderCompleteResult,
    };
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let app = &scene.rig.world.app;
    let chat = app
        .call(OrgChatCreate, &[&scene.tree.org.base.id])
        .token(&scene.ada.token)
        .body(&OrgChatCreateBody {
            visibility: None,
            home_id: None,
            user_ids: vec![scene.dan.id()],
            name: Some("A deliberately selected range".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    let posted = app
        .call(ChatMessageCreate, &[&chat])
        .token(&scene.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Text {
                text: "The selected earlier message is still material.".into(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    orch_db::queries::chats::mark_read(
        &mut *scene.rig.world.conn().await,
        scene.ada.id(),
        chat,
        posted.seq,
    )
    .await
    .unwrap();
    let empty = app
        .call(SummaryRefresh, &[&"chat", &chat])
        .token(&scene.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: None,
            range: None,
        })
        .send()
        .await;
    assert_eq!(empty.status, http::StatusCode::CONFLICT);
    assert_eq!(
        empty.problem().code,
        ErrorCode::SummaryNothingNew,
        "an empty unread range needs neither model selection nor model permission"
    );
    let requests: i64 = sqlx::query_scalar("SELECT count(*) FROM resource_summary_requests")
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(requests, 0);
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,protocols=ARRAY['openai-responses'],upstreams=$2,key_machine_id=$3,status='ready' WHERE id=$1")
        .bind(scene.on.provider).bind(json!([{"protocol":"openai-responses","base_url":"https://fixture.example.invalid/v1","options":{"auth":"none"}}]))
        .bind(scene.rig.machine_id).execute(scene.rig.db.pool()).await.unwrap();
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::ProviderComplete, |command| {
            let Command::ProviderComplete(request) = command else {
                panic!("only the summary completion reaches this fixture")
            };
            assert!(request.input.contains("selected earlier message"));
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                text: "The selected range has a summary.".into(),
                usage: Default::default(),
                request_ref: None,
                timing: Default::default(),
            }))
        });
    let range = SummaryChatRange {
        after: 0,
        through: posted.seq,
    };
    let receipt = app
        .call(SummaryRefresh, &[&"chat", &chat])
        .token(&scene.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(SummaryModelChoice {
                provider_id: scene.on.provider,
                model: rig::MODEL.into(),
                params: None,
            }),
            range: Some(range),
        })
        .send()
        .await
        .ok();
    scene.rig.engine.summarize_resources().await.unwrap();
    rig::eventually(
        "the explicitly selected read range is summarized",
        || async {
            rows::request(&mut *scene.rig.world.conn().await, receipt.id)
                .await
                .unwrap()
                .unwrap()
                .state
                == "done"
        },
    )
    .await;
    let request = rows::request(&mut *scene.rig.world.conn().await, receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.accepted_source["range"], json!(range));
    assert_eq!(scene.rig.commands.issued_of("provider.complete"), 1);
}
