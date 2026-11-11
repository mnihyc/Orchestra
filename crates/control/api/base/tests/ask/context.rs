//! Context limits are decided through real Ask commands, visible items and usage rows.
use super::*;
use orch_link_contract::commands::ProviderGenerate;

fn recorder(s: &Scene) -> Arc<Mutex<Vec<ProviderGenerate>>> {
    recorder_with_summary(
        s,
        "The earlier question asked to retain the original project details.".into(),
    )
}

fn recorder_with_summary(s: &Scene, summary: String) -> Arc<Mutex<Vec<ProviderGenerate>>> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation")
            };
            seen.lock().unwrap().push(request.clone());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: Json::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: if request.input.tools.is_empty() {
                                summary.clone()
                            } else {
                                "A short answer.".into()
                            },
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: orch_link_contract::foreign::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_tokens: 15,
                    ..Default::default()
                },
                timing: Default::default(),
                request_ref: None,
            }))
        });
    requests
}

async fn send(s: &Scene, id: Id, client: &str, text: String) -> AskReceipt {
    let receipt = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: client.into(),
            content: vec![ContentPart::Text { text }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the Ask turn settles", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    receipt
}

fn has_text(request: &ProviderGenerate, wanted: &str) -> bool {
    request
        .input
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .any(|part| matches!(part, GenerationPart::Text { text } if text.contains(wanted)))
}

#[tokio::test]
async fn a_short_history_is_complete_and_a_smaller_ask_window_compacts_once_then_reuses_it() {
    // EN: Ask > send a question > lower Ask context tokens > send again > expand
    // Earlier messages summarized. Originals and the charged summary remain in history.
    // 中文：Ask 中提问，降低 Ask 上下文令牌数后再次提问，展开“较早的消息已归纳”；
    // 原消息保留，归纳调用计入本人的 Ask 用量。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    let requests = recorder(&s);
    let made = create_unseeded(&s, &s.cai).await;
    let original = format!("earliest project detail {}", "x".repeat(12_000));
    send(&s, made.conversation_id, "first", original.clone()).await;
    let first = requests.lock().unwrap()[0].clone();
    assert!(has_text(&first, &original));
    assert!(!first.input.tools.is_empty());
    assert_eq!(first.input.max_output_tokens, 4096);
    assert!(
        read(&s, &s.cai, made.conversation_id)
            .await
            .messages
            .iter()
            .all(|message| message.summary.is_none())
    );
    // This leaves the first request plus 1000 model-estimated tokens, enough
    // for the tiny summary and a follow-up, but not a second long question.
    let reserved = 4096 + orch_engine::ask::context::tokens(MODEL, 262144);
    let smaller =
        reserved + orch_engine::ask::context::estimate(MODEL, &first.input).unwrap() + 1000;
    assert!(smaller < first.model_parameters.context_window.unwrap());
    setting(&s, "ask.context_tokens", json!(smaller)).await;
    let second = send(&s, made.conversation_id, "second", original.clone()).await;
    let snapshot = read(&s, &s.cai, made.conversation_id).await;
    let summaries: Vec<_> = snapshot
        .messages
        .iter()
        .filter(|message| message.summary.is_some())
        .collect();
    assert_eq!(summaries.len(), 1);
    let summary = summaries[0];
    let metadata = summary.summary.as_ref().unwrap();
    assert_eq!(metadata.model.provider_id, provider);
    assert_eq!(metadata.model.model, MODEL);
    assert!(summary.complete && summary.role == AskRole::Assistant);
    assert!(summary.content.iter().any(|part| matches!(part, ContentPart::Text { text } if text.contains("original project details"))));
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| message.role == AskRole::User)
            .count(),
        2
    );
    let calls: Vec<_> = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 3);
    assert!(calls[1].input.tools.is_empty());
    assert!(has_text(&calls[1], &original));
    assert!(has_text(&calls[2], "Earlier messages summarized"));
    let counted: (i64, i64) = sqlx::query_as("SELECT count(*),sum(total_tokens)::bigint FROM usage_events WHERE thread_id=$1 AND purpose='ask' AND user_id=$2 AND provider_id=$3")
        .bind(made.conversation_id).bind(s.cai.id()).bind(provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(counted, (3, 45));
    let turn_usage: i64 =
        sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE turn_id=$1 AND purpose='ask'")
            .bind(second.turn_id.unwrap())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        turn_usage, 2,
        "both the summary and answer belong to this person's same turn"
    );
    send(&s, made.conversation_id, "third", "Continue briefly".into()).await;
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 4);
    assert_eq!(
        calls
            .iter()
            .filter(|request| request.input.tools.is_empty())
            .count(),
        1
    );
    assert!(has_text(&calls[3], "Earlier messages summarized"));
    assert_eq!(
        read(&s, &s.cai, made.conversation_id)
            .await
            .messages
            .iter()
            .filter(|message| message.summary.is_some())
            .count(),
        1
    );
    let parameters: Json = sqlx::query_scalar(
        "SELECT parameters FROM provider_models WHERE provider_id=$1 AND model_name=$2",
    )
    .bind(provider)
    .bind(MODEL)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        parameters["context_window"], 200000,
        "Ask's policy never changes the shared model"
    );
}

#[tokio::test]
async fn ask_output_policy_changes_the_reserve_and_an_impossible_window_refuses_before_generation()
{
    // EN browser: set Ask output to 1024 > Ask > send a short question > lower
    // its context window to 1024 > Send; the visible reserve reason replaces Ready.
    // 中文浏览器：将 Ask 输出上限设为 1024 > 提问 > 发送简短问题 > 将上下文上限降至
    // 1024 > 发送；显示预留空间不足的原因，不以就绪提示覆盖。
    let s = Scene::new().await;
    configured_provider(&s).await;
    setting(&s, "ask.max_output_tokens", json!(1024)).await;
    let requests = recorder(&s);
    let made = create_unseeded(&s, &s.cai).await;
    send(&s, made.conversation_id, "short", "A small question".into()).await;
    assert_eq!(requests.lock().unwrap()[0].input.max_output_tokens, 1024);
    setting(&s, "ask.context_tokens", json!(1024)).await;
    let refused = send(
        &s,
        made.conversation_id,
        "no-room",
        "Another question".into(),
    )
    .await;
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "no model call with an impossible window"
    );
    let error: Json = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(refused.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error["code"], "ask_context_no_room");
    let reason = error["message"].as_str().unwrap();
    for setting in [
        "ask.context_tokens",
        "ask.max_output_tokens",
        "ask.turn_tool_bytes",
    ] {
        assert!(reason.contains(setting));
    }
    let events: Vec<Json> =
        sqlx::query_scalar("SELECT body FROM ask_events WHERE thread_id=$1 ORDER BY seq")
            .bind(made.conversation_id)
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "error" && event["code"] == "ask_context_no_room")
    );
    assert!(
        !events
            .iter()
            .rev()
            .take_while(|event| event["kind"] != "error")
            .any(|event| event["code"] == "ready"),
        "the reason is not overwritten by a Ready notice"
    );
}

#[tokio::test]
async fn a_question_that_cannot_fit_even_without_old_history_is_refused_with_the_model_and_choices()
{
    // EN browser: Ask > send a short question > lower its context window > send
    // a long question; the reason names the model and offers a new Ask or Keep as thread.
    // 中文浏览器：提问 > 发送简短问题 > 降低上下文上限 > 发送长问题；原因指出模型，
    // 并提示新建提问或保留为会话。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let requests = recorder(&s);
    let made = create_unseeded(&s, &s.cai).await;
    send(&s, made.conversation_id, "first", "A small question".into()).await;
    let reserved = 4096 + orch_engine::ask::context::tokens(MODEL, 262144);
    setting(&s, "ask.context_tokens", json!(reserved + 100)).await;
    let refused = send(
        &s,
        made.conversation_id,
        "too-long",
        "A question too large for this context".repeat(100),
    )
    .await;
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "the local fit check sends no provider request"
    );
    let error: Json = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(refused.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error["code"], "ask_context_too_long");
    assert_eq!(error["model"], MODEL);
    let reason = error["message"].as_str().unwrap();
    assert!(
        reason.contains(MODEL)
            && reason.contains("start a new one")
            && reason.contains("keep it as a thread")
    );
}

#[tokio::test]
async fn two_hundred_is_a_page_size_and_never_silently_omits_older_messages() {
    // EN browser: Ask > retain more than 200 messages > ask a follow-up;
    // the first question and the full selected history still inform the answer.
    // 中文浏览器：提问 > 保留超过 200 条消息 > 继续提问；回答仍应使用首个问题和
    // 完整的已选历史，不因数据库分页而静默丢弃早期消息。
    use orch_db::queries::threads;
    let s = Scene::new().await;
    configured_provider(&s).await;
    let requests = recorder(&s);
    let made = create_unseeded(&s, &s.cai).await;
    let first = send(
        &s,
        made.conversation_id,
        "first",
        "the first original question".into(),
    )
    .await;
    let turn = first.turn_id.unwrap();
    let mut conn = s.rig.world.conn().await;
    let segment = threads::turn(&mut conn, turn)
        .await
        .unwrap()
        .unwrap()
        .segment_id;
    // Retain more than one database page of ordinary messages in the selected
    // lineage, while keeping the original user-turn frontier intact.
    for index in 0..210 {
        let text = format!("retained historical message {index}");
        let message = GenerationMessage {
            role: GenerationRole::Assistant,
            content: vec![GenerationPart::Text { text: text.clone() }],
            continuation: vec![],
        };
        let payload = json!({"text":text,"text_len":text.chars().count(),"phase":"commentary","ask_message":message});
        let seq = orch_db::sequences::next(&mut conn, made.conversation_id)
            .await
            .unwrap()
            .unwrap();
        threads::insert_item(
            &mut conn,
            &threads::NewItem {
                id: new_id(),
                thread_id: made.conversation_id,
                segment_id: segment,
                turn_id: Some(turn),
                stream: "main",
                parent_item_id: None,
                kind: "agent_message",
                status: "completed",
                author_id: None,
                payload: &payload,
                native_ref: None,
                search_text: None,
                at: s.rig.world.now(),
            },
            seq,
        )
        .await
        .unwrap();
    }
    drop(conn);
    send(&s, made.conversation_id, "continue", "Continue".into()).await;
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "this short history needs no compaction");
    let last = &calls[1];
    assert!(has_text(last, "the first original question"));
    for index in 0..210 {
        assert!(has_text(
            last,
            &format!("retained historical message {index}")
        ));
    }
}

#[tokio::test]
async fn even_a_completed_summary_that_cannot_fit_is_visible_counted_and_refused_locally() {
    // EN browser: Ask > send a long question > lower the context window > Send;
    // the completed summary stays visible and counted, followed by the fit refusal.
    // 中文浏览器：提问 > 发送长问题 > 降低上下文上限 > 发送；已完成的归纳保持可见、
    // 计入用量，随后显示无法容纳的原因，不再发起回答请求。
    let s = Scene::new().await;
    configured_provider(&s).await;
    // At this model's text estimate this remains below the 4096-token output
    // allowance, but replacing one long question barely shrinks the history.
    let requests = recorder_with_summary(&s, "s".repeat(12_000));
    let made = create_unseeded(&s, &s.cai).await;
    let question = format!("Keep these original details {}", "x".repeat(12_000));
    send(&s, made.conversation_id, "first", question.clone()).await;
    let first = requests.lock().unwrap()[0].clone();
    let reserved = 4096 + orch_engine::ask::context::tokens(MODEL, 262144);
    setting(
        &s,
        "ask.context_tokens",
        json!(reserved + orch_engine::ask::context::estimate(MODEL, &first.input).unwrap() + 1000),
    )
    .await;
    let refused = send(&s, made.conversation_id, "second", question).await;
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "no unfitted answer is sent to the provider");
    assert!(calls[1].input.tools.is_empty());
    let snapshot = read(&s, &s.cai, made.conversation_id).await;
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| message.summary.is_some() && message.complete)
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| message.role == AskRole::User)
            .count(),
        2
    );
    let error: Json = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(refused.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error["code"], "ask_context_too_long");
    assert_eq!(error["model"], MODEL);
    let charged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='ask' AND user_id=$2",
    )
    .bind(made.conversation_id)
    .bind(s.cai.id())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(charged, 2);
}
