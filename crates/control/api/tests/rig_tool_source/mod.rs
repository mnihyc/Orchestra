//! Trusted tool-consumer fixtures still use one actual authored/admitted native source.
//! These helpers do not claim listener/native-ingress proof; dedicated connector cases do.

use crate::rig;
use orch_api_contract::threads::Delivery;
use orch_link_contract::commands::{CommandError, CommandKind, CommandResult, ErrorCode};
use orch_types::Id;
use serde_json::json;

type Proof<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub async fn active(scene: &rig::Scene, thread: Id) -> Proof<Id> {
    if let Some(turn) =
        sqlx::query_scalar::<_, Option<Id>>("SELECT current_turn_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await?
    {
        rig::eventually("the existing actual tool turn is running on its recorded native segment",||async {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM threads h JOIN turns u ON u.id=h.current_turn_id
                AND u.segment_id=h.current_segment_id AND u.thread_id=h.node_id JOIN segments s ON s.id=u.segment_id
                WHERE h.node_id=$1 AND u.id=$2 AND u.state='running' AND s.native_session_ref IS NOT NULL)")
                .bind(thread).bind(turn).fetch_one(scene.rig.db.pool()).await.unwrap_or(false)
        }).await;
        return Ok(turn);
    }
    let mut descriptor: serde_json::Value =
        sqlx::query_scalar("SELECT descriptor FROM agents WHERE id=$1")
            .bind(scene.on.agent)
            .fetch_one(scene.rig.db.pool())
            .await?;
    descriptor["capabilities"]["orch_tools"] = json!("mcp");
    descriptor["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":false,"strict":true});
    let typed: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor.clone())?;
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(scene.on.agent)
        .bind(descriptor)
        .execute(scene.rig.db.pool())
        .await?;
    let agent = scene.on.agent;
    let provider = scene.on.provider;
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::AgentStart, move |command| {
            let orch_link_contract::commands::Command::AgentStart(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong fixture command",
                ));
            };
            if body.agent_id != agent || body.provider_id != provider {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "another fixture agent or provider was requested",
                ));
            }
            Ok(CommandResult::AgentStart(
                orch_link_contract::commands::AgentStartResult {
                    reused: false,
                    harness_version: "1.0.0".into(),
                    descriptor: typed.clone(),
                },
            ))
        });
    let nth = scene.rig.turn_starts(thread).len();
    scene
        .say(
            &scene.ada,
            thread,
            "Exercise the tools on this explicitly authored fixture turn",
            Delivery::Queue,
            &format!("tool-fixture-source-{thread}-{nth}"),
        )
        .await
        .ok();
    let turn = scene.rig.turn_start(thread, nth).await;
    scene
        .rig
        .answer(
            thread,
            &turn,
            &format!("tool-fixture-source-{nth}"),
            "Working on this authored input",
        )
        .await;
    scene.rig.wait_turn(turn.turn_id, "running").await;
    Ok(turn.turn_id)
}
