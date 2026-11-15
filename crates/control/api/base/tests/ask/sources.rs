//! Checked sources keep their current readable content across questions.
use super::context::send_as;
use super::*;
use orch_api_contract::documents::{
    BlockContent, DocumentOp, DocumentOpsCreate, DocumentOpsCreateBody, DocumentSync,
    DocumentSyncGet, DocumentSyncGetQuery, Inline, NodeDocumentCreate, NodeDocumentCreateBody,
};
use orch_link_contract::commands::ProviderGenerate;

async fn document(s: &Scene, text: &str) -> DocumentSync {
    let made = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Readable source".into(),
            blocks: None,
            markdown: Some(text.into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    s.rig
        .world
        .app
        .call(DocumentSyncGet, &[&made.base.id])
        .token(&s.ada.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok()
}

fn reads(s: &Scene, name: &str, arguments: Json) -> Arc<Mutex<Vec<ProviderGenerate>>> {
    reads_with_summary(
        s,
        name,
        arguments,
        "The earlier checked source is retained.".into(),
    )
}

fn reads_with_summary(
    s: &Scene,
    name: &str,
    arguments: Json,
    summary: String,
) -> Arc<Mutex<Vec<ProviderGenerate>>> {
    let name = name.to_owned();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation")
            };
            let mut kept = kept.lock().unwrap();
            let first = kept.is_empty();
            kept.push(request.clone());
            let CommandResult::ProviderGenerate(mut answer) = simple_answer() else {
                unreachable!()
            };
            if request.input.tools.is_empty() {
                answer.output.message.content = vec![GenerationPart::Text {
                    text: summary.clone(),
                }];
            } else if first {
                answer.output.message.content = vec![GenerationPart::ToolCall {
                    id: "checked-document".into(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }];
                answer.output.stop = GenerationStop::Tools;
            }
            Ok(CommandResult::ProviderGenerate(answer))
        });
    seen
}

fn result(request: &ProviderGenerate) -> &str {
    request
        .input
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|part| match part {
            GenerationPart::ToolResult {
                content,
                is_error: false,
                ..
            } => Some(content.as_str()),
            _ => None,
        })
        .expect("the checked document reaches the model as a readable result")
}

#[tokio::test]
async fn a_large_read_can_be_folded_and_reused_on_the_next_questions() {
    let s = Scene::new().await;
    configured_provider(&s).await;
    let question = "Read the source and remember where it is.";
    let measured = super::context::recorder(&s);
    let probe = create_unseeded(&s, &s.ada).await;
    send_as(
        &s,
        &s.ada,
        probe.conversation_id,
        "measure-reading",
        question.into(),
    )
    .await;
    let used =
        orch_engine::ask::context::estimate(MODEL, &measured.lock().unwrap()[0].input).unwrap();
    let paragraph = format!("{}\n\n", "Current checked material. ".repeat(1000));
    let part_tokens = orch_engine::ask::context::result_tokens(MODEL, &paragraph);
    // This full source exceeds the request allowance by at least one complete paragraph;
    // each paragraph remains below the native block size while the model receives a token-fitted projection.
    let blocks = usize::try_from(used / part_tokens + 2).unwrap();
    let made = document(&s, &paragraph.repeat(blocks)).await;
    let source = made.document.base.id;
    setting(
        &s,
        "ask.context_tokens",
        json!(super::context::window_leaving(used + 400)),
    )
    .await;
    let requests = reads_with_summary(
        &s,
        "read",
        json!({"operation":"DocumentSyncGet.read","parameters":{"path":{"id":source}}}),
        format!("The checked source is /doc/{source}."),
    );
    let conversation = create_unseeded(&s, &s.ada).await.conversation_id;
    for (client, text) in [
        ("first-reading", question),
        ("refer-to-reading", "Which source did you read?"),
        ("reuse-reading", "Continue using that source."),
    ] {
        let receipt = send_as(&s, &s.ada, conversation, client, text.into()).await;
        let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
            .bind(receipt.turn_id.unwrap())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            error, None,
            "a preserved large read does not make the next question impossible"
        );
    }
    let calls = requests.lock().unwrap();
    assert!(result(&calls[1]).contains("Result shortened to fit the context"));
    let delivered = result(&calls[1])
        .split("\n[Result shortened")
        .next()
        .unwrap();
    let summary = calls
        .iter()
        .find(|request| request.input.tools.is_empty())
        .unwrap();
    assert!(
        summary
            .input
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|part| matches!(part, GenerationPart::Text { text } if text.contains(delivered))),
        "folding is given at least the checked words the first answer received"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|request| request.input.tools.is_empty())
            .count(),
        1,
        "one charged summary is enough and its checked source is reusable"
    );
    let last = calls.last().unwrap();
    assert!(super::context::has_text(
        last,
        "Earlier messages summarized"
    ));
    assert!(super::context::has_text(last, &format!("/doc/{source}")));
}

#[tokio::test]
async fn search_keeps_checked_empty_results_and_refuses_another_root_the_person_can_read() {
    let s = Scene::new().await;
    configured_provider(&s).await;
    let other = places::make_org(
        &s.rig.world,
        &s.ada,
        "Private other root marker",
        "other-search-root",
    )
    .await
    .base
    .id;
    for (place, expected_error) in [(s.tree.topic.base.id, false), (other, true)] {
        let requests = reads(
            &s,
            "search",
            json!({"query":"NoMatchingSourceCanExist","place":place}),
        );
        let conversation = create(&s, &s.ada, None).await.conversation_id;
        send_as(
            &s,
            &s.ada,
            conversation,
            "search-scope",
            "Search for the source.".into(),
        )
        .await;
        send_as(
            &s,
            &s.ada,
            conversation,
            "search-follow-up",
            "What did the search find?".into(),
        )
        .await;
        let calls = requests.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for request in &calls[1..] {
            let (content, is_error) = request
                .input
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .find_map(|part| match part {
                    GenerationPart::ToolResult {
                        content, is_error, ..
                    } => Some((content, *is_error)),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                is_error, expected_error,
                "the native search keeps its actual conversation boundary: {content}"
            );
            assert!(!content.contains("Private other root marker"));
            if !is_error {
                let value: Json = serde_json::from_str(content).unwrap();
                assert!(
                    value["data"]["events"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|event| event["type"] == "cursor"),
                    "an exhausted source still has a checked native terminal cursor"
                );
            }
        }
    }
}

#[tokio::test]
async fn management_with_as_admin_and_only_foreign_provider_use_keeps_a_source_for_the_follow_up() {
    let s = Scene::new().await;
    let shared = provider(&s).await;
    let home = places::make_org(
        &s.rig.world,
        &s.bea,
        "Other provider home",
        "other-provider-home",
    )
    .await
    .base
    .id;
    sqlx::query("UPDATE providers SET owner_id=$2,org_id=$3,home_id=$3 WHERE id=$1")
        .bind(shared)
        .bind(s.bea.id())
        .bind(home)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE object_kind='provider' AND object_id=$1 AND permission='access.own'")
        .bind(shared).bind(s.rig.world.now()).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .lets_use("provider", shared, "provider.use", &s.ada, json!({}))
        .await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":shared,"model":MODEL}]),
    )
    .await;
    let made = document(
        &s,
        &format!(
            "{} shared-source-end-marker",
            "Details beyond the reference card. ".repeat(100)
        ),
    )
    .await;
    let requests = reads(
        &s,
        "open",
        json!({"reference":format!("/doc/{}",made.document.base.id)}),
    );
    places::admin_mode(&s.rig.world, &s.ada, true).await;
    let models = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.ada.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        models.blocked_reason, None,
        "ordinary foreign Use offers an Ask model"
    );
    let conversation = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.ada.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: Some("/admin/overview".into()),
            model: None,
            retention_days: None,
        })
        .send()
        .await
        .ok()
        .conversation_id;
    for (client, question) in [
        ("management-read", "Open the source and read it to the end."),
        (
            "management-follow-up",
            "What was the last sentence of the source you opened?",
        ),
    ] {
        let receipt = send_as(&s, &s.ada, conversation, client, question.into()).await;
        let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
            .bind(receipt.turn_id.unwrap())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            error, None,
            "Management answers with ordinary foreign provider Use"
        );
    }
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|request| request.provider_id == shared));
    for request in &calls[1..] {
        assert!(super::context::has_text(request, "/admin/overview"));
        assert!(
            result(request).contains("shared-source-end-marker"),
            "the next step and next question have the full source"
        );
    }
    let listed = s
        .rig
        .world
        .app
        .call(AskList, &[])
        .token(&s.ada.token)
        .query(&AskListQuery {
            root_org_id: Some(s.tree.org.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(listed.items.iter().any(|row| row.id == conversation));
}

#[tokio::test]
async fn opening_a_document_and_its_selected_block_keeps_the_words_beyond_the_reference_card() {
    let s = Scene::new().await;
    configured_provider(&s).await;
    let full = format!(
        "{} document-body-end-marker\n\nother-block-only",
        "Readable project detail. ".repeat(100)
    );
    let made = document(&s, &full).await;
    let id = made.document.base.id;
    for selected in [false, true] {
        let reference = if selected {
            format!("/doc/{id}?block={}", made.blocks[0].id)
        } else {
            format!("/doc/{id}")
        };
        let requests = reads(&s, "open", json!({"reference":reference}));
        let conversation = create(&s, &s.ada, None).await.conversation_id;
        send_as(
            &s,
            &s.ada,
            conversation,
            "open-full-source",
            "Read to the end of this source.".into(),
        )
        .await;
        send_as(
            &s,
            &s.ada,
            conversation,
            "refer-to-end",
            "What did the source's last sentence say?".into(),
        )
        .await;
        let calls = requests.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for call in &calls[1..] {
            let given = result(call);
            assert!(
                given.contains("document-body-end-marker"),
                "the body is read past character 500"
            );
            assert_eq!(
                given.contains("other-block-only"),
                !selected,
                "the selector is kept during refresh"
            );
            let value: Json = serde_json::from_str(given).unwrap();
            assert_eq!(
                value["data"]["blocks"].as_array().unwrap().len(),
                if selected { 1 } else { 2 }
            );
            assert!(
                value["citations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|link| link["url"] == reference),
                "the canonical link keeps its selected block"
            );
        }
    }
}

#[tokio::test]
async fn a_follow_up_receives_the_whole_grown_source_when_tokens_are_available() {
    let s = Scene::new().await;
    configured_provider(&s).await;
    let original = format!(
        "{} original-source-end",
        "Original project details. ".repeat(500)
    );
    let made = document(&s, &original).await;
    let id = made.document.base.id;
    let requests = reads(
        &s,
        "read",
        json!({"operation":"DocumentSyncGet.read","parameters":{"path":{"id":id}}}),
    );
    let conversation = create(&s, &s.ada, None).await.conversation_id;
    send_as(
        &s,
        &s.ada,
        conversation,
        "first-source",
        "Read the source.".into(),
    )
    .await;
    let grown = format!(
        "{} grown-source-end",
        "Newly added project details. ".repeat(1000)
    );
    s.rig
        .world
        .app
        .call(DocumentOpsCreate, &[&id])
        .token(&s.ada.token)
        .body(&DocumentOpsCreateBody {
            expected_seq: Some(made.seq),
            client_id: new_id().to_string(),
            ops: vec![DocumentOp::Update {
                id: made.blocks[0].id,
                base_seq: made.blocks[0].seq_updated,
                block_type: None,
                props: None,
                content: Some(BlockContent::Inline(vec![Inline::Text {
                    text: format!("{original}\n{grown}"),
                    marks: None,
                    href: None,
                    color: None,
                    background: None,
                }])),
            }],
        })
        .send()
        .await
        .ok();
    send_as(
        &s,
        &s.ada,
        conversation,
        "grown-source",
        "What did the source add at its end?".into(),
    )
    .await;
    let calls = requests.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(result(&calls[1]).contains("original-source-end"));
    assert!(
        result(&calls[2]).contains("grown-source-end"),
        "fresh reading is not frozen at the first response's byte count"
    );
    let budget = orch_engine::ask::context::Budget::new(
        &calls[2].model_parameters,
        MODEL,
        None,
        calls[2].input.max_output_tokens,
    );
    assert!(budget.fits(&calls[2].input).unwrap());
}

#[tokio::test]
async fn a_conversation_copied_to_another_org_uses_its_destination_knowledge_provider_and_usage_from_the_first_turn()
 {
    let s = Scene::new().await;
    let (source, source_provider) = kept_native_source(&s).await;
    let source_document = document(&s, "Knowledge that stays in the source organization.")
        .await
        .document
        .base
        .id;
    let root = places::make_org(
        &s.rig.world,
        &s.ada,
        "Destination knowledge",
        "copy-destination-knowledge",
    )
    .await
    .base
    .id;
    let project = places::make_project(&s.rig.world, &s.ada, root, "Destination project")
        .await
        .base
        .id;
    let topic = places::make_topic(&s.rig.world, &s.ada, project, "Destination topic")
        .await
        .base
        .id;
    let destination_document = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&topic])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Destination source".into(),
            blocks: None,
            markdown: Some("Knowledge available in the destination organization.".into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    let destination_provider = provider(&s).await;
    sqlx::query("UPDATE providers SET org_id=$2,home_id=$2 WHERE id=$1")
        .bind(destination_provider)
        .bind(root)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(root),
            "ask.models",
            json!([{"provider_id":destination_provider,"model":MODEL}]),
        )
        .await;
    // A selected source model and payer cannot silently become the new org's configuration.
    sqlx::query("UPDATE threads SET ask_model=$2,ask_payer_id=$3 WHERE node_id=$1")
        .bind(source)
        .bind(json!({"provider_id":source_provider,"model":MODEL}))
        .bind(s.ada.id())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let copied = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source])
        .token(&s.ada.token)
        .body(&AskKeepBody {
            topic_id: Some(topic),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let roots: Vec<(Id, Id)> = sqlx::query_as(
        "SELECT node_id,program_root_org_id FROM threads WHERE node_id=ANY($1) ORDER BY node_id",
    )
    .bind(vec![source, copied.thread_id])
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert!(
        roots.contains(&(source, s.tree.org.base.id)),
        "the source's fixed root stays unchanged"
    );
    assert!(
        roots.contains(&(copied.thread_id, root)),
        "the new conversation is created in its destination root"
    );
    let preference: (Option<Json>, Option<Id>) =
        sqlx::query_as("SELECT ask_model,ask_payer_id FROM threads WHERE node_id=$1")
            .bind(copied.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(preference, (None, None));
    for (reference, expected_error) in [(source_document, true), (destination_document, false)] {
        let calls = reads(&s, "open", json!({"reference":format!("/doc/{reference}")}));
        send_as(
            &s,
            &s.ada,
            copied.thread_id,
            &format!("copied-root-{reference}"),
            "Open the requested knowledge source.".into(),
        )
        .await;
        let calls = calls.lock().unwrap();
        assert!(!calls.is_empty());
        assert!(
            calls
                .iter()
                .all(|request| request.provider_id == destination_provider),
            "the first and later turns use the destination's provider choices"
        );
        let (content, error) = calls
            .last()
            .unwrap()
            .input
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|part| match part {
                GenerationPart::ToolResult {
                    content, is_error, ..
                } => Some((content, *is_error)),
                _ => None,
            })
            .last()
            .unwrap();
        assert_eq!(
            error, expected_error,
            "source access belongs to the new conversation's root: {content}"
        );
        if !error {
            assert!(content.contains("Knowledge available in the destination organization."));
        }
    }
    let charged: Vec<(Id, Id)> = sqlx::query_as(
        "SELECT DISTINCT org_id,provider_id FROM usage_events WHERE thread_id=$1 AND counted",
    )
    .bind(copied.thread_id)
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert!(!charged.is_empty());
    assert!(
        charged
            .iter()
            .all(|entry| *entry == (root, destination_provider))
    );
}
