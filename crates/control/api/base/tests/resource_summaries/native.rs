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
use orch_engine::commands::Outgoing;
use orch_link_contract::{
    commands::{AgentStartResult, Command, CommandKind, CommandResult, TurnStart},
    continuations::TurnStartFenced,
    events::{Event, TurnOutcome, UsageUpdated},
};
use std::sync::Arc;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn scene() -> (Arc<rig::Scene>, Id, SummaryModelChoice) {
    let scene = Arc::new(rig::Scene::new().await);
    scene
        .rig
        .commands
        .check_turn_starts_on_wire(&scene.rig.world.app.app);
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
    // This personal refresh remains eligible even when automatic work reserves the whole plan.
    scene
        .rig
        .world
        .set_at(
            orch_settings::Target::Node(scene.tree.org.base.id),
            "delegation.background_headroom_percent",
            json!(100),
        )
        .await;
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

async fn first_native<T>(
    scene: &rig::Scene,
    request: Id,
    pending: impl std::future::Future<Output = T>,
) -> Proof<T> {
    if let Ok(command) = tokio::time::timeout(rig::PATIENCE, pending).await {
        return Ok(command);
    }
    let progress:Json=sqlx::query_scalar("SELECT jsonb_build_object(
        'request',r.state,'reason',r.problem->>'reason','read_attempt',r.read_attempt,
        'paid',r.model_admitted_at IS NOT NULL,'call',c.id IS NOT NULL,'recorded',c.delegated_program IS NOT NULL,
        'helper',h.state,'helper_step',h.starting_step,'blocked',h.blocked_reason,'input',i.state,'turn',t.state,
        'commands',(SELECT jsonb_object_agg(kind,state) FROM commands WHERE serial_key=w.helper_id::text))
        FROM resource_summary_requests r LEFT JOIN summary_calls c ON c.resource_request_id=r.id
        LEFT JOIN delegated_program_work w ON w.source_summary_call_id=c.id
        LEFT JOIN threads h ON h.node_id=w.helper_id LEFT JOIN thread_inputs i ON i.id=w.initial_input_id
        LEFT JOIN turns t ON t.id=i.turn_id WHERE r.id=$1 ORDER BY c.created_at DESC LIMIT 1")
        .bind(request).fetch_one(scene.rig.db.pool()).await?;
    Err(format!("the resource summary did not issue its first native turn: {progress}").into())
}

async fn received_native(
    scene: &rig::Scene,
    request: Id,
    previous: Option<Id>,
) -> Proof<TurnStart> {
    first_native(scene, request, async {
        loop {
            for command in scene.rig.seen.all() {
                let start = match command {
                    Command::TurnStart(start) => start,
                    Command::TurnStartFenced(fenced) => *fenced.turn,
                    _ => continue,
                };
                if Some(start.thread_id) != previous {
                    return start;
                }
            }
            tokio::time::sleep(scene.rig.engine.config().sweep_every).await;
        }
    })
    .await
}

#[tokio::test]
async fn a_document_summary_can_check_its_source_while_a_worker_references_its_topic() -> Proof {
    let (scene, document, choice) = scene().await;
    let mut reference = scene.rig.db.begin().await?;
    let topic = scene.tree.topic.base.id;
    // PostgreSQL holds this lock when a child references its topic through a foreign key.
    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR KEY SHARE")
        .bind(topic)
        .fetch_one(&mut *reference)
        .await?;
    let mut check = scene.rig.db.begin().await?;
    let selected = orch_engine::summaries::resource::select(
        &mut check,
        &orch_authz::Caller::session(scene.ada.id()),
        SummaryTarget::Document { id: document },
        None,
        Some(&choice),
        orch_types::Time::new(scene.rig.world.now()),
    )
    .await?;
    tokio::time::timeout(
        rig::PATIENCE,
        orch_engine::summaries::resource::lock_selection(&mut check, &selected),
    )
    .await??;
    let conflict = sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR NO KEY UPDATE NOWAIT")
        .bind(topic)
        .fetch_one(&mut *reference)
        .await
        .expect_err("changing the same topic must still wait");
    assert_eq!(
        conflict
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03")
    );
    reference.rollback().await?;
    check.commit().await?;
    Ok(())
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
    let pending = changed_before_admission.then(|| hold_native(&scene));
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
    let start = if let Some(pending) = pending {
        let original = first_native(&scene, receipt.id, pending).await?;
        let original_turn = turn(&original);
        assert!(request_frame(&scene, receipt.id).await["model_admitted_at"].is_null());
        change_document(&scene, document).await;
        assert!(
            admit_stored_command(&scene.rig.world.app.app, original.id)
                .await
                .is_err(),
            "the queued old prompt cannot cross the real wire after its source changes"
        );
        let start = received_native(&scene, receipt.id, Some(original_turn.thread_id)).await?;
        assert_ne!(start.thread_id, original_turn.thread_id);
        assert!(scene.rig.turn_starts(original_turn.thread_id).is_empty());
        // The earlier worker's clock can be ahead of the one that rebuilt the prompt.
        sqlx::query("UPDATE delegated_program_work old SET created_at=fresh.created_at+INTERVAL '1 microsecond'
            FROM delegated_program_work fresh WHERE old.helper_id=$1 AND fresh.helper_id=$2")
            .bind(original_turn.thread_id).bind(start.thread_id).execute(scene.rig.db.pool()).await?;
        assert!(start.text.contains("Fresh source material"));
        assert!(!start.text.contains("Original source material"));
        let frame = request_frame(&scene, receipt.id).await;
        assert_eq!(frame["read_attempt"], json!(1));
        assert_ne!(frame["call_id"], json!(receipt.id));
        start
    } else {
        received_native(&scene, receipt.id, None).await?
    };
    assert!(
        !serde_json::to_string(&start)?.contains(scene.ada.token.as_str()),
        "the native program receives no acting credential"
    );
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
    // This account's provider Use includes provider View. Its document-only key
    // can read the same paragraph without gaining either optional metadata target.
    assert_eq!(
        working["program"]["provider"]["id"],
        json!(scene.on.provider)
    );
    let (_, reader) = document_summary_key(&scene, document, "SummaryGet", &["read"]).await;
    let restricted = scene
        .rig
        .world
        .app
        .call(SummaryGet, &[&"document", &document])
        .token(&reader)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        restricted.text.as_deref(),
        Some("Previously approved paragraph.")
    );
    let restricted = serde_json::to_value(restricted)?;
    assert!(restricted["program"]["provider"].is_null());
    assert!(restricted["program"]["helper_id"].is_null());
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
        rig::eventually("the running helper has reported its actual usage before the edit", || async {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM turns WHERE id=$1 AND usage->>'total_tokens'='20')")
                .bind(start.turn_id).fetch_one(scene.rig.db.pool()).await.unwrap()
        }).await;
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
        .token(&scene.cai.token)
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
    let (key_id, raw_key) =
        document_summary_key(&scene, document, "SummaryRefresh", &["read", "write"]).await;
    let held = hold_native(&scene);
    let receipt = scene
        .rig
        .world
        .app
        .call(SummaryRefresh, &[&"document", &document])
        .token(&raw_key)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(choice),
            range: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(receipt.projection, ReceiptProjection::Receipt);
    let command = first_native(&scene, receipt.id, held).await?;
    let id = command.id;
    let _replacement = scene.rig.commands.stop_next_issue(command.kind);
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
                        // Other workers may be finishing a short check of this key. This
                        // wire read stays paused until the separate transaction takes it.
                        let mut edit = source.rig.db.begin().await.unwrap();
                        sqlx::query("SELECT id FROM api_tokens WHERE id=$1 FOR UPDATE")
                            .bind(key_id)
                            .fetch_one(&mut *edit)
                            .await
                            .unwrap();
                        edit.commit().await.unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_automatic_document_keeps_its_paragraph_and_original_request_while_waiting_for_plan_usage()
-> Proof {
    let (scene, document, _) = scene().await;
    let target = SummaryTarget::Document { id: document };
    let cache = cache_in(
        &scene.rig.world,
        target,
        None,
        "The shared paragraph.",
        json!([{"kind":"document","id":document}]),
    )
    .await;
    scene
        .rig
        .world
        .app
        .call(
            NodeSettingSet,
            &[&scene.tree.topic.base.id, &"summary.targets"],
        )
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            value: json!({"document":{"automatic":true,"min_interval_secs":0}}),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    change_document(&scene, document).await;
    rig::eventually("the automatic request waits without a paid start", ||async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM resource_summary_requests WHERE summary_id=$1 AND origin='automatic' AND state='queued'
            AND problem->>'reason'='usage_measurement' AND model_admitted_at IS NULL AND lease_token IS NULL AND due_at>$2)")
            .bind(cache).bind(scene.rig.world.now()).fetch_one(scene.rig.db.pool()).await.unwrap()
    }).await;
    let snapshot = scene
        .rig
        .world
        .app
        .call(SummaryGet, &[&"document", &document])
        .token(&scene.cai.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(snapshot.state, SummaryState::Scheduled);
    assert_eq!(
        snapshot.wait.map(|hold| hold.reason),
        Some(orch_api_contract::summaries::SummaryWaitReason::UsageMeasurement)
    );
    assert_eq!(snapshot.text.as_deref(), Some("The shared paragraph."));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM resource_summary_requests WHERE summary_id=$1")
            .bind(cache)
            .fetch_one(scene.rig.db.pool())
            .await?;
    assert_eq!(count, 1);
    assert!(
        scene.rig.seen.of_kind(CommandKind::TurnStart).is_empty()
            && scene
                .rig
                .seen
                .of_kind(CommandKind::TurnStartFenced)
                .is_empty()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_using_one_subscription_receive_separate_native_conversations_and_usage() -> Proof
{
    let (scene, first_doc, choice) = scene().await;
    let second_doc = scene
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.eve.token)
        .body(&NodeDocumentCreateBody {
            title: "Second source".into(),
            markdown: Some("Separate source for the other participant.".into()),
            blocks: None,
            access: Some(orch_api_contract::documents::NewDocumentAccess::Private),
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "the second document owner could not read its created source")?
        .base
        .id;
    let refresh = |document: Id, token: String| {
        let scene = scene.clone();
        let model = choice.clone();
        async move {
            let request = scene
                .rig
                .world
                .app
                .call(SummaryRefresh, &[&"document", &document])
                .token(&token)
                .body(&SummaryRefreshBody {
                    client_id: new_id(),
                    model: Some(model),
                    range: None,
                });
            request.send().await.ok().id
        }
    };
    let first_request = refresh(first_doc, scene.ada.token.clone()).await;
    let a = received_native(&scene, first_request, None).await?;
    scene.rig.say(a.thread_id, rig::turn_began(a.turn_id)).await;
    scene.rig.wait_turn(a.turn_id, "running").await;
    let second_request = refresh(second_doc, scene.eve.token.clone()).await;
    let b = received_native(&scene, second_request, Some(a.thread_id)).await?;
    scene.rig.say(b.thread_id, rig::turn_began(b.turn_id)).await;
    scene.rig.wait_turn(b.turn_id, "running").await;
    assert_ne!(a.thread_id, b.thread_id);
    assert!(
        a.text.contains("Original source material.")
            && !a
                .text
                .contains("Separate source for the other participant.")
    );
    assert!(
        b.text
            .contains("Separate source for the other participant.")
            && !b.text.contains("Original source material.")
    );
    for (start, user, request, answer) in [
        (&a, scene.ada.id(), first_request, "First private summary."),
        (
            &b,
            scene.eve.id(),
            second_request,
            "Second private summary.",
        ),
    ] {
        let opened: Json=sqlx::query_scalar("SELECT COALESCE(body->'session',body) FROM commands
          WHERE serial_key=$1 AND kind IN ('session.open','session.open.fenced') ORDER BY created_at DESC LIMIT 1")
            .bind(start.thread_id.to_string()).fetch_one(scene.rig.db.pool()).await?;
        assert_eq!(opened["mode"], "new");
        assert!(opened["native_session_ref"].is_null());
        let usage = orch_api_contract::threads::TokenUsage {
            input_tokens: 8,
            output_tokens: 2,
            total_tokens: 10,
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
        scene
            .rig
            .say(start.thread_id, rig::message_began(start.turn_id, "final"))
            .await;
        scene
            .rig
            .say(start.thread_id, rig::message_grew("final", 0, answer))
            .await;
        scene
            .rig
            .say(start.thread_id, rig::message_done("final", answer))
            .await;
        scene
            .rig
            .say(
                start.thread_id,
                rig::turn_ended(start.turn_id, TurnOutcome::Completed),
            )
            .await;
        scene.rig.wait_turn(start.turn_id, "completed").await;
        rig::eventually(
            "the resource request settles after the native reply",
            || async {
                matches!(
                    request_frame(&scene, request).await["state"].as_str(),
                    Some("done" | "failed" | "discarded")
                )
            },
        )
        .await;
        let settled = request_frame(&scene, request).await;
        assert_eq!(
            settled["state"], "done",
            "summary refusal: {}",
            settled["problem"]["reason"]
        );
        let paragraph: String = sqlx::query_scalar(
            "SELECT s.text FROM resource_summary_requests r JOIN resource_summaries s ON s.id=r.summary_id
             WHERE r.id=$1 AND s.completed_request_id=r.id AND s.viewer_id=$2",
        )
        .bind(request).bind(user).fetch_one(scene.rig.db.pool()).await?;
        assert_eq!(paragraph, answer);
        rig::eventually(
            "the terminal ledger counts this person's ten tokens",
            || async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT COALESCE(sum(total_tokens) FILTER(WHERE counted),0)=10
              FROM usage_events WHERE turn_id=$1 AND user_id=$2 AND purpose='summary'",
                )
                .bind(start.turn_id)
                .bind(user)
                .fetch_one(scene.rig.db.pool())
                .await
                .unwrap()
            },
        )
        .await;
    }
    assert!(scene.rig.seen.of_kind(CommandKind::ProviderStop).is_empty());
    Ok(())
}
