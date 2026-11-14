//! Ask answers as the person, whatever the As admin switch of their session says.
use super::context::{has_text, recorder, send_as};
use super::*;
use orch_link_contract::commands::ProviderGenerate;

/// The model reads the topic first, then answers; every request is kept.
pub(super) fn reads_then_answers(s: &Scene) -> Arc<Mutex<Vec<ProviderGenerate>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    let topic = s.tree.topic.base.id;
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
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: Json::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![if first {
                            GenerationPart::ToolCall {
                                id: "topic".into(),
                                name: "open".into(),
                                arguments: json!({"reference":format!("topic:{topic}")}),
                            }
                        } else {
                            GenerationPart::Text {
                                text: "It is the topic of this organization.".into(),
                            }
                        }],
                        continuation: vec![],
                    },
                    stop: if first {
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
    seen
}

#[tokio::test]
async fn a_person_with_ask_only_provider_use_is_offered_the_model_and_receives_an_answer() {
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    let home = places::make_org(
        &s.rig.world,
        &s.bea,
        "Shared provider home",
        "shared-provider-home",
    )
    .await
    .base
    .id;
    sqlx::query("UPDATE providers SET owner_id=$2,org_id=$3,home_id=$3 WHERE id=$1")
        .bind(provider)
        .bind(s.bea.id())
        .bind(home)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE object_kind='provider' AND object_id=$1 AND permission='access.own'")
        .bind(provider).bind(s.rig.world.now()).execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("UPDATE permission_entries SET options=$3 WHERE object_kind='provider' AND object_id=$1 AND permission='provider.use' AND holder_kind='user' AND holder_id=$2")
        .bind(provider).bind(s.cai.id()).bind(json!({"purposes":["ask"]})).execute(s.rig.db.pool()).await.unwrap();
    let seen = recorder(&s);
    let models = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.cai.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        models.blocked_reason, None,
        "Ask-only use offers a usable Ask model"
    );
    let made = create(&s, &s.cai, None).await;
    let receipt = send_as(
        &s,
        &s.cai,
        made.conversation_id,
        "ask-only",
        "Answer with the provider shared for Ask.".into(),
    )
    .await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_organization_admin_with_as_admin_on_asks_reads_a_source_and_is_answered() {
    // EN browser: As admin on > open a topic > Ask > send a question; the model reads the topic as
    // the person and the answer comes, with its history listed and its models offered.
    // 中文浏览器：开启“以管理员身份” > 打开话题 > Ask > 提问；模型以本人身份读取话题并给出回答，
    // 历史和模型列表照常显示。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let seen = reads_then_answers(&s);
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
    assert_eq!(models.blocked_reason, None, "the models are offered");
    let made = create_unseeded(&s, &s.ada).await;
    let receipt = send_as(
        &s,
        &s.ada,
        made.conversation_id,
        "as-admin",
        "What is this topic?".into(),
    )
    .await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None, "the turn is answered");
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "one step to read, one to answer");
    let read = calls[1]
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
        .expect("the answer step carries what the topic read gave");
    assert!(!read.1, "the topic is readable to the person: {}", read.0);
    assert!(
        read.0.contains(&s.tree.topic.base.id.to_string()),
        "what the read gave is the topic itself: {}",
        read.0
    );
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
    assert!(
        listed
            .items
            .iter()
            .any(|row| row.id == made.conversation_id),
        "the conversation is in the person's own history"
    );
}

#[tokio::test]
async fn an_organization_admin_with_as_admin_on_asks_from_a_page_and_the_model_is_given_the_page() {
    // EN browser: As admin on > open a topic > Ask > send a question; the page in view is read as the
    // person and the model is given it.
    // 中文浏览器：开启“以管理员身份” > 打开话题 > Ask > 提问；当前页面以本人身份读取，并交给模型。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let seen = recorder(&s);
    places::admin_mode(&s.rig.world, &s.ada, true).await;
    let made = create(&s, &s.ada, None).await;
    let receipt = send_as(
        &s,
        &s.ada,
        made.conversation_id,
        "page-as-admin",
        "What is this page?".into(),
    )
    .await;
    let error: Option<Json> = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
        .bind(receipt.turn_id.unwrap())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(error, None, "the turn is answered");
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one step answers");
    assert!(
        has_text(&calls[0], "Page in view") && has_text(&calls[0], &s.tree.topic.base.name),
        "the model is given the page in view"
    );
    assert!(
        !has_text(&calls[0], "no longer available to you"),
        "the page is readable to the person"
    );
}

#[tokio::test]
async fn a_question_asked_from_a_page_of_no_organization_is_answered_in_the_conversations_organization()
 {
    // EN browser: the left rail shows a team organization > open Management (As admin on) or Settings
    // > Ask > send a question; the page is context only, and the answer comes in the team organization.
    // 中文浏览器：左栏显示团队组织 > 打开管理（开启“以管理员身份”）或设置 > Ask > 提问；该页面只作上下文，
    // 回答在该团队组织中给出。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let seen = recorder(&s);
    let made = create_unseeded(&s, &s.ada).await;
    for (number, (as_admin, page)) in [(true, "/admin/overview"), (false, "/settings/profile")]
        .into_iter()
        .enumerate()
    {
        places::admin_mode(&s.rig.world, &s.ada, as_admin).await;
        let sent = s
            .rig
            .world
            .app
            .call(AskMessageCreate, &[&made.conversation_id])
            .token(&s.ada.token)
            .body(&AskMessageBody {
                client_id: format!("from-{number}"),
                content: vec![ContentPart::Text {
                    text: "What is this page?".into(),
                }],
                context: Some(page.into()),
                model: None,
            })
            .send()
            .await;
        assert!(
            sent.status.is_success(),
            "a question from {page} is taken: {:?}",
            sent.status
        );
        eventually("the Ask turn settles", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
    }
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "each question is answered");
    assert!(
        has_text(&calls[0], "/admin/overview"),
        "the first answer has its page in view"
    );
    assert!(
        has_text(&calls[1], "/settings/profile"),
        "the second answer has its page in view"
    );
}

#[tokio::test]
async fn a_system_admin_who_is_no_member_is_told_that_ask_answers_as_themselves() {
    // EN browser: As admin on in an organization you are not in > Ask > New conversation; the
    // panel says that Ask answers as you are, and that As admin does not open the organization.
    // 中文浏览器：在你不属于的组织中开启“以管理员身份” > Ask > 新对话；面板说明 Ask 以你本人的身份
    // 回答，“以管理员身份”不会打开该组织。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let sysadmin = places::admin(&s.rig.world, "sysadmin").await;
    let refused = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&sysadmin.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: None,
            model: None,
            retention_days: None,
        })
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(refused.problem().extra["reason"], "root_refused");
}
