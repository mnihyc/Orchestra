//! Context limits are decided through real Ask commands, visible items and usage rows.
use super::*;
use orch_link_contract::commands::ProviderGenerate;

pub(super) fn recorder(s: &Scene) -> Arc<Mutex<Vec<ProviderGenerate>>> {
    recorder_with_summary(
        s,
        "The earlier question asked to retain the original project details.".into(),
    )
}

pub(super) fn recorder_with_summary(
    s: &Scene,
    summary: String,
) -> Arc<Mutex<Vec<ProviderGenerate>>> {
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

/// The Ask window that leaves `room` tokens for a request: the answer takes half of a small window, and its
/// 4096 tokens of a larger one.
pub(super) fn window_leaving(room: i64) -> i64 {
    if room <= 4096 { 2 * room } else { room + 4096 }
}

pub(super) async fn send(s: &Scene, id: Id, client: &str, text: String) -> AskReceipt {
    send_as(s, &s.cai, id, client, text).await
}

pub(super) async fn send_as(
    s: &Scene,
    who: &places::Who,
    id: Id,
    client: &str,
    text: String,
) -> AskReceipt {
    let receipt = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&id])
        .token(&who.token)
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

pub(super) fn has_text(request: &ProviderGenerate, wanted: &str) -> bool {
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
    let smaller =
        window_leaving(orch_engine::ask::context::estimate(MODEL, &first.input).unwrap() + 1000);
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
async fn a_model_added_by_name_with_no_window_answers_with_every_setting_as_shipped() {
    // EN: Providers > add a model by name, without a context window > Ask > send a question; it is answered.
    // 中文：在提供方中按名称添加不带上下文窗口的模型 > Ask > 提问；得到回答。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    sqlx::query(
        "UPDATE provider_models SET parameters='{}' WHERE provider_id=$1 AND model_name=$2",
    )
    .bind(provider)
    .bind(MODEL)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let requests = recorder(&s);
    let made = create_unseeded(&s, &s.cai).await;
    let receipt = send(
        &s,
        made.conversation_id,
        "no-window",
        "A first question".into(),
    )
    .await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None, "the turn is not refused for room");
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "the model was asked");
    assert!(has_text(&calls[0], "A first question"));
    assert_eq!(calls[0].input.max_output_tokens, 4096);
    assert_eq!(
        calls[0].model_parameters.context_window, None,
        "no window is made up for it"
    );
}

#[tokio::test]
async fn ask_output_policy_changes_the_reserve_and_an_impossible_window_refuses_before_generation()
{
    // EN browser: set Ask output to 1024 > Ask > send a short question > lower
    // its context window to 1024 > Send; the visible reason replaces Ready.
    // 中文浏览器：将 Ask 输出上限设为 1024 > 提问 > 发送简短问题 > 将上下文上限降至
    // 1024 > 发送；显示无法容纳的原因，不以就绪提示覆盖。
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
    assert_eq!(
        error["code"], "ask_context_too_long",
        "a window the answer's most would fill derives the answer from it and refuses only what cannot fit"
    );
    assert_eq!(error["model"], MODEL);
    let events: Vec<Json> =
        sqlx::query_scalar("SELECT body FROM ask_events WHERE thread_id=$1 ORDER BY seq")
            .bind(made.conversation_id)
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "error" && event["code"] == "ask_context_too_long")
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
    setting(&s, "ask.context_tokens", json!(window_leaving(100))).await;
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
    setting(
        &s,
        "ask.context_tokens",
        json!(window_leaving(
            orch_engine::ask::context::estimate(MODEL, &first.input).unwrap() + 1000
        )),
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

#[tokio::test]
async fn a_model_that_starts_with_live_web_search_is_asked_without_a_search_option_and_answers() {
    // EN browser: Providers > a model whose default is live web search > Ask > send a question;
    // it is answered, and the request names no search of its own.
    // 中文浏览器：提供方中默认开启实时联网搜索的模型 > Ask > 提问；得到回答，请求不选择联网搜索。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    sqlx::query("UPDATE provider_models SET parameters=$3 WHERE provider_id=$1 AND model_name=$2")
        .bind(provider)
        .bind(MODEL)
        .bind(json!({"context_window":200000,"web_search":["live"],"default_web_search":"live"}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let requests = recorder(&s);
    let made = create_unseeded(&s, &s.cai).await;
    let receipt = send(
        &s,
        made.conversation_id,
        "search-default",
        "A first question".into(),
    )
    .await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None, "the turn is answered");
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "the model was asked");
    let params = calls[0].params.clone().unwrap_or(Json::Null);
    assert_eq!(
        params["web_search"]["mode"], "disabled",
        "the model's default search never reaches Ask's own tools: {params}"
    );
}

#[tokio::test]
async fn on_a_model_with_a_256000_window_a_question_is_answered_with_every_setting_as_shipped() {
    // EN browser: Providers > a model with a 256,000-token window > Ask > send a question; it is answered.
    // 中文浏览器：提供方中窗口为 256,000 令牌的模型 > Ask > 提问；得到回答。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    sqlx::query("UPDATE provider_models SET parameters=$3 WHERE provider_id=$1 AND model_name=$2")
        .bind(provider)
        .bind(MODEL)
        .bind(json!({"context_window":256000}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let requests = recorder(&s);
    let made = create_unseeded(&s, &s.cai).await;
    let receipt = send(
        &s,
        made.conversation_id,
        "large-window",
        "A first question".into(),
    )
    .await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None, "the turn is answered");
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "the model was asked once");
    assert_eq!(calls[0].model_parameters.context_window, Some(256000));
    assert_eq!(calls[0].input.max_output_tokens, 4096);
}

/// The model reads the guide first, then answers; every request is kept.
fn reads_the_guide_then_answers(s: &Scene) -> Arc<Mutex<Vec<ProviderGenerate>>> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation")
            };
            let read = request
                .input
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .any(|part| matches!(part, GenerationPart::ToolResult { .. }));
            seen.lock().unwrap().push(request.clone());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: Json::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![if read {
                            GenerationPart::Text {
                                text: "The guide says so.".into(),
                            }
                        } else {
                            GenerationPart::ToolCall {
                                id: "guide".into(),
                                name: "docs".into(),
                                arguments: json!({"query":"ask.models"}),
                            }
                        }],
                        continuation: vec![],
                    },
                    stop: if read {
                        GenerationStop::Complete
                    } else {
                        GenerationStop::Tools
                    },
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    requests
}

#[tokio::test]
async fn a_tool_result_larger_than_the_room_left_is_cut_with_its_line_and_the_answer_still_comes() {
    // EN browser: Ask > a question whose answer reads the guide, in a window that holds the question and
    // a little more > the answer comes, and the source it read shows that it was cut and where the whole is.
    // 中文浏览器：Ask > 提问，回答需要读取指南，窗口只比问题略大 > 回答照常给出，所读来源标明已截短及完整来源所在。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let question = "What does the guide say about the Ask models?";
    // The same question in an empty conversation, to measure the request it makes.
    let measured = recorder(&s);
    let probe = create_unseeded(&s, &s.cai).await;
    send(&s, probe.conversation_id, "measure", question.into()).await;
    let used =
        orch_engine::ask::context::estimate(MODEL, &measured.lock().unwrap()[0].input).unwrap();
    // The window leaves 400 tokens after that request; the guide's pages come to many times that.
    let window = window_leaving(used + 400);
    setting(&s, "ask.context_tokens", json!(window)).await;
    let requests = reads_the_guide_then_answers(&s);
    let made = create_unseeded(&s, &s.cai).await;
    let receipt = send(&s, made.conversation_id, "cut", question.into()).await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None, "the answer comes");
    let calls = requests.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "one step to read, one to answer");
    let (read, is_error) = calls[1]
        .input
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|part| match part {
            GenerationPart::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .expect("the answer step carries what the guide gave");
    assert!(!is_error);
    assert!(
        read.contains("Result shortened to fit the context") && read.contains("/help/"),
        "the cut says that it is one and where the whole source is: {read}"
    );
    let answer = (window / 2).min(4096);
    assert!(
        orch_engine::ask::context::estimate(MODEL, &calls[1].input).unwrap() <= window - answer,
        "the request that carries the cut fits the window less the answer"
    );
    assert!(
        read.len() < 12_000,
        "what the window left is a few hundred tokens, not the five pages"
    );
}

#[tokio::test]
async fn a_topic_opened_by_reference_reaches_the_next_step_as_it_was_first_given() {
    // EN browser: Ask > a question whose answer opens a topic > the answer comes from what the open gave,
    // as it was first given: a tool result, neither shortened nor turned into plain text.
    // 中文浏览器：Ask > 提问，回答需要打开话题 > 回答基于所打开的内容，与首次给出的完全相同：
    // 仍是工具结果，既不被截短，也不被改成纯文本。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let seen = super::admin_mode::reads_then_answers(&s);
    let made = create_unseeded(&s, &s.ada).await;
    send_as(
        &s,
        &s.ada,
        made.conversation_id,
        "open",
        "What is this topic?".into(),
    )
    .await;
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "one step to open, one to answer");
    let (given, is_error) = calls[1]
        .input
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|part| match part {
            GenerationPart::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .expect("the answer step carries what the open gave as a tool result");
    assert!(!is_error, "the topic opens: {given}");
    let first: String = sqlx::query_scalar(
        "SELECT payload#>>'{tool,result}' FROM items WHERE thread_id=$1 AND kind='tool_call'",
    )
    .bind(made.conversation_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        given, first,
        "a source that has not changed is given again as it was first given"
    );
}

#[tokio::test]
async fn a_conversation_started_on_a_page_keeps_its_reads_for_the_next_question() {
    // EN browser: topic > Ask > let the answer open, search and read > ask what those sources said;
    // every step and the follow-up still receive the checked exchanges, within the model's window.
    // 中文浏览器：话题 > Ask > 回答打开、搜索并读取来源 > 追问这些来源说了什么；
    // 每一步及后续问题仍在模型窗口内收到已核验的工具交互。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let topic = s.tree.topic.base.id;
    let reads = [
        (
            "opened-topic",
            "open",
            json!({"reference":format!("topic:{topic}")}),
        ),
        (
            "searched-topic",
            "search",
            json!({"query":s.tree.topic.base.name,"place":topic}),
        ),
        (
            "read-topic",
            "read",
            json!({"operation":"NodeGet.read","parameters":{"path":{"id":topic}}}),
        ),
    ];
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
            let index = kept.len();
            kept.push(request.clone());
            let content = match index {
                0 => reads
                    .iter()
                    .map(|(id, name, arguments)| GenerationPart::ToolCall {
                        id: (*id).into(),
                        name: (*name).into(),
                        arguments: arguments.clone(),
                    })
                    .collect(),
                1 => vec![GenerationPart::ToolCall {
                    id: "catalogue".into(),
                    name: "find_operation".into(),
                    arguments: json!({"query":"NodeGet"}),
                }],
                _ => vec![GenerationPart::Text {
                    text: "The topic sources are still available for the next question.".into(),
                }],
            };
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: Json::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content,
                        continuation: vec![],
                    },
                    stop: if index < 2 {
                        GenerationStop::Tools
                    } else {
                        GenerationStop::Complete
                    },
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create(&s, &s.ada, None).await;
    for (client, question) in [
        (
            "read-from-page",
            "Open, search and read this topic, then find its read operation.",
        ),
        (
            "refer-to-reads",
            "What did the sources you opened, searched and read for my previous question say?",
        ),
    ] {
        let receipt = send_as(&s, &s.ada, made.conversation_id, client, question.into()).await;
        let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
            .bind(receipt.turn_id.unwrap())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(error, None, "each question is answered");
    }
    let calls = seen.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        4,
        "two reads, an answer and a follow-up answer"
    );
    for (step, request) in calls.iter().enumerate().skip(1) {
        assert!(has_text(request, "Page in view"));
        let expected = if step == 1 { 3 } else { 4 };
        let results: Vec<_> = request
            .input
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|part| match part {
                GenerationPart::ToolResult {
                    call_id,
                    content,
                    is_error,
                    witness,
                } => Some((call_id, content, is_error, witness)),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), expected, "step {step} keeps each result");
        for (id, content, is_error, witness) in results {
            assert!(!is_error, "step {step}, {id}: {content}");
            assert!(witness.is_some(), "{id} retains its source identity");
            assert!(request.input.messages.iter().flat_map(|message| &message.content).any(
                |part| matches!(part, GenerationPart::ToolCall { id: call, .. } if call == id)
            ), "step {step} keeps the request paired with {id}");
            if matches!(id.as_str(), "opened-topic" | "read-topic") {
                assert!(content.contains(&topic.to_string()), "{id}: {content}");
            }
        }
        let budget = orch_engine::ask::context::Budget::new(
            &request.model_parameters,
            &request.model_name,
            None,
            request.input.max_output_tokens,
        );
        assert!(budget.fits(&request.input).unwrap(), "step {step} fits");
    }
}

#[tokio::test]
async fn a_page_keeps_its_current_tool_thinking_and_rechecks_sources_for_the_follow_up() {
    // EN browser: topic > Ask > open the topic > follow up. This turn keeps its tool thinking;
    // the later question receives the readable source without hidden earlier source state.
    // 中文浏览器：话题 > Ask > 打开话题 > 追问；当前轮次保留工具推理，
    // 后续问题取得可读来源，不重放此前来源的隐藏状态。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let topic = s.tree.topic.base.id;
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
            answer.output.message.continuation = vec![json!({"opaque":"synthetic-page-thinking"})];
            if first {
                answer.output.message.content = vec![GenerationPart::ToolCall {
                    id: "page-topic".into(),
                    name: "open".into(),
                    arguments: json!({"reference":format!("topic:{topic}")}),
                }];
                answer.output.stop = GenerationStop::Tools;
            }
            Ok(CommandResult::ProviderGenerate(answer))
        });
    let made = create(&s, &s.ada, None).await;
    send_as(
        &s,
        &s.ada,
        made.conversation_id,
        "open-page",
        "Open this topic.".into(),
    )
    .await;
    send_as(
        &s,
        &s.ada,
        made.conversation_id,
        "follow-page",
        "What did you just open?".into(),
    )
    .await;
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 3, "a read, its answer and a follow-up answer");
    assert!(
        calls[1]
            .input
            .messages
            .iter()
            .any(|message| !message.continuation.is_empty()),
        "the current tool round trip keeps its own thinking"
    );
    assert!(calls[1].input.messages.iter().flat_map(|message| &message.content).any(|part|
        matches!(part, GenerationPart::ToolResult { content, is_error: false, .. } if content.contains(&topic.to_string()))
    ), "the current round trip keeps its checked tool result");
    assert!(
        calls[2]
            .input
            .messages
            .iter()
            .all(|message| message.continuation.is_empty()),
        "the follow-up cannot replay hidden earlier source material"
    );
    assert!(
        has_text(&calls[2], &topic.to_string()),
        "the refreshed readable source still reaches the follow-up"
    );
}

#[tokio::test]
async fn a_page_that_fills_the_window_gives_room_back_to_a_completed_read() {
    let s = Scene::new().await;
    configured_provider(&s).await;
    let question = "Find the reader for this absent operation.";
    let measured = recorder(&s);
    let probe = create(&s, &s.ada, None).await;
    send_as(
        &s,
        &s.ada,
        probe.conversation_id,
        "measure-page",
        question.into(),
    )
    .await;
    let used =
        orch_engine::ask::context::estimate(MODEL, &measured.lock().unwrap()[0].input).unwrap();
    setting(&s, "ask.context_tokens", json!(window_leaving(used))).await;
    let requests = Arc::new(Mutex::new(Vec::<ProviderGenerate>::new()));
    let seen = requests.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation")
            };
            let mut seen = seen.lock().unwrap();
            let first = seen.is_empty();
            seen.push(request.clone());
            let CommandResult::ProviderGenerate(mut answer) = simple_answer() else {
                unreachable!()
            };
            if first {
                answer.output.message.content = vec![GenerationPart::ToolCall {
                    id: "absent-reader".into(),
                    name: "find_operation".into(),
                    arguments: json!({"query":"NoSuchReadOperation"}),
                }];
                answer.output.stop = GenerationStop::Tools;
            }
            Ok(CommandResult::ProviderGenerate(answer))
        });
    let made = create(&s, &s.ada, None).await;
    let receipt = send_as(
        &s,
        &s.ada,
        made.conversation_id,
        "read-with-page",
        question.into(),
    )
    .await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        error, None,
        "the optional page yields room for the completed read"
    );
    let calls = requests.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let given = calls[1]
        .input
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|part| match part {
            GenerationPart::ToolResult {
                content,
                is_error: false,
                ..
            } => Some(content),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Json>(given).unwrap(),
        json!({"operations":[],"citations":[]}),
        "the whole small result reaches the model instead of just a shortening line"
    );
    assert!(orch_engine::ask::context::estimate(MODEL, &calls[1].input).unwrap() <= used);
}

#[tokio::test]
async fn a_question_whose_reading_leaves_no_room_is_refused_with_what_a_person_can_do() {
    // EN browser: Ask > a question whose answer reads the guide, in a window that holds the question and
    // hardly more > the reason says that the question and what it read are too long for the model, and
    // to ask a narrower question or start with fewer sources.
    // 中文浏览器：Ask > 提问，回答需要读取指南，窗口几乎只容得下问题 > 原因说明问题及其所读内容超出模型容量，
    // 并建议缩小问题或从更少的来源开始。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let question = "What does the guide say about the Ask models?";
    // The same question in an empty conversation, to measure the request it makes.
    let measured = recorder(&s);
    let probe = create_unseeded(&s, &s.cai).await;
    send(&s, probe.conversation_id, "measure", question.into()).await;
    let used =
        orch_engine::ask::context::estimate(MODEL, &measured.lock().unwrap()[0].input).unwrap();
    // The window leaves 20 tokens after that request; the reply that asks for the read takes them.
    setting(&s, "ask.context_tokens", json!(window_leaving(used + 20))).await;
    let requests = reads_the_guide_then_answers(&s);
    let made = create_unseeded(&s, &s.cai).await;
    let refused = send(&s, made.conversation_id, "no-room", question.into()).await;
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "the step that would carry the read is not sent"
    );
    let error: Json = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(refused.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error["code"], "ask_turn_too_long");
    assert_eq!(error["model"], MODEL);
    let reason = error["message"].as_str().unwrap();
    assert!(
        reason.contains(MODEL)
            && reason.contains("narrower question")
            && reason.contains("fewer sources"),
        "the reason says what a person can do: {reason}"
    );
}

#[tokio::test]
async fn an_unknown_window_reduces_the_answer_allowance_without_refusing_a_question_that_fits() {
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    sqlx::query(
        "UPDATE provider_models SET parameters='{}' WHERE provider_id=$1 AND model_name=$2",
    )
    .bind(provider)
    .bind(MODEL)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let measured = recorder(&s);
    let probe = create_unseeded(&s, &s.cai).await;
    let question = "Answer this short question.";
    send(&s, probe.conversation_id, "measure", question.into()).await;
    let prompt =
        orch_engine::ask::context::estimate(MODEL, &measured.lock().unwrap()[0].input).unwrap();
    setting(&s, "ask.max_output_tokens", json!(prompt * 2)).await;
    // Halving a positive i64 answer allowance takes at most its number of value bits.
    setting(&s, "ask.max_steps", json!(i64::BITS)).await;
    let requests = Arc::new(Mutex::new(Vec::<ProviderGenerate>::new()));
    let seen = requests.clone();
    let actual_window = Arc::new(Mutex::new(None));
    let enforced = actual_window.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation")
            };
            let input = orch_engine::ask::context::estimate(MODEL, &request.input).unwrap();
            let mut window = enforced.lock().unwrap();
            let window = *window.get_or_insert(input * 2);
            seen.lock().unwrap().push(request.clone());
            if input + request.input.max_output_tokens > window {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "The request and answer exceed the model window",
                )
                .with_details(
                    json!({"failure":{"code":"context_overflow","status":400,"retryable":false}}),
                ));
            }
            Ok(simple_answer())
        });
    let made = create_unseeded(&s, &s.cai).await;
    let receipt = send(&s, made.conversation_id, "answer-fits", question.into()).await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        error, None,
        "the question fits once the answer allowance is reduced"
    );
    let calls = requests.lock().unwrap();
    assert!(
        calls.len() > 1,
        "the actual combined window refuses the initial allowance"
    );
    let last = calls.last().unwrap();
    assert!(
        !last.input.tools.is_empty(),
        "no earlier history needs a summary"
    );
    assert!(
        orch_engine::ask::context::estimate(MODEL, &last.input).unwrap()
            + last.input.max_output_tokens
            <= actual_window.lock().unwrap().unwrap()
    );
    assert!(has_text(last, question), "the original question is intact");
}

#[tokio::test]
async fn a_providers_context_too_long_answer_compacts_the_conversation_once_and_the_question_is_answered()
 {
    // EN browser: a model whose window is not recorded > a long question, then another > the provider
    // says the second is too long > earlier messages are summarized and the answer comes.
    // 中文浏览器：未记录窗口的模型 > 先后发送两个长问题 > 提供方说第二个过长 > 较早消息被归纳，回答照常给出。
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    sqlx::query(
        "UPDATE provider_models SET parameters='{}' WHERE provider_id=$1 AND model_name=$2",
    )
    .bind(provider)
    .bind(MODEL)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let requests = Arc::new(Mutex::new(Vec::<ProviderGenerate>::new()));
    let seen = requests.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation")
            };
            let mut seen = seen.lock().unwrap();
            seen.push(request.clone());
            // The provider enforces one actual window on every request, including retries and summaries.
            let window = orch_engine::ask::context::estimate(MODEL, &seen[0].input).unwrap()
                + seen[0].input.max_output_tokens;
            if orch_engine::ask::context::estimate(MODEL, &request.input).unwrap()
                + request.input.max_output_tokens
                > window
            {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "the generation upstream answered 400",
                )
                .with_details(
                    json!({"failure":{"code":"context_overflow","status":400,"retryable":false}}),
                ));
            }
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: Json::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: if request.input.tools.is_empty() {
                                "The earlier question asked to retain the original project details."
                                    .into()
                            } else {
                                "A short answer.".into()
                            },
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create_unseeded(&s, &s.cai).await;
    let long = format!("earliest project detail {}", "x".repeat(40_000));
    send(&s, made.conversation_id, "first", long.clone()).await;
    let second = send(&s, made.conversation_id, "second", long.clone()).await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(second.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None, "the second question is answered");
    let calls = requests.lock().unwrap().clone();
    let summary = calls
        .iter()
        .find(|call| call.input.tools.is_empty())
        .unwrap();
    assert!(has_text(summary, &long));
    let answer = calls.last().unwrap();
    assert!(has_text(answer, "Earlier messages summarized"));
    let asked = answer
        .input
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|part| matches!(part, GenerationPart::Text { text } if text.contains("earliest project detail")))
        .count();
    assert_eq!(
        asked, 1,
        "only the new question is left in full; the first is summarized"
    );
    let snapshot = read(&s, &s.cai, made.conversation_id).await;
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| message.summary.is_some() && message.complete)
            .count(),
        1
    );
}
