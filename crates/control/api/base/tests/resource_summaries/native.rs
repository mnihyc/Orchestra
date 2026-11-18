//! Real accepted summaries and serialized native commands share the production first-wire gate.
use super::*;
use futures::future::BoxFuture;
use orch_api_base::testing::admit_stored_command;
use orch_api_contract::{
    documents::{
        BlockContent, DocumentOp, DocumentOpsCreate, DocumentOpsCreateBody, Inline,
        NodeDocumentCreate, NodeDocumentCreateBody,
    },
    settings::{NodeSettingSet, NodeSettingSetBody},
    summaries::SummaryModelChoice,
    threads::{ThreadItemList, ThreadItemListQuery},
};
use orch_api_core::dispatch::StoredCommand;
use orch_engine::commands::Outgoing;
use orch_link_contract::{
    commands::{AgentStartResult, CommandKind, CommandResult, TurnStart},
    continuations::TurnStartFenced,
    events::{Event, TurnOutcome, UsageUpdated},
};
use std::sync::Arc;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn scene() -> (Arc<rig::Scene>, Id, SummaryModelChoice) {
    let scene = Arc::new(rig::Scene::new().await);
    let mut declared = rig::descriptor();
    declared["side_thread_modes"] = json!(["read_only", "no_tools"]);
    declared["x_capability_program"] = json!({"title":"Fixture text program","access":{"native":{"orch_tools":true,"native":{
        "generation":{"enabled":true,"modes":["native"],"default_mode":"native"},
        "summary":{"enabled":true,"modes":["native"],"default_mode":"native","delegated_mode":"native",
            "execution":[{"input":"summary.request","result":"summary.result","confined_tools":true}]}
    }}}});
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(scene.on.agent)
        .bind(&declared)
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::AgentStart, move |_| {
            Ok(CommandResult::AgentStart(AgentStartResult {
                reused: true,
                harness_version: "1.0.0".into(),
                descriptor: serde_json::from_value(declared.clone()).unwrap(),
            }))
        });
    scene
        .rig
        .world
        .app
        .call(
            NodeSettingSet,
            &[&scene.tree.org.base.id, &"providers.subscription_sharing"],
        )
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(["fake"]),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    scene
        .rig
        .lets_use(
            "provider",
            scene.on.provider,
            "provider.use",
            &scene.ada,
            json!({"models":[rig::MODEL],"arrangements":["separated"]}),
        )
        .await;
    let mut conn = scene.rig.db.acquire().await.unwrap();
    let page = access::PageKey::new("provider", scene.on.provider, "live");
    for owner in access::owner_entries(&mut conn, std::slice::from_ref(&page))
        .await
        .unwrap()
    {
        if owner.holder_id == Some(scene.ada.id()) {
            assert!(
                access::remove_entry(&mut conn, owner.id, scene.ada.id(), scene.rig.world.now())
                    .await
                    .unwrap()
            );
        }
    }
    access::testing::add_entry(
        &mut conn,
        &page,
        "any",
        "access.own",
        ("user", Some(scene.bea.id()), None),
        None,
        scene.rig.world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE providers SET owner_id=$2 WHERE id=$1")
        .bind(scene.on.provider)
        .bind(scene.bea.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    access::bump_epoch(&mut conn).await.unwrap();
    drop(conn);
    let choice = SummaryModelChoice {
        backend: serde_json::from_value(json!({
            "route":"summary.delegation", "offering":{"provider":scene.on.provider,"offering":rig::MODEL},
            "config":{"environment_id":scene.on.environment,"helper_topic_id":scene.tree.topic.base.id},
            "x_choice":{"revision":"personal"}
        })).unwrap(),
        params: None,
    };
    scene
        .rig
        .world
        .app
        .call(
            NodeSettingSet,
            &[&scene.tree.topic.base.id, &"auto.summarizer"],
        )
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            value: json!({"kind":"model","backend":choice.backend}),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let document = scene
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Native summary source".into(),
            markdown: Some("Original source material.".into()),
            blocks: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    cache_in(
        &scene.rig.world,
        SummaryTarget::Document {
            id: document.base.id,
        },
        Some(scene.ada.id()),
        "Previously approved paragraph.",
        json!([{"kind":"document","id":document.base.id}]),
    )
    .await;
    (scene, document.base.id, choice)
}

fn hold_native(scene: &rig::Scene) -> BoxFuture<'static, Outgoing> {
    let mut plain = scene.rig.commands.stop_next_issue(CommandKind::TurnStart);
    let mut fenced = scene
        .rig
        .commands
        .stop_next_issue(CommandKind::TurnStartFenced);
    Box::pin(async move {
        tokio::select! { command = &mut plain => command.unwrap(), command = &mut fenced => command.unwrap() }
    })
}

fn turn(command: &Outgoing) -> TurnStart {
    match command.kind {
        CommandKind::TurnStart => serde_json::from_value(command.body.clone()).unwrap(),
        CommandKind::TurnStartFenced => {
            *serde_json::from_value::<TurnStartFenced>(command.body.clone())
                .unwrap()
                .turn
        }
        _ => panic!("only the actual native turn is held"),
    }
}

async fn queue(scene: &rig::Scene, command: Outgoing) {
    scene
        .rig
        .world
        .app
        .app
        .link
        .dispatcher()
        .issue_stored(StoredCommand {
            id: Some(command.id),
            machine: command.machine,
            kind: command.kind.as_str().into(),
            key: command.key.wire(),
            deadline: command.deadline,
            body: command.body,
            authority: command.authority,
        })
        .await
        .unwrap();
}

async fn change_document(scene: &rig::Scene, document: Id) {
    let block = orch_db::queries::documents::live_blocks(
        &mut scene.rig.db.acquire().await.unwrap(),
        document,
    )
    .await
    .unwrap()
    .into_iter()
    .next()
    .unwrap();
    let edit = scene
        .rig
        .world
        .app
        .call(DocumentOpsCreate, &[&document])
        .token(&scene.ada.token)
        .body(&DocumentOpsCreateBody {
            expected_seq: None,
            client_id: new_id().to_string(),
            ops: vec![DocumentOp::Update {
                id: block.id,
                base_seq: block.seq_updated,
                block_type: None,
                props: None,
                content: Some(BlockContent::Inline(vec![Inline::Text {
                    text: "Fresh source material.".into(),
                    marks: None,
                    href: None,
                    color: None,
                    background: None,
                }])),
            }],
        });
    edit.send().await.ok();
}

async fn request_frame(scene: &rig::Scene, request: Id) -> Json {
    sqlx::query_scalar("SELECT to_jsonb(r) FROM resource_summary_requests r WHERE id=$1")
        .bind(request)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap()
}

async fn run(changed_before_admission: bool) -> Proof {
    let (scene, document, choice) = scene().await;
    let pending = hold_native(&scene);
    let receipt = scene
        .rig
        .world
        .app
        .call(SummaryRefresh, &[&"document", &document])
        .token(&scene.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(choice.clone()),
            range: None,
        })
        .send()
        .await
        .ok();
    let original = tokio::time::timeout(rig::PATIENCE, pending).await?;
    let original_id = original.id;
    let original_turn = turn(&original);
    assert!(
        request_frame(&scene, receipt.id)
            .await
            .get("model_admitted_at")
            .is_none_or(Json::is_null),
        "a prepared helper is not a model admission"
    );
    assert!(
        !serde_json::to_string(&original.body)?.contains(scene.ada.token.as_str()),
        "the native program receives no acting credential"
    );
    let next = if changed_before_admission {
        Some(scene.rig.commands.stop_next_issue(original.kind))
    } else {
        None
    };
    queue(&scene, original).await;
    let start = if let Some(next) = next {
        change_document(&scene, document).await;
        assert!(
            admit_stored_command(&scene.rig.world.app.app, original_id)
                .await
                .is_err(),
            "the queued old prompt cannot cross the real wire after its source changes"
        );
        let fresh = tokio::time::timeout(rig::PATIENCE, next).await??;
        let start = turn(&fresh);
        assert!(start.text.contains("Fresh source material"));
        assert!(!start.text.contains("Original source material"));
        let id = fresh.id;
        queue(&scene, fresh).await;
        assert!(admit_stored_command(&scene.rig.world.app.app, id).await?);
        let frame = request_frame(&scene, receipt.id).await;
        assert_eq!(frame["read_attempt"], json!(1));
        assert_ne!(frame["call_id"], json!(receipt.id));
        start
    } else {
        assert!(admit_stored_command(&scene.rig.world.app.app, original_id).await?);
        original_turn
    };
    let frame = request_frame(&scene, receipt.id).await;
    assert!(
        frame
            .get("model_admitted_at")
            .is_some_and(|at| !at.is_null()),
        "only the successful real native wire cut records possible model work"
    );
    let working = scene
        .rig
        .world
        .app
        .call(SummaryGet, &[&"document", &document])
        .token(&scene.ada.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    let working = serde_json::to_value(working)?;
    assert_eq!(working["program"]["helper_id"], json!(start.thread_id));
    assert_eq!(working["program"]["actor"]["id"], json!(scene.ada.id()));
    assert!(
        working["program"]["provider"].is_null(),
        "shared Use does not disclose the sign-in or its plan"
    );
    scene
        .rig
        .say(start.thread_id, rig::turn_began(start.turn_id))
        .await;
    scene.rig.wait_turn(start.turn_id, "running").await;
    let usage = orch_api_contract::threads::TokenUsage {
        input_tokens: 13,
        output_tokens: 7,
        total_tokens: 20,
        ..Default::default()
    };
    scene
        .rig
        .say(
            start.thread_id,
            Event::UsageUpdated(UsageUpdated {
                turn_id: start.turn_id,
                turn: usage,
                thread: usage,
                context_window: None,
                context_used: None,
                report: None,
            }),
        )
        .await;
    if !changed_before_admission {
        change_document(&scene, document).await;
    }
    scene
        .rig
        .say(
            start.thread_id,
            rig::message_began(start.turn_id, "native-summary-answer"),
        )
        .await;
    scene
        .rig
        .say(
            start.thread_id,
            rig::message_grew("native-summary-answer", 0, "The fresh work is summarized."),
        )
        .await;
    scene
        .rig
        .say(
            start.thread_id,
            rig::message_done("native-summary-answer", "The fresh work is summarized."),
        )
        .await;
    scene
        .rig
        .say(
            start.thread_id,
            rig::turn_ended(start.turn_id, TurnOutcome::Completed),
        )
        .await;
    rig::eventually("the original resource request settles", || async {
        let state = request_frame(&scene, receipt.id).await;
        matches!(
            state["state"].as_str(),
            Some("done" | "discarded" | "failed")
        )
    })
    .await;
    let frame = request_frame(&scene, receipt.id).await;
    let snapshot = scene
        .rig
        .world
        .app
        .call(SummaryGet, &[&"document", &document])
        .token(&scene.ada.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    if changed_before_admission {
        assert_eq!(frame["state"], json!("done"));
        assert_eq!(
            snapshot.text.as_deref(),
            Some("The fresh work is summarized.")
        );
    } else {
        assert_eq!(frame["state"], json!("discarded"));
        assert_eq!(frame["problem"]["reason"], json!("source_changed"));
        assert_eq!(frame["read_attempt"], json!(0));
        assert_eq!(
            snapshot.text.as_deref(),
            Some("Previously approved paragraph.")
        );
    }
    rig::eventually("native usage keeps its original source and issuer", || async {
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(sum(total_tokens) FILTER(WHERE counted),0)::bigint FROM usage_events WHERE turn_id=$1 AND purpose='summary' AND user_id=$2 AND work_node_id=$3")
            .bind(start.turn_id).bind(scene.ada.id()).bind(scene.tree.topic.base.id)
            .fetch_one(scene.rig.db.pool()).await.unwrap() == 20
    }).await;
    assert!(
        scene
            .rig
            .seen
            .of_kind(CommandKind::ProviderComplete)
            .is_empty()
    );
    assert!(
        scene
            .rig
            .seen
            .of_kind(CommandKind::ProviderGenerate)
            .is_empty()
    );
    let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
        .bind(start.thread_id)
        .fetch_one(scene.rig.db.pool())
        .await?;
    assert_eq!(owner, scene.ada.id());
    let hidden = scene
        .rig
        .world
        .app
        .call(ThreadItemList, &[&start.thread_id])
        .query(&ThreadItemListQuery::default())
        .token(&scene.bea.token)
        .send()
        .await;
    assert!([403, 404].contains(&hidden.status.as_u16()));
    let other = scene
        .rig
        .world
        .app
        .call(SummaryGet, &[&"document", &document])
        .token(&scene.bea.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert!(
        serde_json::to_value(other)?
            .get("program")
            .is_none_or(Json::is_null),
        "another reader's paragraph does not reveal a personal request's helper"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_program_summary_rebuilds_changed_source_before_its_native_model_admission()
-> Proof {
    run(true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paid_program_summary_keeps_the_previous_paragraph_when_its_source_changes() -> Proof {
    run(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_summary_source_read_leaves_its_document_and_asking_key_available() -> Proof {
    use orch_engine::summaries::resource::command::observation;
    use std::sync::atomic::{AtomicBool, Ordering};
    let (scene, document, choice) = scene().await;
    let held = hold_native(&scene);
    let receipt = scene
        .rig
        .world
        .app
        .call(SummaryRefresh, &[&"document", &document])
        .token(&scene.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(choice),
            range: None,
        })
        .send()
        .await
        .ok();
    let command = tokio::time::timeout(rig::PATIENCE, held).await?;
    let id = command.id;
    let _replacement = scene.rig.commands.stop_next_issue(command.kind);
    queue(&scene, command).await;
    let changed = Arc::new(AtomicBool::new(false));
    let (source, moved) = (scene.clone(), changed.clone());
    let checked = tokio::time::timeout(
        rig::PATIENCE,
        observation::follow(
            move |request| {
                let (source, moved) = (source.clone(), moved.clone());
                Box::pin(async move {
                    assert_eq!(request, receipt.id);
                    if !moved.swap(true, Ordering::SeqCst) {
                        change_document(&source, document).await;
                    }
                })
            },
            admit_stored_command(&scene.rig.world.app.app, id),
        ),
    )
    .await
    .expect("the source read must not hold the document or the asking key");
    assert!(changed.load(Ordering::SeqCst));
    assert!(
        checked.is_err(),
        "the changed old prompt never reaches a model"
    );
    assert!(
        request_frame(&scene, receipt.id)
            .await
            .get("model_admitted_at")
            .is_none_or(Json::is_null)
    );
    Ok(())
}
