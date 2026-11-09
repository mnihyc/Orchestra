//! Ordinary tools reach the same actual Read operations under a native owner origin.

mod places;
mod rig;
mod rig_tool_source;
mod support;

use orch_agent_contract::events::OrchTool;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::nodes::NodeGet;
use orch_engine::commands::Commands;
use orch_engine::tools::CallContext;
use orch_settings::Target;
use orch_types::{Id, Json};
use serde_json::json;

async fn setup() -> (rig::Scene, Id, Id) {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread])
        .token(&scene.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    scene
        .rig
        .world
        .set_at(Target::Node(thread), "orch_tools.owner_reads", json!(true))
        .await;
    let turn = match rig_tool_source::active(&scene, thread).await {
        Ok(turn) => turn,
        Err(error) => panic!("the real native turn must be admitted: {error:?}"),
    };
    (scene, thread, turn)
}

async fn call(scene: &rig::Scene, thread: Id, turn: Id, reference: &str, target: Id) -> Json {
    let call = CallContext {
        machine: scene.rig.machine_id,
        thread,
        turn,
        attempt: None,
        call_ref: reference.into(),
        tool: OrchTool::Read,
        arguments: json!({"operation":"NodeGet.read","parameters":{"path":{"id":target}}}),
    };
    if let Err(error) = scene.rig.engine.answer_tool_call(&call).await {
        panic!("the genuine tool source must answer: {error:?}");
    }
    match scene
        .rig
        .commands
        .retained_body(orch_engine::ids::tool_result(thread, reference))
        .await
    {
        Ok(Some(body)) => body,
        other => panic!("the actual native result must be retained: {other:?}"),
    }
}

#[tokio::test]
async fn native_owner_reads_use_the_actual_catalogue_and_keep_the_fixed_root() {
    // EN browser: private native thread > enable Personal read tools > ask it
    // to read this topic; follow the citation. Another owned root remains refused.
    // 中文浏览器：私有原生会话 > 启用个人读取工具 > 要求读取本话题并打开引用；
    // 即使拥有另一个根组织，也不得在本会话读取它。
    let (scene, thread, turn) = setup().await;
    let result = call(
        &scene,
        thread,
        turn,
        "native-own-topic",
        scene.tree.topic.base.id,
    )
    .await;
    assert!(result.get("error").is_none_or(Json::is_null), "{result}");
    assert_eq!(
        result["result"]["data"]["id"],
        json!(scene.tree.topic.base.id)
    );
    assert!(
        result["result"]["citations"]
            .as_array()
            .is_some_and(|citations| !citations.is_empty())
    );
    let other = places::make_org(
        &scene.rig.world,
        &scene.ada,
        "Other knowledge",
        "other-knowledge",
    )
    .await;
    let refused = call(&scene, thread, turn, "native-other-root", other.base.id).await;
    assert!(
        refused.get("error").is_some_and(|error| !error.is_null()),
        "{refused}"
    );
    assert!(refused.get("result").is_none_or(Json::is_null));
}

#[tokio::test]
async fn native_owner_origin_cannot_be_claimed_by_headers_or_used_after_a_new_grant() {
    // EN: ordinary HTTP without a credential cannot claim the native owner via
    // a header; add another viewer, then request a NEW personal read: use shared tools.
    // 中文：普通 HTTP 请求不能用请求头伪装原生所有者；新增查看者后，再发起新的
    // 个人读取必须被拒绝，并提示使用共享读取工具。
    let (scene, thread, turn) = setup().await;
    let http = scene
        .rig
        .world
        .app
        .call(NodeGet, &[&scene.tree.topic.base.id])
        .header("X-Orch-Owner-Read", &scene.ada.id().to_string())
        .send()
        .await;
    assert_eq!(http.status.as_u16(), 401);
    let before = call(
        &scene,
        thread,
        turn,
        "native-before-share",
        scene.tree.topic.base.id,
    )
    .await;
    assert!(before.get("error").is_none_or(Json::is_null), "{before}");
    scene
        .rig
        .lets_use("node", thread, "thread.view", &scene.bea, json!({}))
        .await;
    let after = call(
        &scene,
        thread,
        turn,
        "native-after-share",
        scene.tree.topic.base.id,
    )
    .await;
    assert_eq!(after["error"]["code"], "not_yours", "{after}");
    assert!(
        after["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("read_shared"))
    );
    assert!(after.get("result").is_none_or(Json::is_null));
}
