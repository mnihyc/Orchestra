//! A refused Ask says why, in a reason of its own, whatever the generic sentence of its status.
use super::*;

async fn ask(
    s: &Scene,
    who: &places::Who,
    id: Id,
    client: &str,
) -> orch_api_core::testing::Reply<AskReceipt> {
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&id])
        .token(&who.token)
        .body(&AskMessageBody {
            client_id: client.into(),
            content: vec![ContentPart::Text {
                text: "A question".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
}

#[tokio::test]
async fn a_question_refused_for_a_usage_limit_names_the_limit_as_its_reason() {
    // EN browser: reach a personal Ask limit > Send; the panel says that a usage limit was
    // reached, not the generic "You do not have access to this".
    // 中文浏览器：达到个人 Ask 额度 > 发送；面板说明已达用量上限，而不是笼统的“你没有这项权限”。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let made = create_unseeded(&s, &s.cai).await;
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,uncounted,created_by) VALUES($1,$2,'user',$3,'shared','tokens','day',0,ARRAY['ask'],'allow',$4)")
        .bind(new_id()).bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    let refused = ask(&s, &s.cai, made.conversation_id, "capped").await;
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(refused.problem().extra["reason"], "cap_reached");
}

#[tokio::test]
async fn a_question_asked_after_leaving_the_organization_names_the_organization_as_its_reason() {
    // EN browser: leave the organization in another tab > Send in Ask; the panel says that the
    // organization can no longer be opened.
    // 中文浏览器：在另一个标签页退出组织 > 在 Ask 中发送；面板说明无法再打开该组织。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let made = create_unseeded(&s, &s.cai).await;
    leave_root(&s, &s.cai).await;
    let refused = ask(&s, &s.cai, made.conversation_id, "left").await;
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(refused.problem().extra["reason"], "root_refused");
}

#[tokio::test]
async fn a_running_question_keeps_its_specific_reason_when_provider_availability_or_use_changes() {
    for reason in ["keeper_offline", "no_use"] {
        let s = Scene::new().await;
        let provider = configured_provider(&s).await;
        s.rig.machine.handler.hold(CommandKind::ProviderGenerate);
        s.rig
            .machine
            .handler
            .on(CommandKind::ProviderGenerate, |_| {
                let CommandResult::ProviderGenerate(mut answer) = simple_answer() else {
                    unreachable!()
                };
                answer.output.message.content = vec![GenerationPart::ToolCall {
                    id: "before-offline".into(),
                    name: "docs".into(),
                    arguments: json!({"query":"Ask"}),
                }];
                answer.output.stop = GenerationStop::Tools;
                Ok(CommandResult::ProviderGenerate(answer))
            });
        let made = create(&s, &s.cai, None).await;
        let receipt = ask(&s, &s.cai, made.conversation_id, "keeper-goes-offline")
            .await
            .ok();
        eventually("the model's admitted step starts", || async {
            s.rig
                .machine
                .handler
                .executions()
                .iter()
                .any(|run| run.kind == CommandKind::ProviderGenerate)
        })
        .await;
        if reason == "keeper_offline" {
            sqlx::query("UPDATE machines SET online=false WHERE id=$1")
                .bind(s.rig.machine_id)
                .execute(s.rig.db.pool())
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_kind='provider' AND object_id=$1 AND permission='provider.use' AND holder_kind='user' AND holder_id=$2")
            .bind(provider).bind(s.cai.id()).bind(s.rig.world.now()).execute(s.rig.db.pool()).await.unwrap();
        }
        s.rig.machine.handler.release();
        eventually("the current question settles with its refusal", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
        let error: Json = sqlx::query_scalar("SELECT error FROM turns WHERE id=$1")
            .bind(receipt.turn_id.unwrap())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(error["code"], format!("ask.{reason}"));
        assert!(!error["message"].as_str().unwrap().contains("authz.denied:"));
        let events: Vec<Json> = sqlx::query_scalar(
            "SELECT body FROM ask_events WHERE thread_id=$1 AND body->>'kind'='error'",
        )
        .bind(made.conversation_id)
        .fetch_all(s.rig.db.pool())
        .await
        .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event["code"] == format!("ask.{reason}")),
            "the stream carries the same specific reason"
        );
    }
}

#[tokio::test]
async fn a_model_that_is_not_on_the_list_is_refused_in_plain_words() {
    // EN browser: choose a model that the organization has taken off Ask's list > Send; the panel
    // says that the model is not on the list, in words and without a setting's name.
    // 中文浏览器：选择组织已从 Ask 列表中移除的模型 > 发送；面板用文字说明该模型不在列表中，不出现设置名称。
    let s = Scene::new().await;
    configured_provider(&s).await;
    let made = create_unseeded(&s, &s.cai).await;
    let refused = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "unlisted".into(),
            content: vec![ContentPart::Text {
                text: "A question".into(),
            }],
            context: None,
            model: Some(Some(AskModel {
                provider_id: new_id(),
                model: "a-model-nobody-listed".into(),
                params: None,
            })),
        })
        .send()
        .await;
    assert!(refused.status.is_client_error());
    let problem = refused.problem();
    let message = problem.extra["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("Ask list") && !message.contains("ask.models"),
        "the refusal reads as words: {message}"
    );
}
