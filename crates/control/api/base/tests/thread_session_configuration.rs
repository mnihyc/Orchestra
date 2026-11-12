//! Actual setting writes refresh session-only cwd and inherited notes exactly once without changing native identity or core/tool bytes.

mod places;
mod rig;
mod support;

use orch_api_contract::{
    model_inputs::{ModelInputListQuery, ModelInputPurpose, NodeModelInputList},
    settings::{NodeSettingDelete, NodeSettingDeleteQuery, NodeSettingSet, NodeSettingSetBody},
};
use orch_link_contract::commands::{Command, SessionMode, SessionOpen};
use orch_types::{Id, Json};
use serde_json::json;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn opens(scene: &rig::Scene, thread: Id) -> Vec<SessionOpen> {
    scene
        .rig
        .seen
        .all()
        .into_iter()
        .filter_map(|command| match command {
            Command::SessionOpen(open) if open.thread_id == thread => Some(open),
            _ => None,
        })
        .collect()
}

async fn setting(scene: &rig::Scene, node: Id, key: &str, value: Json) {
    scene
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &key])
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            value,
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

async fn stable_bytes(scene: &rig::Scene, thread: Id) -> Proof {
    let body = opens(scene, thread);
    let first = body.first().ok_or("initial native opening missing")?;
    assert!(
        body.iter()
            .all(|open| open.instructions == first.instructions
                && open.orch_tools == first.orch_tools)
    );
    let page = scene
        .rig
        .world
        .app
        .call(NodeModelInputList, &[&thread])
        .token(&scene.ada.token)
        .query(&ModelInputListQuery {
            limit: Some(200),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let tools: Vec<_> = page
        .items
        .iter()
        .filter(|input| input.purpose == ModelInputPurpose::Tool)
        .collect();
    assert_eq!(tools.len(), body.len());
    let original = tools
        .first()
        .ok_or("actual offered tool bytes were not recorded")?;
    assert!(
        tools
            .iter()
            .all(|offered| offered.blocks == original.blocks),
        "cwd/notes cannot silently change offered definitions"
    );
    Ok(())
}

#[tokio::test]
async fn an_actual_start_directory_write_resumes_the_same_conversation_once_and_sends_the_new_context()
-> Proof {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "First real input", "First answer")
        .await;
    scene
        .exchange(thread, 1, "Unchanged setting control", "Second answer")
        .await;
    let first = opens(&scene, thread);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].mode, SessionMode::New);
    let native: String = sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE id=$1")
        .bind(first[0].segment_id)
        .fetch_one(scene.rig.db.pool())
        .await?;
    setting(&scene, thread, "thread.cwd", json!("child")).await;
    assert_eq!(
        opens(&scene, thread).len(),
        1,
        "the setting write itself does no native work"
    );
    scene
        .exchange(thread, 2, "Use the changed start directory", "Third answer")
        .await;
    let after = opens(&scene, thread);
    assert_eq!(after.len(), 2);
    assert_eq!(after[1].cwd, format!("{}/child", first[0].directory));
    assert_eq!(
        (
            after[1].segment_id,
            after[1].native_session_ref.as_deref(),
            after[1].mode
        ),
        (
            first[0].segment_id,
            Some(native.as_str()),
            SessionMode::Resume
        )
    );
    let given = scene.rig.turn_starts(thread)[2].text.clone();
    assert!(given.contains(&format!("you start in {}.", after[1].cwd)));
    assert!(
        !given.contains(&format!("you start in {}.", first[0].cwd)),
        "the changed context never claims the old start directory"
    );
    scene
        .exchange(
            thread,
            3,
            "Unchanged new directory control",
            "Fourth answer",
        )
        .await;
    assert_eq!(
        opens(&scene, thread).len(),
        2,
        "an unchanged cwd does not repeatedly reopen the engine session"
    );
    stable_bytes(&scene, thread).await?;
    Ok(())
}

#[tokio::test]
async fn inherited_notes_set_and_clear_reconfigure_the_same_native_reference_without_reopening_unchanged_turns()
-> Proof {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    setting(&scene, scene.tree.org.base.id, "agent.memory", json!("on")).await;
    scene
        .exchange(thread, 0, "Keep the existing notes", "First answer")
        .await;
    let first = opens(&scene, thread);
    assert_eq!(first.len(), 1);
    assert!(first[0].memory.is_some());
    let native: String = sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE id=$1")
        .bind(first[0].segment_id)
        .fetch_one(scene.rig.db.pool())
        .await?;
    // A thread-only invalidation hook cannot cover this inherited change.
    setting(
        &scene,
        scene.tree.topic.base.id,
        "agent.memory",
        json!("off"),
    )
    .await;
    assert_eq!(opens(&scene, thread).len(), 1);
    scene
        .exchange(thread, 1, "Notes are disabled", "Second answer")
        .await;
    let disabled = opens(&scene, thread);
    assert_eq!(disabled.len(), 2);
    assert!(disabled[1].memory.is_none());
    assert_eq!(
        (
            disabled[1].segment_id,
            disabled[1].native_session_ref.as_deref(),
            disabled[1].mode
        ),
        (
            first[0].segment_id,
            Some(native.as_str()),
            SessionMode::Resume
        )
    );
    scene
        .exchange(thread, 2, "Keep the disabled setting", "Third answer")
        .await;
    assert_eq!(opens(&scene, thread).len(), 2);
    scene
        .rig
        .world
        .app
        .call(
            NodeSettingDelete,
            &[&scene.tree.topic.base.id, &"agent.memory"],
        )
        .token(&scene.ada.token)
        .query(&NodeSettingDeleteQuery::default())
        .send()
        .await
        .ok();
    scene
        .exchange(
            thread,
            3,
            "Use the org's original notes setting",
            "Fourth answer",
        )
        .await;
    let enabled = opens(&scene, thread);
    assert_eq!(enabled.len(), 3);
    assert_eq!(enabled[2].memory, first[0].memory);
    assert_eq!(
        enabled[2].native_session_ref.as_deref(),
        Some(native.as_str())
    );
    scene
        .exchange(thread, 4, "Unchanged inherited control", "Fifth answer")
        .await;
    assert_eq!(opens(&scene, thread).len(), 3);
    stable_bytes(&scene, thread).await?;
    Ok(())
}

#[tokio::test]
async fn a_refused_reconfiguration_cannot_acknowledge_the_new_settings_in_the_open_session_cache()
-> Proof {
    use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Original real turn", "Original answer")
        .await;
    let original = opens(&scene, thread);
    assert_eq!(original.len(), 1);
    setting(&scene, thread, "thread.cwd", json!("child")).await;
    scene.rig.seen.refuse(
        CommandKind::SessionOpen,
        CommandError::new(
            ErrorCode::Unsupported,
            "Synthetic native configuration refusal",
        ),
    );
    let refused = scene
        .say(
            &scene.ada,
            thread,
            "Attempt the changed configuration",
            orch_api_contract::threads::Delivery::Queue,
            "refused-session-configuration",
        )
        .await
        .ok()
        .input;
    rig::eventually(
        "the actual refused native opening ends its attempted turn",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM turns WHERE input_id=$1 AND state='failed')",
            )
            .bind(refused.id)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    rig::eventually(
        "the failed start's immutable follow-up settles before the explicit new input",
        || async {
            !orch_db::queries::threads::turn_end_work_pending(
                &mut *scene.rig.world.conn().await,
                thread,
            )
            .await
            .unwrap_or(true)
        },
    )
    .await;
    assert_eq!(opens(&scene, thread).len(), 2);
    assert_eq!(
        scene.rig.turn_starts(thread).len(),
        1,
        "a refused SessionOpen never submits that turn to its model"
    );
    scene
        .exchange(
            thread,
            1,
            "Explicitly authored recovery input",
            "Recovered answer",
        )
        .await;
    let recovered = opens(&scene, thread);
    assert_eq!(
        recovered.len(),
        3,
        "the unacknowledged cwd must be offered again"
    );
    assert_eq!(recovered[2].mode, SessionMode::Resume);
    assert_eq!(recovered[2].segment_id, original[0].segment_id);
    assert_eq!(recovered[2].cwd, recovered[1].cwd);
    assert_eq!(
        recovered[2].native_session_ref,
        recovered[1].native_session_ref
    );
    scene
        .exchange(
            thread,
            2,
            "Unchanged accepted recovery control",
            "Last answer",
        )
        .await;
    assert_eq!(opens(&scene, thread).len(), 3);
    stable_bytes(&scene, thread).await?;
    Ok(())
}
