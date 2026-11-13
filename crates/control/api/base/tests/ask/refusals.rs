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
