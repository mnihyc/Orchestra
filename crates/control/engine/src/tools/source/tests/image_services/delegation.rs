//! Actual native helpers enter through the same recorded tool, creator and paid-command paths.
use super::*;
use orch_access_contract::permissions::Permission;
use orch_db::queries::access::{self, PageKey};
use orch_link_contract::{
    commands::{AgentStartResult, SessionOpenResult},
    foreign::ItemPayload,
};

const NATIVE_MODEL: &str = "fixture-native-images";

async fn declare_program(scene: &Scene, descriptor: Json) -> Proof {
    scene.world.declares(descriptor.clone()).await;
    let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor)?;
    scene
        .world
        .machine
        .handler
        .on(CommandKind::AgentStart, move |_| {
            Ok(CommandResult::AgentStart(AgentStartResult {
                reused: true,
                harness_version: "1.0.0".into(),
                descriptor: descriptor.clone(),
            }))
        });
    Ok(())
}

async fn native_scene() -> Proof<(Scene, Id)> {
    let (scene, provider, _) = native_scene_with_file_failure().await?;
    Ok((scene, provider))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unit_admission_uses_its_real_helper_and_node_and_rechecks_the_original_model_use()
-> Proof {
    let (scene, provider) = native_scene().await?;
    let (node, host) = scene
        .program_node
        .as_ref()
        .ok_or("missing capability node")?;
    publish(&scene, "source-bound-unit", "out/source-bound-unit.png").await;
    let (helper, started) = begin_helper(&scene, "source-bound-unit").await?;
    let content: Json =
        sqlx::query_scalar("SELECT content FROM thread_inputs WHERE thread_id=$1 AND turn_id=$2")
            .bind(helper)
            .bind(started.turn_id)
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(
        Some(started.text.as_str()),
        content[0]["text"].as_str(),
        "the provider receives exactly the frozen request"
    );
    assert!(
        started
            .images
            .iter()
            .all(|image| image.workspace_path.is_none())
    );
    let inherited: i64 = sqlx::query_scalar("SELECT count(*) FROM model_inputs WHERE node_id=$1 AND purpose IN ('session','tool') AND jsonb_array_length(blocks)>0")
        .bind(helper).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(
        inherited, 0,
        "the kept session records describe only instructions and tools actually sent"
    );
    let engine = scene.world.engine();
    let snapshot = {
        let mut conn = scene.world.pool.acquire().await?;
        let crate::sessions::StartLoad::Ready(snapshot) = engine
            .load_turn_snapshot(
                &mut conn,
                helper,
                orch_db::queries::threads::turn_state::RUNNING,
            )
            .await?
        else {
            return Err("no actual owned helper snapshot".into());
        };
        snapshot
    };
    let program = engine
        .prepare_program_unit(&snapshot)
        .await?
        .ok_or("no actual work unit admission")?;
    assert_eq!(program.admission.helper, helper);
    assert_eq!(program.node, node.id);
    let runtime:Id = sqlx::query_scalar("SELECT id FROM runtimes WHERE host_id=$1 AND provider_id=$2 AND machine_id=$3 AND state='running'")
        .bind(host).bind(provider).bind(scene.world.machine_id).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(program.admission.parent, runtime);
    assert_eq!(program.admission.peer, scene.world.machine_id);
    let admission_command = crate::ids::derived(
        "capability.unit.admit",
        &[program.admission.work.as_bytes()],
    );
    assert_eq!(node.machine.handler.runs_of(admission_command), 1);
    let mut different = snapshot.as_ref().clone();
    different.thread.node_id = new_id();
    assert!(engine.prepare_program_unit(&different).await.is_err());
    sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1 AND model_name=$2")
        .bind(provider)
        .bind(NATIVE_MODEL)
        .execute(scene.world.pool.pool())
        .await?;
    assert!(engine.prepare_program_unit(&snapshot).await.is_err());
    assert_eq!(node.machine.handler.runs_of(admission_command), 1);
    scene.end_original().await?;
    common::eventually("the isolated context closes when its work ends",||async {
        let mut conn=match scene.world.pool.acquire().await {Ok(conn)=>conn,Err(_)=>return false};
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM commands WHERE body->>'thread_id'=$1::uuid::text AND kind IN ('session.close','session.close.fenced'))")
            .bind(helper).fetch_one(&mut *conn).await.unwrap_or(false)
    }).await;
    let present: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1)")
        .bind(helper)
        .fetch_one(scene.world.pool.pool())
        .await?;
    assert!(
        present,
        "closing physical work keeps the ordinary helper history"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_placement_without_isolated_networking_explains_the_refusal_before_dispatch()
-> Proof {
    let (scene, provider) = native_scene().await?;
    let (_, host) = scene
        .program_node
        .as_ref()
        .ok_or("missing capability node")?;
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{recorded-unit-driver,program_units}', $2) WHERE id=$1")
        .bind(host).bind(json!({"supported":false,"reason":"Restricted proxy unavailable"}))
        .execute(scene.world.pool.pool()).await?;
    let engine = scene.world.engine();
    let mut conn = scene.world.pool.acquire().await?;
    let actor = Principal::load(
        &mut conn,
        Caller::session(scene.world.ada),
        scene.world.clock.time(),
    )
    .await?;
    let choice = engine.review_capability_backend(&mut conn, &actor, scene.thread, "image_generation",
        serde_json::from_value(json!({"route":"image_generation.delegation","offering":{"provider":provider,"offering":NATIVE_MODEL}}))?).await?;
    assert!(!choice.allowed);
    assert_eq!(
        choice.reason.as_deref(),
        Some("program_unit_network_unavailable")
    );
    drop(conn);
    publish(&scene, "no-unit", "out/no-unit.png").await;
    let reply = Scene::answer(&scene.world, scene.thread, "no-unit", None).await?;
    assert_eq!(
        reply
            .result
            .as_ref()
            .and_then(|body| body["failure"]["reason"]
                .as_str()
                .or_else(|| body["reason"].as_str())),
        Some("program_unit_network_unavailable"),
        "{reply:?}"
    );
    let works: i64 =
        sqlx::query_scalar("SELECT count(*) FROM delegated_program_work WHERE source_answer_id=$1")
            .bind(crate::ids::tool_result(scene.thread, "no-unit"))
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(works, 0);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_only_image_input_is_staged_by_hash_inside_the_helpers_scratch() -> Proof {
    let (scene, _) = native_scene().await?;
    keep_input_image(&scene).await?;
    scene
        .world
        .set(
            scene.world.topic,
            orch_settings_contract::registry::keys::FILES_NATIVE_IMAGE_BYTES,
            json!(0),
        )
        .await;
    let call = "image-in-scratch";
    Scene::publish(&scene.world,&scene.source,scene.start.turn_id,call,None,OrchTool::ImageGeneration,
        json!({"prompt":"Edit the orchard","images":["out/source.png"],"output_paths":["out/edited-in-scratch.png"]})).await;
    let (helper, start) = begin_helper(&scene, call).await?;
    let blob = Hash::of(&bytes()?);
    assert!(
        start.images.is_empty(),
        "the requested file-only policy is preserved"
    );
    assert!(start.text.contains(&format!("/work/inputs/{blob}")));
    assert!(!start.text.contains("out/source.png"));
    let staged:Json = sqlx::query_scalar("SELECT COALESCE(body#>'{program_work,input_files}',body#>'{session,program_work,input_files}')
        FROM commands WHERE serial_key=$1 AND kind IN ('session.open','session.open.fenced') ORDER BY created_at DESC LIMIT 1")
        .bind(orch_link_contract::commands::SerialKey::Thread(helper).wire()).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(staged, json!([blob]));
    image(&scene, helper, start.turn_id, "scratch-output", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, call, None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    scene.end_original().await?;
    Ok(())
}

async fn approved_image_scene(effort: &str) -> Proof<(Scene, Id)> {
    let scene = Scene::with_approved_input().await?;
    let configured = configured_scene(Shape::Compatible, scene).await?;
    let (scene, provider, _) = native_scene_configured(configured).await?;
    let author: Id = sqlx::query_scalar("SELECT author_id FROM thread_inputs WHERE id=$1")
        .bind(scene.input)
        .fetch_one(scene.world.pool.pool())
        .await?;
    assert_ne!(author, scene.world.ada);
    scene
        .world
        .set(
            scene.world.root,
            orch_settings_contract::registry::keys::PROVIDERS_SUBSCRIPTION_SHARING,
            json!(["fake"]),
        )
        .await;
    sqlx::query("UPDATE provider_models SET parameters=$2 WHERE provider_id=$1")
        .bind(provider)
        .bind(json!({"input_images":true,"context_window":10000,
            "efforts":["low","high"],"default_effort":"low"}))
        .execute(scene.world.pool.pool())
        .await?;
    scene
        .world
        .set(
            scene.world.topic,
            orch_settings_contract::registry::keys::AGENT_EFFORT,
            json!("high"),
        )
        .await;
    let mut conn = scene.world.pool.acquire().await?;
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id: new_id(),
            page: &PageKey::new("provider", provider, "live"),
            profile: "any",
            permission: "provider.use",
            holder_kind: "user",
            holder_id: Some(author),
            holder_role: None,
            options: &json!({"efforts":[effort]}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(scene.world.ada),
            at: scene.world.clock.now(),
        },
    )
    .await?;
    access::bump_epoch(&mut conn).await?;
    drop(conn);
    Ok((scene, provider))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_author_cannot_be_charged_for_a_helper_effort_their_entry_refuses() -> Proof {
    let (scene, _) = approved_image_scene("low").await?;
    publish(
        &scene,
        "author-effort-refused",
        "out/author-effort-refused.png",
    )
    .await;
    let answer = Scene::answer(&scene.world, scene.thread, "author-effort-refused", None).await?;
    assert!(
        answer.error.is_some(),
        "the actor's broader Use cannot replace the author's effort limit"
    );
    let turns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM turns t JOIN delegated_program_work w ON w.helper_id=t.thread_id WHERE w.request_id=$1",
    ).bind(crate::ids::tool_result(scene.thread, "author-effort-refused"))
        .fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(
        turns, 0,
        "no paid helper turn is admitted with the refused effort"
    );
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_author_may_use_the_actual_helper_effort_when_the_model_default_is_refused()
-> Proof {
    let (scene, _) = approved_image_scene("high").await?;
    publish(
        &scene,
        "author-effort-allowed",
        "out/author-effort-allowed.png",
    )
    .await;
    let (helper, start) = begin_helper(&scene, "author-effort-allowed").await?;
    assert_eq!(start.effort.as_deref(), Some("high"));
    image(&scene, helper, start.turn_id, "author-effort-image", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, "author-effort-allowed", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_helpers_prompt_keeps_its_source_governors_and_sharing_notice() -> Proof {
    let (scene, _) = native_scene().await?;
    publish(&scene, "copied-image-source", "out/copied-image-source.png").await;
    let (helper, start) = begin_helper(&scene, "copied-image-source").await?;
    let mut conn = scene.world.pool.acquire().await?;
    let markers = orch_db::queries::model_inputs::governors(&mut conn, helper).await?;
    let prompt: Id =
        sqlx::query_scalar("SELECT id FROM model_inputs WHERE turn_id=$1 AND purpose='input'")
            .bind(start.turn_id)
            .fetch_one(&mut *conn)
            .await?;
    assert!(markers.iter().any(|marker| marker.model_input_id == prompt
        && marker.viewer_id.is_none()
        && marker.source_refs == json!([{"kind":"thread","id":scene.thread}])));
    assert_eq!(
        orch_db::queries::capability_work::copied_source_count(&mut conn, helper).await?,
        1
    );
    drop(conn);
    image(&scene, helper, start.turn_id, "copied-source-image", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, "copied-image-source", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    // Source cleanup never changes a protected prompt into an ordinary helper record.
    sqlx::query("DELETE FROM delegated_program_work WHERE helper_id=$1")
        .bind(helper)
        .execute(scene.world.pool.pool())
        .await?;
    let mut conn = scene.world.pool.acquire().await?;
    assert_eq!(
        orch_db::queries::capability_work::copied_source_count(&mut conn, helper).await?,
        1
    );
    assert!(
        orch_db::queries::model_inputs::governors(&mut conn, helper)
            .await?
            .iter()
            .any(|marker| marker.model_input_id == prompt)
    );
    drop(conn);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_admitted_image_request_needs_no_unrelated_thread_creation_operation() -> Proof {
    use orch_access_contract::key_scope::KeyScope;
    let (scene, _, _) = native_scene_with_input_keys(true).await?;
    let keys: Vec<(Id, Json)> = sqlx::query_as(
        "SELECT id,operation_scope FROM api_tokens WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(scene.world.ada)
    .fetch_all(scene.world.pool.pool())
    .await?;
    assert!(!keys.is_empty());
    for (id, value) in keys {
        let mut scope: KeyScope = serde_json::from_value(value)?;
        for rule in &mut scope.rules {
            rule.allow
                .retain(|operation| !operation.operation.starts_with("ThreadCreate"));
        }
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(id)
            .bind(serde_json::to_value(scope)?)
            .execute(scene.world.pool.pool())
            .await?;
    }
    publish(
        &scene,
        "source-native-creation",
        "out/source-native-creation.png",
    )
    .await;
    let (helper, start) = begin_helper(&scene, "source-native-creation").await?;
    image(
        &scene,
        helper,
        start.turn_id,
        "source-native-created-image",
        true,
    )
    .await?;
    let answer = Scene::answer(&scene.world, scene.thread, "source-native-creation", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    let authority:Json=sqlx::query_scalar("SELECT i.submission_authority FROM delegated_program_work w JOIN thread_inputs i ON i.id=w.initial_input_id WHERE w.helper_id=$1")
        .bind(helper).fetch_one(scene.world.pool.pool()).await?;
    scene.end_original().await?;
    assert!(
        authority
            .get("origins")
            .is_none_or(|origins| origins.as_array().is_some_and(Vec::is_empty)),
        "an internal execution input does not claim a public creation operation"
    );
    assert!(
        authority
            .get("boundaries")
            .and_then(Json::as_array)
            .is_some_and(|boundaries| !boundaries.is_empty())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_program_executor_cannot_use_a_call_context_without_its_actual_source() -> Proof {
    let (scene, provider) = native_scene().await?;
    let engine = scene.world.engine();
    let sources = engine
        .inner
        .capability_services
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or("no installed declarations")?;
    let selected=crate::tools::capability_service::selection::from_value(sources.capabilities().declarations(),"image_generation",
        json!({"route":"image_generation.delegation","offering":{"provider":provider,"offering":NATIVE_MODEL}})).ok_or("no native selection")?;
    let call = crate::tools::CallContext {
        machine: scene.world.machine_id,
        thread: scene.thread,
        turn: scene.start.turn_id,
        call_ref: "unscoped-program".into(),
        attempt: None,
        tool: orch_agent_contract::events::OrchTool::ImageGeneration,
        arguments: json!({"prompt":"An orchard","output_paths":["out/unscoped.png"]}),
    };
    let result = crate::tools::capability_service::delegation::executor::Executor::thread_tool(
        &engine,
        &call,
        "image_generation",
        &sources,
        selected,
    )
    .await;
    assert!(
        matches!(result, Err(EngineError::StaleToolSource)),
        "a current thread ID is not native source authority"
    );
    let helpers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND id<>$1")
            .bind(scene.thread)
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(helpers, 0);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_threadless_placement_never_picks_another_environment_or_governance_root() -> Proof {
    use crate::tools::capability_service::delegation::placement::Placement;
    use orch_provider_contract::capabilities::ProgramPlacement;
    let (scene, _) = native_scene().await?;
    let mut conn = scene.world.pool.acquire().await?;
    let source = orch_db::queries::threads::thread(&mut conn, scene.thread)
        .await?
        .ok_or("missing source thread")?;
    let mut config = ProgramPlacement {
        environment_id: source.environment_id,
        helper_topic_id: Some(new_id()),
    };
    let placed =
        Placement::configured(&mut conn, source.org_id, Some(source.topic_id), &config).await?;
    assert_eq!(Some(placed.environment), source.environment_id);
    assert_eq!(
        placed.topic, source.topic_id,
        "the actual source topic takes precedence"
    );
    config.helper_topic_id = Some(source.topic_id);
    assert!(
        Placement::configured(&mut conn, source.org_id, None, &config)
            .await?
            .current(&mut conn)
            .await?
    );
    assert!(
        Placement::configured(&mut conn, new_id(), None, &config)
            .await
            .is_err()
    );
    config.environment_id = None;
    assert!(
        Placement::configured(&mut conn, source.org_id, None, &config)
            .await
            .is_err(),
        "a usable environment elsewhere is never selected by default"
    );
    config.environment_id = Some(new_id());
    assert!(
        Placement::configured(&mut conn, source.org_id, None, &config)
            .await
            .is_err()
    );
    drop(conn);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_threads_delegated_work_keeps_its_actual_environment_topic_and_directory() -> Proof {
    let (scene, provider) = native_scene().await?;
    scene.world.set(scene.thread,orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION_BACKEND,
        json!({"route":"image_generation.delegation","offering":{"provider":provider,"offering":NATIVE_MODEL},
          "config":{"environment_id":new_id(),"helper_topic_id":new_id()}})).await;
    publish(&scene, "source-placement", "out/source-placement.png").await;
    let (helper, start) = begin_helper(&scene, "source-placement").await?;
    let actual: (Id, Id, String) = sqlx::query_as(
        "SELECT environment_id,topic_id,workspace_dir FROM threads WHERE node_id=$1",
    )
    .bind(helper)
    .fetch_one(scene.world.pool.pool())
    .await?;
    let expected: (Id, Id, String) = sqlx::query_as(
        "SELECT environment_id,topic_id,workspace_dir FROM threads WHERE node_id=$1",
    )
    .bind(scene.thread)
    .fetch_one(scene.world.pool.pool())
    .await?;
    assert_eq!(
        actual, expected,
        "backend placement references never redirect an ordinary thread's work"
    );
    image(&scene, helper, start.turn_id, "placed-image", true).await?;
    let outcome = Scene::answer(&scene.world, scene.thread, "source-placement", None).await?;
    assert!(outcome.error.is_none(), "{outcome:?}");
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changing_the_requesting_directory_stops_owned_work_before_a_result_is_published() -> Proof
{
    let (scene, _) = native_scene().await?;
    publish(&scene, "moved-source", "out/moved-source.png").await;
    let (helper, start) = begin_helper(&scene, "moved-source").await?;
    sqlx::query("UPDATE threads SET workspace_dir='moved-source' WHERE node_id=$1")
        .bind(scene.thread)
        .execute(scene.world.pool.pool())
        .await?;
    scene.world.engine().wake(scene.thread);
    scene.world.wait_turn(start.turn_id, "interrupted").await;
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    assert_eq!(scene.world.turn_starts(helper).len(), 1);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_program_without_the_requested_codec_is_unavailable_before_a_helper_is_created() -> Proof
{
    let (scene, provider) = native_scene().await?;
    let mut descriptor: Json = scene
        .world
        .scalar(
            "SELECT descriptor FROM agents WHERE id=$1",
            scene.world.agent,
        )
        .await;
    descriptor["x_capability_program"]["access"]["native"]["native"]["image_generation"]["execution"] =
        json!([]);
    scene.world.declares(descriptor).await;
    let engine = scene.world.engine();
    let sources = engine
        .inner
        .capability_services
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or("no installed declarations")?;
    let mut conn = scene.world.pool.acquire().await?;
    let result = crate::tools::capability_service::delegation::native_state(
        &mut conn,
        &sources,
        provider,
        NATIVE_MODEL,
        "image_generation",
    )
    .await?;
    drop(conn);
    assert!(
        result.is_err(),
        "a delegated mode alone does not prove an implemented input and result codec"
    );
    publish(&scene, "missing-image-codec", "out/missing-codec.png").await;
    let dispatched = Scene::answer(&scene.world, scene.thread, "missing-image-codec", None).await?;
    assert!(dispatched.error.is_none(), "{dispatched:?}");
    let failure = dispatched
        .result
        .ok_or("the missing codec has no explicit failure")?;
    assert_eq!(failure["state"], "failed");
    assert_eq!(
        failure["failure"]["reason"],
        "program_execution_unavailable"
    );
    let helpers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nodes WHERE kind='thread' AND name='Capability helper'",
    )
    .fetch_one(scene.world.pool.pool())
    .await?;
    assert_eq!(helpers, 0);
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind IN ('turn.start','turn.start.fenced') AND serial_key<>$1")
        .bind(orch_link_contract::commands::SerialKey::Thread(scene.thread).wire())
        .fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(commands, 0);
    scene.end_original().await?;
    Ok(())
}

async fn native_scene_with_file_failure() -> Proof<(Scene, Id, Arc<AtomicBool>)> {
    Box::pin(native_scene_with_input_keys(false)).await
}

async fn native_scene_with_input_keys(keyed: bool) -> Proof<(Scene, Id, Arc<AtomicBool>)> {
    let configured = Box::pin(configured_with_input_keys(Shape::Compatible, keyed)).await?;
    native_scene_configured(configured).await
}

async fn native_scene_configured(
    (mut scene, _, fail): (Scene, Id, Arc<AtomicBool>),
) -> Proof<(Scene, Id, Arc<AtomicBool>)> {
    let mut descriptor: Json = scene
        .world
        .scalar(
            "SELECT descriptor FROM agents WHERE id=$1",
            scene.world.agent,
        )
        .await;
    descriptor["x_program_work"] = json!({"origins":["https://native-provider.example.invalid/"]});
    descriptor["x_capability_program"] = json!({
    "title":"Fixture image program", "access":{"native":{"orch_tools":true,"native":{
        "generation":{"enabled":true,"modes":["native"],"default_mode":"native"},
        "image_generation":{"enabled":true,"modes":["off","on"],"default_mode":"off","delegated_mode":"on",
            "execution":[{"input":"image.request","result":"image.generated"}],
            "eligible_when":{"required":["plan"],"properties":{"plan":{"const":"fixture-paid"}}},
            "delegation_label":{"en":"Fixture image program","zh-CN":"测试图像程序"}}
    }}}});
    declare_program(&scene, descriptor).await?;
    // The parent's strict prefix fixture remains intact. This is an ordinary, independent session.
    scene
        .world
        .machine
        .handler
        .on(CommandKind::SessionOpenFenced, |command| {
            let Command::SessionOpenFenced(open) = command else {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "not an ordinary helper session",
                ));
            };
            Ok(CommandResult::SessionOpenFenced(SessionOpenResult {
                native_session_ref: format!("native-{}", open.session.segment_id),
                resumed: false,
                import_report: None,
                instruction_sources: vec![],
                approval_reviewer: None,
            }))
        });
    let provider = new_id();
    sqlx::query("INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,harness_kind,agent_id,machine_id,arrangements,status,facts_requested_at,facts) VALUES($1,'Native image subscription',$2,$3,$3,'subscription','fake',$4,$5,ARRAY['separated'],'ready',$6,$7)")
        .bind(provider).bind(scene.world.ada).bind(scene.world.root).bind(scene.world.agent).bind(scene.world.machine_id)
        .bind(scene.world.clock.now()).bind(json!({"plan":"fixture-paid"})).execute(scene.world.pool.pool()).await?;
    let mut conn = scene.world.pool.acquire().await?;
    // As in ordinary provider creation, attribution is accompanied by its actual Owners entry.
    access::testing::add_entry(
        &mut conn,
        &PageKey::new("provider", provider, "live"),
        "any",
        "access.own",
        ("user", Some(scene.world.ada), None),
        None,
        scene.world.clock.now(),
    )
    .await?;
    drop(conn);
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,groups,enabled,source,created_by) VALUES($1,$2,$3,'Native image model',$4,ARRAY['images'],true,'manual',$5)")
        .bind(new_id()).bind(provider).bind(NATIVE_MODEL).bind(json!({"input_images":true,"context_window":10000}))
        .bind(scene.world.ada).execute(scene.world.pool.pool()).await?;
    scene.world.set(scene.thread, orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION_BACKEND,
        json!({"route":"image_generation.delegation","offering":{"provider":provider,"offering":NATIVE_MODEL}})).await;
    // These cases decide direct accepted/refused outcomes. Recovery choices have their own cases.
    scene
        .world
        .set(
            scene.world.topic,
            orch_settings_contract::registry::keys::CALLS_RULES,
            json!([]),
        )
        .await;
    let node = scene.world.add_machine("Capability node", &["node"]).await;
    node.machine.handler.on(CommandKind::NodeProgramAdmit, |_| {
        Ok(CommandResult::NodeProgramAdmit(
            orch_link_contract::commands::Empty {},
        ))
    });
    let host = common::program_node::placement(
        &mut *scene.world.pool.acquire().await?,
        node.id,
        scene.world.machine_id,
        provider,
        scene.world.ada,
        scene.world.root,
        scene.world.clock.now(),
    )
    .await?;
    scene.program_node = Some((node, host));
    let engine = scene.world.engine();
    let sources = engine
        .inner
        .capability_services
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or("the native fixture has no installed capability declarations")?;
    let mut conn = scene.world.pool.acquire().await?;
    let eligible = crate::tools::capability_service::delegation::native_state(
        &mut conn,
        &sources,
        provider,
        NATIVE_MODEL,
        "image_generation",
    )
    .await?;
    assert!(
        eligible.is_ok(),
        "the paid fixture must satisfy its actual declared native eligibility: {eligible:?}"
    );
    drop(conn);
    assert!(
        can_use_native_provider(&scene, scene.world.ada, provider).await?,
        "the fixture's real Owners entry must admit the selected native model"
    );
    Ok((scene, provider, fail))
}

// Report only durable phases and refusal codes when a real dispatch misses its boundary.
async fn startup_progress(scene: &Scene, call: &str) -> Proof<Json> {
    let answer = crate::ids::tool_result(scene.thread, call);
    let mut facts:Json = sqlx::query_scalar("SELECT jsonb_build_object(
        'frozen_images', jsonb_array_length(c.plan#>'{image_input_material,items}'),
        'completed', c.completed, 'unit_admitted', w.unit_admission IS NOT NULL,
        'helper', h.state, 'blocked', h.blocked_reason, 'step', h.starting_step, 'input', i.state,
        'turn', t.state, 'turn_error', t.error->>'code',
        'commands',(SELECT jsonb_object_agg(kind,state) FROM commands WHERE serial_key=w.helper_id::text))
        FROM thread_tool_calls c LEFT JOIN delegated_program_work w ON w.source_answer_id=c.answer_id
        LEFT JOIN threads h ON h.node_id=w.helper_id LEFT JOIN thread_inputs i ON i.id=w.initial_input_id
        LEFT JOIN turns t ON t.id=i.turn_id WHERE c.answer_id=$1 ORDER BY w.created_at DESC LIMIT 1")
        .bind(answer).fetch_optional(scene.world.pool.pool()).await?.unwrap_or_else(||json!({}));
    if let Some(reply) = scene
        .world
        .engine()
        .retained_tool_answer(answer, scene.world.machine_id)
        .await?
    {
        facts["reply_error"] = json!(reply.error.map(|error| error.code));
        facts["reply_reason"] = reply
            .result
            .and_then(|result| result.pointer("/failure/reason").cloned())
            .unwrap_or(Json::Null);
    }
    Ok(facts)
}

async fn helper(scene: &Scene, call: &str) -> Proof<Id> {
    let answer = crate::ids::tool_result(scene.thread, call);
    match tokio::time::timeout(common::PATIENCE, async {
        loop {
            let helper:Option<Id>=sqlx::query_scalar("SELECT helper_id FROM delegated_program_work WHERE request_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1")
                .bind(answer).fetch_optional(scene.world.pool.pool()).await?;
            if let Some(helper)=helper {return Ok(helper);}
            tokio::time::sleep(common::config().sweep_every).await;
        }
    }).await {
        Ok(result) => result,
        Err(_) => Err(format!("the real tool did not retain its helper: {}",startup_progress(scene,call).await?).into()),
    }
}

async fn begin_helper(scene: &Scene, call: &str) -> Proof<(Id, TurnStart)> {
    let helper = helper(scene, call).await?;
    let start = match tokio::time::timeout(common::PATIENCE, async {
        loop {
            if let Some(start) = scene.world.turn_starts(helper).first().cloned() {
                break start;
            }
            tokio::time::sleep(common::config().sweep_every).await;
        }
    })
    .await
    {
        Ok(start) => start,
        Err(_) => {
            return Err(format!(
                "the helper did not reach native startup: {}",
                startup_progress(scene, call).await?
            )
            .into());
        }
    };
    scene
        .world
        .say(helper, common::turn_began(start.turn_id))
        .await;
    scene.world.wait_turn(start.turn_id, "running").await;
    Ok((helper, start))
}

async fn publish(scene: &Scene, call: &str, path: &str) {
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        None,
        OrchTool::ImageGeneration,
        json!({"prompt":"An orchard","output_paths":[path]}),
    )
    .await;
}

async fn image(
    scene: &Scene,
    helper: Id,
    turn: Id,
    reference: &str,
    generated: bool,
) -> Proof<Hash> {
    let png = bytes()?;
    let blob = Hash::of(&png);
    scene
        .store
        .put(
            &blob,
            Some(i64::try_from(png.len())?),
            stream_of(vec![Bytes::from(png.clone())]),
        )
        .await?;
    let mut conn = scene.world.pool.acquire().await?;
    orch_db::queries::storage::register_blob(
        &mut conn,
        &blob,
        i64::try_from(png.len())?,
        scene.world.clock.now(),
    )
    .await?;
    drop(conn);
    let payload = ItemPayload::ToolCall(serde_json::from_value(json!({
        "title":"Native image result","started_at":scene.world.clock.time(),"duration_ms":null,
        "tool":{"kind":"image","blob":blob,"media_type":"image/png","caption":"A retained orchard.",
            "capability":if generated {Some("image_generation")} else {None}},
        "raw":{"name":if generated {"imageGeneration"} else {"imageView"},"input":orch_api_contract::threads::OutputRef::of("{}"),"output":null,"is_error":false}
    }))?);
    scene
        .world
        .say(
            helper,
            Event::ItemStarted(orch_link_contract::events::ItemStarted {
                turn_id: turn,
                native_ref: reference.into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: orch_link_contract::events::EmittedItemKind::ToolCall,
                payload: payload.clone(),
            }),
        )
        .await;
    scene
        .world
        .say(
            helper,
            Event::ItemCompleted(orch_link_contract::events::ItemCompleted {
                native_ref: reference.into(),
                status: orch_link_contract::foreign::ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        )
        .await;
    Ok(blob)
}

async fn native_text(
    scene: &Scene,
    helper: Id,
    turn: Id,
    reference: &str,
    text: &str,
    phase: Option<&str>,
    parent: Option<&str>,
) -> Proof {
    let payload = ItemPayload::AgentMessage(serde_json::from_value(json!({
        "text":text,"text_len":text.chars().count(),"phase":phase,
    }))?);
    scene
        .world
        .say(
            helper,
            Event::ItemStarted(orch_link_contract::events::ItemStarted {
                turn_id: turn,
                native_ref: reference.into(),
                stream: "main".into(),
                parent_native_ref: parent.map(str::to_owned),
                kind: orch_link_contract::events::EmittedItemKind::AgentMessage,
                payload: payload.clone(),
            }),
        )
        .await;
    scene
        .world
        .say(
            helper,
            Event::ItemCompleted(orch_link_contract::events::ItemCompleted {
                native_ref: reference.into(),
                status: orch_link_contract::foreign::ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        )
        .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_text_answer_uses_only_the_successful_owned_turns_final_main_message() -> Proof {
    use crate::tools::capability_service::delegation::text;
    for phased in [true, false] {
        let (scene, _) = native_scene().await?;
        publish(
            &scene,
            "native-text-evidence",
            "out/text-is-not-an-image.png",
        )
        .await;
        let (helper, start) = begin_helper(&scene, "native-text-evidence").await?;
        native_text(
            &scene,
            helper,
            start.turn_id,
            "earlier",
            "Earlier text.",
            phased.then_some("commentary"),
            None,
        )
        .await?;
        native_text(
            &scene,
            helper,
            start.turn_id,
            "answer",
            "The final answer.",
            phased.then_some("final"),
            None,
        )
        .await?;
        native_text(
            &scene,
            helper,
            start.turn_id,
            "nested",
            "A nested answer is not the result.",
            phased.then_some("final"),
            Some("answer"),
        )
        .await?;
        if phased {
            native_text(
                &scene,
                helper,
                start.turn_id,
                "later-unphased",
                "A later unphased note is not the final phase.",
                None,
                None,
            )
            .await?;
        }
        let mut conn = scene.world.pool.acquire().await?;
        assert!(
            text::completed(
                &mut conn,
                helper,
                start.turn_id,
                Some(scene.world.machine_id)
            )
            .await?
            .is_none(),
            "an item is not a successful turn"
        );
        drop(conn);
        scene
            .world
            .say(
                helper,
                common::turn_ended(start.turn_id, TurnOutcome::Completed),
            )
            .await;
        scene.world.wait_turn(start.turn_id, "completed").await;
        let mut conn = scene.world.pool.acquire().await?;
        let result = text::completed(
            &mut conn,
            helper,
            start.turn_id,
            Some(scene.world.machine_id),
        )
        .await?
        .ok_or("no final text receipt")?;
        assert_eq!(result.text, "The final answer.");
        assert_eq!(result.native_ref, "answer");
        assert!(!result.command.is_nil());
        assert_eq!(
            Some(result),
            text::completed(
                &mut conn,
                helper,
                start.turn_id,
                Some(scene.world.machine_id)
            )
            .await?
        );
        assert!(
            text::completed(&mut conn, helper, start.turn_id, Some(new_id()))
                .await?
                .is_none()
        );
        assert!(
            text::completed(
                &mut conn,
                scene.thread,
                start.turn_id,
                Some(scene.world.machine_id)
            )
            .await?
            .is_none()
        );
        drop(conn);
        assert_eq!(scene.world.turn_starts(helper).len(), 1);
        assert_eq!(
            scene.world.began(CommandKind::FsWrite),
            0,
            "text cannot replace generated image evidence"
        );
        scene.end_original().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commentary_and_failed_turns_cannot_supply_a_successful_text_answer() -> Proof {
    use crate::tools::capability_service::delegation::text;
    for (outcome, state, phase) in [
        (TurnOutcome::Failed, "failed", "final"),
        (TurnOutcome::Completed, "completed", "commentary"),
    ] {
        let (scene, _) = native_scene().await?;
        publish(&scene, "failed-native-text", "out/failed-text.png").await;
        let (helper, start) = begin_helper(&scene, "failed-native-text").await?;
        native_text(
            &scene,
            helper,
            start.turn_id,
            "failed-answer",
            "An incomplete answer.",
            Some(phase),
            None,
        )
        .await?;
        scene
            .world
            .say(helper, common::turn_ended(start.turn_id, outcome))
            .await;
        scene.world.wait_turn(start.turn_id, state).await;
        let mut conn = scene.world.pool.acquire().await?;
        assert!(
            text::completed(
                &mut conn,
                helper,
                start.turn_id,
                Some(scene.world.machine_id)
            )
            .await?
            .is_none()
        );
        drop(conn);
        scene.end_original().await?;
    }
    Ok(())
}

async fn can_read(scene: &Scene, person: Id, thread: Id) -> Proof<bool> {
    let mut conn = scene.world.pool.acquire().await?;
    let principal =
        Principal::load(&mut conn, Caller::session(person), scene.world.clock.time()).await?;
    Ok(orch_authz::nodes::check(
        &mut conn,
        &principal,
        thread,
        Permission::ThreadView,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await?
    .is_allowed())
}

async fn can_use_native_provider(scene: &Scene, person: Id, provider: Id) -> Proof<bool> {
    let mut conn = scene.world.pool.acquire().await?;
    let principal =
        Principal::load(&mut conn, Caller::session(person), scene.world.clock.time()).await?;
    let policies = crate::access::resource_policies(&mut conn, provider).await?;
    let usable = orch_authz::usable::usable(
        &mut conn,
        &principal,
        provider,
        Some(scene.thread),
        &policies,
    )
    .await?;
    let groups = ["images".to_owned()];
    Ok(usable.granted().is_some_and(|granted| {
        granted.allows(&orch_authz::options::Wanted {
            model: Some((NATIVE_MODEL, &groups[..])),
            ..orch_authz::options::Wanted::default()
        })
    }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_real_input_keys_remain_required_by_the_helper_and_its_stored_native_command() -> Proof
{
    for (name, change) in [
        ("source-author", "revoke"),
        ("source-editor", "revoke"),
        ("source-author", "expire"),
        ("source-editor", "narrow"),
    ] {
        let (scene, _, _) = Box::pin(native_scene_with_input_keys(true)).await?;
        let authority: Json = scene
            .world
            .scalar(
                "SELECT submission_authority FROM thread_inputs WHERE id=$1",
                scene.input,
            )
            .await;
        assert_eq!(
            authority["origins"]
                .as_array()
                .ok_or("actual origins absent")?
                .len(),
            2,
            "the actual submit and queued edit must retain independent credentials"
        );
        publish(&scene, "keyed-native-image", "out/keyed.png").await;
        let (helper, start) = begin_helper(&scene, "keyed-native-image").await?;
        let mut conn = scene.world.pool.acquire().await?;
        let native:Id=sqlx::query_scalar("SELECT id FROM commands WHERE kind IN ('session.open','session.open.fenced') AND COALESCE(body->>'thread_id',body#>>'{session,thread_id}')=$1 ORDER BY created_at,id LIMIT 1")
            .bind(helper.to_string()).fetch_one(&mut *conn).await?;
        let command = orch_db::queries::link::command(&mut conn, native)
            .await?
            .ok_or("native command was not journaled")?;
        let private = orch_db::queries::link::command_authority(&mut conn, native)
            .await?
            .ok_or("native command has no actual private source")?;
        let saved: crate::commands::CommandAuthority = serde_json::from_value(private)?;
        assert!(
            matches!(
                &saved,
                crate::commands::CommandAuthority::CapabilitySession(_)
            ),
            "the real command carries the original capability source"
        );
        let engine = scene.world.engine();
        assert!(
            saved
                .current_callers(
                    &mut conn,
                    &command,
                    scene.world.clock.as_ref(),
                    engine.inner.access.as_ref()
                )
                .await?
                .is_some(),
            "the stored native authority must pass positively before changing either key"
        );
        let token: Id =
            sqlx::query_scalar("SELECT id FROM api_tokens WHERE user_id=$1 AND name=$2")
                .bind(scene.world.ada)
                .bind(name)
                .fetch_one(&mut *conn)
                .await?;
        match change {
            "revoke" => {
                orch_db::queries::identity::revoke_api_token(
                    &mut conn,
                    token,
                    scene.world.ada,
                    scene.world.clock.now(),
                )
                .await?;
            }
            "expire" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(token)
                    .bind(scene.world.clock.now() - chrono::Duration::seconds(1))
                    .execute(&mut *conn)
                    .await?;
            }
            "narrow" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(token)
                    .bind(serde_json::to_value(
                        orch_access_contract::key_scope::KeyScope::default(),
                    )?)
                    .execute(&mut *conn)
                    .await?;
            }
            _ => return Err("unknown key control".into()),
        }
        assert!(
            saved
                .current_callers(
                    &mut conn,
                    &command,
                    scene.world.clock.as_ref(),
                    engine.inner.access.as_ref()
                )
                .await?
                .is_none(),
            "a journaled native command never launders the {name} key after {change}"
        );
        drop(conn);
        scene.world.wait_turn(start.turn_id, "interrupted").await;
        assert!(!scene.directory.path().join("out/keyed.png").exists());
        assert_eq!(
            scene.world.turn_starts(helper).len(),
            1,
            "no subsequent helper work is bought after loss of the original credential"
        );
        assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
        assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
        assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
        scene.end_original().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_native_helper_keeps_its_real_owner_result_links_and_ledger_on_replay() -> Proof {
    let (scene, provider) = native_scene().await?;
    publish(&scene, "native-image", "out/native.png").await;
    let (helper, start) = begin_helper(&scene, "native-image").await?;
    let (owner,topic,kind,parent):(Id,Id,String,Option<Id>)=sqlx::query_as("SELECT n.owner_id,t.topic_id,n.kind,t.side_of FROM nodes n JOIN threads t ON t.node_id=n.id WHERE n.id=$1")
        .bind(helper).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(owner, scene.world.ada);
    assert_eq!(topic, scene.world.topic);
    assert_eq!(kind, "thread");
    assert!(parent.is_none(), "a helper is not a side thread");
    assert!(start.text.contains(&format!("/thread/{}", scene.thread)));
    assert!(can_read(&scene, scene.world.ada, helper).await?);
    let outsider = scene.world.person("native-helper-reader").await;
    assert!(
        !can_read(&scene, outsider, helper).await?,
        "the source's audience is not a second helper access model"
    );
    let usage = TokenUsage {
        input_tokens: 5,
        output_tokens: 2,
        total_tokens: 7,
        ..TokenUsage::default()
    };
    scene
        .world
        .say(
            helper,
            Event::UsageUpdated(UsageUpdated {
                turn_id: start.turn_id,
                turn: usage,
                thread: usage,
                context_window: None,
                context_used: None,
                report: Some(orch_api_contract::context::TurnUsageReport {
                    models: vec![orch_api_contract::context::ModelTokens {
                        model: NATIVE_MODEL.into(),
                        tokens: usage,
                        tokens_reported: true,
                        counts: BTreeMap::from([("image_generation".into(), 1)]),
                    }],
                    ..Default::default()
                }),
            }),
        )
        .await;
    let blob = image(&scene, helper, start.turn_id, "native-generation", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, "native-image", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    let value = answer.result.ok_or("native image outcome absent")?;
    assert_eq!(value["images"][0]["blob"], json!(blob));
    assert_eq!(value["images"][0]["status"], "ready");
    assert_eq!(value["helper"]["thread_id"], json!(helper));
    assert_eq!(value["helper"]["actor_id"], json!(scene.world.ada));
    assert_eq!(value["helper"]["provider_id"], json!(provider));
    assert_eq!(
        std::fs::read(scene.directory.path().join("out/native.png"))?,
        bytes()?
    );
    scene.world.wait_turn(start.turn_id, "completed").await;
    let rows = scene.world.ledger_of(start.turn_id).await;
    assert!(
        !rows.is_empty(),
        "actual native observations must be counted"
    );
    assert!(rows.iter().all(|row| row.provider_id == provider
        && row.user_id == Some(scene.world.ada)
        && row.thread_id == Some(helper)
        && row.purpose == "image_generation"));
    let credited:i64=sqlx::query_scalar("SELECT COALESCE(sum(total_tokens) FILTER(WHERE counted),0)::bigint FROM usage_events WHERE turn_id=$1")
        .bind(start.turn_id).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(credited, 7);
    let repeated = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "native-image",
        Some(new_id()),
        OrchTool::ImageGeneration,
        json!({"prompt":"An orchard","output_paths":["out/native.png"]}),
    )
    .await?;
    assert_eq!(repeated.result, Some(value));
    assert_eq!(scene.world.turn_starts(helper).len(), 1);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    assert_eq!(scene.world.began(CommandKind::FsWrite), 1);
    let after = scene.world.ledger_of(start.turn_id).await;
    assert_eq!(
        rows, after,
        "publishing a retained result never charges a second turn"
    );
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_viewed_image_never_satisfies_the_native_generation_and_source_end_stops_only_its_helper()
-> Proof {
    let (scene, _) = native_scene().await?;
    publish(&scene, "view-is-not-generation", "out/not-generated.png").await;
    let (helper, start) = begin_helper(&scene, "view-is-not-generation").await?;
    image(&scene, helper, start.turn_id, "viewed-only", false).await?;
    let items:i64=sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND payload#>>'{tool,capability}'='image_generation'")
        .bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(items, 0);
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    scene.end_original().await?;
    scene.world.wait_turn(start.turn_id, "interrupted").await;
    assert!(
        !scene
            .directory
            .path()
            .join("out/not-generated.png")
            .exists()
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ineligible_native_plan_cannot_start_a_helper_or_borrow_a_primary_offering() -> Proof {
    let (scene, provider) = native_scene().await?;
    sqlx::query("UPDATE providers SET facts=$2 WHERE id=$1")
        .bind(provider)
        .bind(json!({"plan":"fixture-free"}))
        .execute(scene.world.pool.pool())
        .await?;
    let outcome = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "free-native-image",
        None,
        OrchTool::ImageGeneration,
        json!({"prompt":"An orchard","output_paths":["out/refused.png"]}),
    )
    .await?;
    assert!(
        outcome.error.is_none(),
        "a reported capability failure is a structured domain result"
    );
    let failed = outcome.result.ok_or("the eligibility reason was dropped")?;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["isError"], true);
    assert_eq!(failed["failure"]["code"], "unavailable");
    assert_eq!(
        failed["failure"]["reason"],
        "program_support_not_applicable"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND id<>$1")
            .bind(scene.thread)
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(count, 0);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_attribution_without_an_owners_entry_cannot_create_a_native_helper() -> Proof {
    let (scene, provider) = native_scene().await?;
    let mut tx = scene.world.pool.begin().await?;
    let owners =
        access::owner_entries(&mut tx, &[PageKey::new("provider", provider, "live")]).await?;
    assert_eq!(owners.len(), 1);
    assert!(
        access::remove_entry(
            &mut tx,
            owners[0].id,
            scene.world.ada,
            scene.world.clock.now(),
        )
        .await?
    );
    access::bump_epoch(&mut tx).await?;
    tx.commit().await?;
    let attributed: Id = scene
        .world
        .scalar("SELECT owner_id FROM providers WHERE id=$1", provider)
        .await;
    assert_eq!(attributed, scene.world.ada);
    assert!(!can_use_native_provider(&scene, scene.world.ada, provider).await?);
    publish(&scene, "native-owner-attribution", "out/no-owner.png").await;
    let refused =
        Scene::answer(&scene.world, scene.thread, "native-owner-attribution", None).await?;
    assert!(refused.error.is_some(), "{refused:?}");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND id<>$1")
            .bind(scene.thread)
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(count, 0, "the refusal precedes ordinary helper creation");
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    scene.end_original().await?;
    Ok(())
}

/// Makes somebody else the owner of the provider, so that the scene's person uses it only through
/// the entries that are written for them afterwards.
async fn give_the_sign_in_to_another_owner(scene: &Scene, provider: Id) -> Proof<Id> {
    let sign_in_owner = scene.world.person("native-signin-owner").await;
    let mut tx = scene.world.pool.begin().await?;
    let page = PageKey::new("provider", provider, "live");
    let owners = access::owner_entries(&mut tx, std::slice::from_ref(&page)).await?;
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].holder_id, Some(scene.world.ada));
    assert!(
        access::remove_entry(
            &mut tx,
            owners[0].id,
            scene.world.ada,
            scene.world.clock.now(),
        )
        .await?
    );
    access::testing::add_entry(
        &mut tx,
        &page,
        "any",
        "access.own",
        ("user", Some(sign_in_owner), None),
        None,
        scene.world.clock.now(),
    )
    .await?;
    sqlx::query("UPDATE providers SET owner_id=$2 WHERE id=$1")
        .bind(provider)
        .bind(sign_in_owner)
        .execute(&mut *tx)
        .await?;
    access::bump_epoch(&mut tx).await?;
    tx.commit().await?;
    Ok(sign_in_owner)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_provider_use_controls_a_shared_sign_in_without_changing_helper_ownership() -> Proof
{
    use orch_db::queries::access::NewEntry;
    for shared in [false, true] {
        let (scene, provider) = native_scene().await?;
        let sign_in_owner = give_the_sign_in_to_another_owner(&scene, provider).await?;
        scene
            .world
            .set(
                scene.world.root,
                orch_settings_contract::registry::keys::PROVIDERS_SUBSCRIPTION_SHARING,
                if shared { json!(["fake"]) } else { json!([]) },
            )
            .await;
        let mut conn = scene.world.pool.acquire().await?;
        access::insert_entry(
            &mut conn,
            &NewEntry {
                id: new_id(),
                page: &PageKey::new("provider", provider, "live"),
                profile: "any",
                permission: "provider.use",
                holder_kind: "user",
                holder_id: Some(scene.world.ada),
                holder_role: None,
                options: &json!({"models":[NATIVE_MODEL],"arrangements":["separated"]}),
                expires_at: None,
                note: "",
                source: "page",
                created_by: Some(sign_in_owner),
                at: scene.world.clock.now(),
            },
        )
        .await?;
        drop(conn);
        assert_eq!(
            can_use_native_provider(&scene, scene.world.ada, provider).await?,
            shared,
            "the real Use entry remains subject to the subscription-sharing policy"
        );
        publish(&scene, "shared-native-image", "out/shared.png").await;
        if !shared {
            let refused =
                Scene::answer(&scene.world, scene.thread, "shared-native-image", None).await?;
            assert!(
                refused.error.is_some(),
                "a Use entry cannot defeat the existing sign-in sharing policy"
            );
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND id<>$1")
                    .bind(scene.thread)
                    .fetch_one(scene.world.pool.pool())
                    .await?;
            assert_eq!(count, 0);
        } else {
            let (helper, start) = begin_helper(&scene, "shared-native-image").await?;
            assert!(can_read(&scene, scene.world.ada, helper).await?);
            assert!(
                !can_read(&scene, sign_in_owner, helper).await?,
                "paying with a shared sign-in does not disclose helper content to its owner"
            );
            let owner: Id = scene
                .world
                .scalar("SELECT owner_id FROM nodes WHERE id=$1", helper)
                .await;
            assert_eq!(owner, scene.world.ada);
            image(&scene, helper, start.turn_id, "shared-native-result", true).await?;
            let answer =
                Scene::answer(&scene.world, scene.thread, "shared-native-image", None).await?;
            assert!(answer.error.is_none(), "{answer:?}");
            assert_eq!(
                answer.result.ok_or("shared result absent")?["helper"]["actor_id"],
                json!(scene.world.ada)
            );
        }
        assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
        assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
        scene.end_original().await?;
    }
    Ok(())
}

/// An agent's own tool call that uses the provider is work of the thread's environment: the entry
/// has to name the place the thread runs in, or the helper is not made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_helper_is_created_only_where_the_entry_lets_the_threads_environment_run() -> Proof
{
    use orch_db::queries::access::NewEntry;
    for named in [false, true] {
        let (scene, provider) = native_scene().await?;
        let sign_in_owner = give_the_sign_in_to_another_owner(&scene, provider).await?;
        scene
            .world
            .set(
                scene.world.root,
                orch_settings_contract::registry::keys::PROVIDERS_SUBSCRIPTION_SHARING,
                json!(["fake"]),
            )
            .await;
        let place = if named {
            scene.world.environment
        } else {
            new_id()
        };
        let mut conn = scene.world.pool.acquire().await?;
        access::insert_entry(
            &mut conn,
            &NewEntry {
                id: new_id(),
                page: &PageKey::new("provider", provider, "live"),
                profile: "any",
                permission: "provider.use",
                holder_kind: "user",
                holder_id: Some(scene.world.ada),
                holder_role: None,
                options: &json!({
                    "models": [NATIVE_MODEL],
                    "arrangements": ["separated"],
                    "where": [{ "kind": "environment", "id": place }],
                }),
                expires_at: None,
                note: "",
                source: "page",
                created_by: Some(sign_in_owner),
                at: scene.world.clock.now(),
            },
        )
        .await?;
        drop(conn);
        publish(&scene, "placed-native-image", "out/placed.png").await;
        if named {
            let (helper, start) = begin_helper(&scene, "placed-native-image").await?;
            image(&scene, helper, start.turn_id, "placed-native-result", true).await?;
            let answer =
                Scene::answer(&scene.world, scene.thread, "placed-native-image", None).await?;
            assert!(
                answer.error.is_none(),
                "the entry names the place: {answer:?}"
            );
        } else {
            let refused =
                Scene::answer(&scene.world, scene.thread, "placed-native-image", None).await?;
            assert!(
                refused.error.is_some(),
                "an entry that names another environment does not let the thread's tool call run"
            );
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND id<>$1")
                    .bind(scene.thread)
                    .fetch_one(scene.world.pool.pool())
                    .await?;
            assert_eq!(count, 0, "the refusal precedes helper creation");
        }
        assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
        assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
        scene.end_original().await?;
    }
    Ok(())
}

async fn report_native_image_limit(scene: &Scene, helper: Id, turn: Id, reset: Time) -> Proof {
    let error = ItemPayload::Error(serde_json::from_value(json!({
        "code":"image_generation_failed","message":"The native image allowance was reached.","retryable":true,
        "window":"image_generation","resets_at":reset,
        "details":{"capability":"image_generation","failure":{"code":"rate_limited","message":"The native image allowance was reached.","retryable":true,"window":"image_generation","resets_at":reset}}
    }))?);
    scene
        .world
        .say(
            helper,
            Event::ItemStarted(orch_link_contract::events::ItemStarted {
                turn_id: turn,
                native_ref: "native-image-limit".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: orch_link_contract::events::EmittedItemKind::Error,
                payload: error.clone(),
            }),
        )
        .await;
    scene
        .world
        .say(
            helper,
            Event::ItemCompleted(orch_link_contract::events::ItemCompleted {
                native_ref: "native-image-limit".into(),
                status: orch_link_contract::foreign::ItemStatus::Completed,
                payload: error,
                output_blob: None,
            }),
        )
        .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_image_allowance_failure_keeps_its_actual_window_and_reset_for_the_original_call()
-> Proof {
    let (scene, _) = native_scene().await?;
    scene
        .world
        .set(
            scene.world.topic,
            orch_settings_contract::registry::keys::CALLS_RULES,
            json!([]),
        )
        .await;
    publish(&scene, "native-limit", "out/limited.png").await;
    let (helper, start) = begin_helper(&scene, "native-limit").await?;
    let reset = Time::new(scene.world.clock.now() + chrono::Duration::minutes(15));
    report_native_image_limit(&scene, helper, start.turn_id, reset).await?;
    let failed = Scene::answer(&scene.world, scene.thread, "native-limit", None).await?;
    assert!(failed.error.is_none());
    let reply = failed
        .result
        .ok_or("the actual native failure was dropped")?;
    assert_eq!(reply["state"], "failed");
    assert_eq!(reply["isError"], true);
    assert_eq!(reply["failure"]["code"], "rate_limited");
    assert_eq!(reply["failure"]["window"], "image_generation");
    assert_eq!(reply["failure"]["resets_at"], json!(reset));
    assert_eq!(reply["helper"]["thread_id"], json!(helper));
    let retained: Json = scene
        .world
        .scalar(
            "SELECT plan FROM thread_tool_calls WHERE answer_id=$1",
            crate::ids::tool_result(scene.thread, "native-limit"),
        )
        .await;
    let failure = retained
        .as_object()
        .ok_or("private call plan absent")?
        .iter()
        .find(|(key, _)| key.starts_with("delegated_failure:"))
        .map(|(_, value)| value)
        .ok_or("native failure was discarded")?;
    assert_eq!(failure["code"], "rate_limited");
    assert_eq!(failure["window"], "image_generation");
    assert_eq!(failure["resets_at"], json!(reset));
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    assert!(!scene.directory.path().join("out/limited.png").exists());
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_allowance_pause_keeps_the_rule_helper_and_reset_without_an_accepted_image()
-> Proof {
    let (scene, _) = native_scene().await?;
    scene.world.set(scene.world.topic,orch_settings_contract::registry::keys::CALLS_RULES,
        json!([{"id":"image-reset","when":{"kind":"rate_limited","purpose":"image_generation"},"steps":[{"type":"wait","until":"reset"}]}])).await;
    let call = "native-limit-pause";
    publish(&scene, call, "out/paused.png").await;
    let (helper, start) = begin_helper(&scene, call).await?;
    let reset = Time::new(scene.world.clock.now() + chrono::Duration::minutes(15));
    report_native_image_limit(&scene, helper, start.turn_id, reset).await?;
    let reply = Scene::answer(&scene.world, scene.thread, call, None).await?;
    assert!(
        reply.error.is_none(),
        "a real rule pause remains a control result: {reply:?}"
    );
    let body = reply.result.ok_or("the pause outcome was dropped")?;
    assert_eq!(body["state"], "paused");
    assert_eq!(body["service"], "image_generation");
    assert_eq!(body["cause"], "rate_limited");
    assert_eq!(body["reason"], "reset_after_deadline");
    assert_eq!(body["until"], json!(reset));
    assert_eq!(body["failure"]["window"], "image_generation");
    assert_eq!(body["failure"]["resets_at"], json!(reset));
    assert_eq!(body["helper"]["thread_id"], json!(helper));
    assert!(body.get("images").is_none());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_tool_service_results WHERE answer_id=$1")
            .bind(crate::ids::tool_result(scene.thread, call))
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(count, 0, "a pause never mints an accepted result receipt");
    let repeated = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        call,
        None,
        OrchTool::ImageGeneration,
        json!({"prompt":"An orchard","output_paths":["out/paused.png"]}),
    )
    .await?;
    assert_eq!(repeated.result, Some(body));
    assert_eq!(scene.world.turn_starts(helper).len(), 1);
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn already_generated_pixels_survive_a_crash_before_acceptance_without_fresh_model_use()
-> Proof {
    let (scene, provider) = native_scene().await?;
    let call = "generated-before-acceptance";
    expect_task(scene.thread, call);
    let (ready, release) = gate(scene.thread, call, Point::ProgramResult);
    publish(&scene, call, "out/generated-before-acceptance.png").await;
    let (helper, start) = begin_helper(&scene, call).await?;
    let blob = image(&scene, helper, start.turn_id, "paid-native-image", true).await?;
    tokio::time::timeout(common::PATIENCE, ready).await??;
    let answer = crate::ids::tool_result(scene.thread, call);
    let accepted: bool = sqlx::query_scalar(
        "SELECT plan ? 'accepted_service_result' FROM thread_tool_calls WHERE answer_id=$1",
    )
    .bind(answer)
    .fetch_one(scene.world.pool.pool())
    .await?;
    assert!(
        !accepted,
        "the crash is before acceptance, with real generated-image evidence durable"
    );
    interrupt_task(scene.thread, call).await?;
    drop(release);
    let sources = scene
        .world
        .engine()
        .inner
        .capability_services
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or("no installed native codecs")?;
    sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1 AND model_name=$2")
        .bind(provider)
        .bind(NATIVE_MODEL)
        .execute(scene.world.pool.pool())
        .await?;
    scene.world.restart_api().await;
    let engine = scene.world.engine();
    engine.attach_blobs(scene.store.clone());
    engine.attach_capability_services(sources);
    engine.attach_parcels(Arc::new(crate::parcels::WorkspaceParcels::new(&engine)));
    publish(&scene, call, "out/generated-before-acceptance.png").await;
    let restored = Scene::answer(&scene.world, scene.thread, call, None).await?;
    assert!(
        restored.error.is_none(),
        "paid pixels need source Read, not fresh paid model admission: {restored:?}"
    );
    let value = restored
        .result
        .ok_or("the generated pixels were not recovered")?;
    assert_eq!(value["images"][0]["blob"], json!(blob));
    assert_eq!(value["images"][0]["status"], "ready");
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
        (SELECT count(*) FROM delegated_program_work WHERE request_id=$1),
        (SELECT count(*) FROM thread_inputs WHERE thread_id=$2),
        (SELECT count(*) FROM turns WHERE thread_id=$2)",
    )
    .bind(answer)
    .bind(helper)
    .fetch_one(scene.world.pool.pool())
    .await?;
    assert_eq!(counts, (1, 1, 1));
    assert_eq!(scene.world.turn_starts(helper).len(), 1);
    assert_eq!(
        std::fs::read(
            scene
                .directory
                .path()
                .join("out/generated-before-acceptance.png")
        )?,
        bytes()?
    );
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_result_survives_file_failure_and_restart_without_reopening_its_paid_backend()
-> Proof {
    let (scene, provider, fail) = native_scene_with_file_failure().await?;
    fail.store(true, Ordering::SeqCst);
    publish(&scene, "retained-native-image", "out/native-retry.png").await;
    let (helper, start) = begin_helper(&scene, "retained-native-image").await?;
    let blob = image(
        &scene,
        helper,
        start.turn_id,
        "retained-native-result",
        true,
    )
    .await?;
    let failed = Scene::answer(&scene.world, scene.thread, "retained-native-image", None).await?;
    assert!(
        failed.error.is_none(),
        "the paid image survives the file failure: {failed:?}"
    );
    let first = failed.result.ok_or("retained native result absent")?;
    assert_eq!(first["images"][0]["status"], "failed");
    assert_eq!(first["images"][0]["blob"], json!(blob));
    assert!(!scene.directory.path().join("out/native-retry.png").exists());
    let sources = scene
        .world
        .engine()
        .inner
        .capability_services
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or("native capability services absent")?;
    scene.world.restart_api().await;
    let engine = scene.world.engine();
    engine.attach_blobs(scene.store.clone());
    engine.attach_capability_services(sources);
    engine.attach_parcels(Arc::new(crate::parcels::WorkspaceParcels::new(&engine)));
    sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1 AND model_name=$2")
        .bind(provider)
        .bind(NATIVE_MODEL)
        .execute(scene.world.pool.pool())
        .await?;
    fail.store(false, Ordering::SeqCst);
    let delivered = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "deliver-retained-native",
        None,
        OrchTool::ImageGeneration,
        json!({"delivery_ref":first["delivery_ref"],"output_paths":["out/native-retry.png"]}),
    )
    .await?;
    assert!(
        delivered.error.is_none(),
        "delivery uses retained pixels after the backend closes: {delivered:?}"
    );
    let value = delivered.result.ok_or("delivered native outcome absent")?;
    assert_eq!(value["images"][0]["blob"], json!(blob));
    assert_eq!(value["images"][0]["status"], "ready");
    assert_eq!(
        std::fs::read(scene.directory.path().join("out/native-retry.png"))?,
        bytes()?
    );
    let helpers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND id<>$1")
            .bind(scene.thread)
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(helpers, 1);
    assert_eq!(scene.world.turn_starts(helper).len(), 1);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reported_free_and_unconfirmed_native_sign_in_return_the_programs_reason_without_paid_work()
-> Proof {
    let programme: Json = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../extensions/agents/codex/src/capability-program.json"
    )))?;
    for (facts, reason) in [
        (
            json!({"plan":"free","credential":{"yields":"login","method":"auth_file"},"credential_verified":true}),
            "image_generation.free_plan",
        ),
        (
            json!({"plan":"plus","credential":null,"credential_verified":false}),
            "image_generation.sign_in_unconfirmed",
        ),
    ] {
        let (scene, provider) = native_scene().await?;
        let mut descriptor: Json = scene
            .world
            .scalar(
                "SELECT descriptor FROM agents WHERE id=$1",
                scene.world.agent,
            )
            .await;
        descriptor["x_capability_program"] = programme.clone();
        scene.world.declares(descriptor).await;
        sqlx::query("UPDATE providers SET facts=$2 WHERE id=$1")
            .bind(provider)
            .bind(facts)
            .execute(scene.world.pool.pool())
            .await?;
        let reply = Scene::ask(
            &scene.world,
            &scene.source,
            scene.start.turn_id,
            "reported-native-unavailable",
            None,
            OrchTool::ImageGeneration,
            json!({"prompt":"An orchard","output_paths":["out/refused-native.png"]}),
        )
        .await?;
        assert!(
            reply.error.is_none(),
            "the exact domain reason must survive the dispatcher"
        );
        let body = reply.result.ok_or("native failure was not returned")?;
        assert_eq!(body["state"], "failed");
        assert_eq!(body["isError"], true);
        assert_eq!(body["failure"]["reason"], reason);
        assert!(body.get("images").is_none());
        assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
        assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
        assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND id<>$1")
                .bind(scene.thread)
                .fetch_one(scene.world.pool.pool())
                .await?;
        assert_eq!(count, 0);
        scene.end_original().await?;
    }
    Ok(())
}

async fn retained_idle_close(
    scene: &Scene,
    helper: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> Proof<Id> {
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            scene.world.engine().close_idle_program_sessions(at).await?;
            let close: Option<Id> = sqlx::query_scalar(
                "SELECT idle_close_command_id FROM delegated_program_work WHERE helper_id=$1",
            )
            .bind(helper)
            .fetch_one(scene.world.pool.pool())
            .await?;
            if let Some(close) = close {
                return Ok(close);
            }
            tokio::time::sleep(common::config().sweep_every).await;
        }
    })
    .await
    .map_err(|_| "the native ending controls did not settle before the owned context could close")?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_an_idle_context_keeps_the_helper_history_and_the_provider_running() -> Proof {
    let (scene, _) = native_scene().await?;
    let engine = scene.world.engine();
    publish(&scene, "idle-context", "out/idle.png").await;
    let (helper, start) = begin_helper(&scene, "idle-context").await?;
    assert_eq!(
        engine
            .close_idle_program_sessions(scene.world.clock.now() + chrono::Duration::days(1))
            .await?,
        0,
        "an idle timer cannot close running work"
    );
    image(&scene, helper, start.turn_id, "idle-image", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, "idle-context", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    scene.world.wait_turn(start.turn_id, "completed").await;
    scene.world.wait_followed_up(helper).await;
    let completed: chrono::DateTime<chrono::Utc> = scene
        .world
        .scalar("SELECT completed_at FROM turns WHERE id=$1", start.turn_id)
        .await;
    let items = scene
        .world
        .count("SELECT count(*) FROM items WHERE thread_id=$1", helper)
        .await;
    assert_eq!(
        engine
            .close_idle_program_sessions(completed + chrono::Duration::seconds(299))
            .await?,
        0
    );
    let close =
        retained_idle_close(&scene, helper, completed + chrono::Duration::seconds(300)).await?;
    let commands:i64=scene.world.scalar("SELECT count(*) FROM commands WHERE kind IN ('session.close','session.close.fenced') AND body->>'thread_id'=$1::uuid::text",helper).await;
    assert_eq!(commands, 1, "the context has exactly one durable close");
    assert_eq!(
        engine
            .close_idle_program_sessions(completed + chrono::Duration::seconds(301))
            .await?,
        0,
        "another worker resumes the retained close rather than queueing another one"
    );
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            engine.deliver_lifecycle_controls().await?;
            let state: String = sqlx::query_scalar("SELECT state FROM commands WHERE id=$1")
                .bind(close)
                .fetch_one(scene.world.pool.pool())
                .await?;
            if state == "succeeded" {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
            if matches!(state.as_str(), "failed" | "expired" | "cancelled") {
                return Err(format!("the native close ended as {state}").into());
            }
            tokio::time::sleep(common::config().sweep_every).await;
        }
    })
    .await
    .map_err(|_| "the retained native close was not acknowledged within the proof budget")??;
    assert_eq!(
        engine
            .close_idle_program_sessions(completed + chrono::Duration::days(1))
            .await?,
        0
    );
    assert_eq!(
        scene
            .world
            .count("SELECT count(*) FROM items WHERE thread_id=$1", helper)
            .await,
        items
    );
    assert_eq!(
        scene
            .world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", helper)
            .await,
        1
    );
    assert!(can_read(&scene, scene.world.ada, helper).await?);
    assert_eq!(scene.world.began(CommandKind::ProviderStop), 0);
    assert_eq!(scene.world.began(CommandKind::AgentStop), 0);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_idle_time_closes_a_finished_context_without_waiting_for_another_turn() -> Proof {
    let (scene, _) = native_scene().await?;
    scene
        .world
        .set_system("delegation.session_idle_secs", json!(0))
        .await;
    publish(&scene, "zero-idle-context", "out/zero-idle.png").await;
    let (helper, start) = begin_helper(&scene, "zero-idle-context").await?;
    image(&scene, helper, start.turn_id, "zero-idle-image", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, "zero-idle-context", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    scene.world.wait_turn(start.turn_id, "completed").await;
    scene.world.wait_followed_up(helper).await;
    let completed = scene
        .world
        .scalar("SELECT completed_at FROM turns WHERE id=$1", start.turn_id)
        .await;
    // Wait until the native ending controls settle; the ordinary sweeper may finish first.
    let close = retained_idle_close(&scene, helper, completed).await?;
    assert!(!close.is_nil());
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn later_independent_helper_work_is_outside_the_delegated_contexts_idle_timer() -> Proof {
    let (scene, _) = native_scene().await?;
    publish(&scene, "independent-context", "out/independent-context.png").await;
    let (helper, start) = begin_helper(&scene, "independent-context").await?;
    image(
        &scene,
        helper,
        start.turn_id,
        "independent-context-image",
        true,
    )
    .await?;
    let answer = Scene::answer(&scene.world, scene.thread, "independent-context", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    scene.world.wait_turn(start.turn_id, "completed").await;
    let first_close: Id = sqlx::query_scalar(
        "SELECT idle_close_command_id FROM delegated_program_work WHERE helper_id=$1",
    )
    .bind(helper)
    .fetch_one(scene.world.pool.pool())
    .await?;
    let engine = scene.world.engine();
    engine
        .submit_input(
            &Caller::session(scene.world.ada),
            helper,
            common::queued("Continue with my own independent work"),
        )
        .await?;
    let independent = scene.world.turn_start(helper, 1).await;
    scene
        .world
        .answer(
            helper,
            &independent,
            "independent-reply",
            "An independent answer",
        )
        .await;
    scene
        .world
        .say(
            helper,
            common::turn_ended(independent.turn_id, TurnOutcome::Completed),
        )
        .await;
    scene
        .world
        .wait_turn(independent.turn_id, "completed")
        .await;
    assert_eq!(
        engine
            .close_idle_program_sessions(scene.world.clock.now() + chrono::Duration::days(1))
            .await?,
        0
    );
    let close: Option<Id> = scene
        .world
        .scalar(
            "SELECT idle_close_command_id FROM delegated_program_work WHERE helper_id=$1",
            helper,
        )
        .await;
    assert_eq!(
        close,
        Some(first_close),
        "the later human turn cannot acquire a second close"
    );
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generated_helper_work_does_not_recursively_buy_automation_but_later_human_work_can()
-> Proof {
    let (scene, _) = native_scene().await?;
    publish(&scene, "automation-context", "out/automation.png").await;
    let (helper, start) = begin_helper(&scene, "automation-context").await?;
    let mut conn = scene.world.pool.acquire().await?;
    crate::summaries::due(&mut conn, helper, "milestone", scene.world.clock.now()).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM node_summary_state WHERE node_id=$1")
            .bind(helper)
            .fetch_one(&mut *conn)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resource_summary_events WHERE target_id=$1"
        )
        .bind(helper)
        .fetch_one(&mut *conn)
        .await?,
        0
    );
    let children = orch_db::queries::summaries::topic_threads(
        &mut conn,
        scene.world.topic,
        None,
        scene.world.engine().config().sweep_batch,
    )
    .await?;
    assert!(!children.contains(&helper));
    let children = orch_db::queries::summaries::topic_sources(
        &mut conn,
        scene.world.topic,
        None,
        scene.world.engine().config().sweep_batch,
    )
    .await?;
    assert!(
        !children
            .iter()
            .any(|child| child.kind == "thread" && child.id == helper)
    );
    drop(conn);
    image(&scene, helper, start.turn_id, "automation-image", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, "automation-context", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    scene.world.wait_turn(start.turn_id, "completed").await;
    let engine = scene.world.engine();
    engine
        .submit_input(
            &Caller::session(scene.world.ada),
            helper,
            common::queued("My next independent question"),
        )
        .await?;
    let independent = scene.world.turn_start(helper, 1).await;
    scene
        .world
        .answer(
            helper,
            &independent,
            "human-automation-reply",
            "An answer to the human question",
        )
        .await;
    scene
        .world
        .say(
            helper,
            common::turn_ended(independent.turn_id, TurnOutcome::Completed),
        )
        .await;
    scene
        .world
        .wait_turn(independent.turn_id, "completed")
        .await;
    let mut conn = scene.world.pool.acquire().await?;
    crate::summaries::due(&mut conn, helper, "milestone", scene.world.clock.now()).await?;
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM node_summary_state WHERE node_id=$1")
            .bind(helper)
            .fetch_one(&mut *conn)
            .await?
            > 0
    );
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resource_summary_events WHERE target_id=$1"
        )
        .bind(helper)
        .fetch_one(&mut *conn)
        .await?
            > 0
    );
    let children = orch_db::queries::summaries::topic_threads(
        &mut conn,
        scene.world.topic,
        None,
        engine.config().sweep_batch,
    )
    .await?;
    assert!(children.contains(&helper));
    drop(conn);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegated_image_uses_its_native_mode_and_later_human_work_keeps_the_orgs_service()
-> Proof {
    let (scene, provider) = native_scene().await?;
    scene
        .world
        .set(
            scene.world.root,
            orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION,
            json!("service"),
        )
        .await;
    scene.world.set(scene.world.root,orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION_BACKEND,
        json!({"route":"image_generation.delegation","offering":{"provider":provider,"offering":NATIVE_MODEL}})).await;
    let opens = Arc::new(std::sync::Mutex::new(Vec::<
        orch_link_contract::commands::SessionOpen,
    >::new()));
    for kind in [CommandKind::SessionOpen, CommandKind::SessionOpenFenced] {
        let opens = opens.clone();
        scene.world.machine.handler.on(kind, move |command| {
            let (open, fenced) = match command {
                Command::SessionOpen(open) => (open.clone(), false),
                Command::SessionOpenFenced(open) => (open.session.as_ref().clone(), true),
                _ => return Err(CommandError::new(ErrorCode::Conflict, "not a session open")),
            };
            let result = SessionOpenResult {
                native_session_ref: open
                    .native_session_ref
                    .clone()
                    .unwrap_or_else(|| format!("native-{}", open.segment_id)),
                resumed: open.mode == orch_link_contract::commands::SessionMode::Resume,
                import_report: None,
                instruction_sources: vec![],
                approval_reviewer: None,
            };
            opens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(open);
            Ok(if fenced {
                CommandResult::SessionOpenFenced(result)
            } else {
                CommandResult::SessionOpen(result)
            })
        });
    }
    publish(&scene, "native-mode", "out/native-mode.png").await;
    let (helper, start) = begin_helper(&scene, "native-mode").await?;
    let initial = opens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|open| open.thread_id == helper)
        .cloned()
        .ok_or("no helper open")?;
    image(&scene, helper, start.turn_id, "native-mode-image", true).await?;
    let answer = Scene::answer(&scene.world, scene.thread, "native-mode", None).await?;
    assert!(answer.error.is_none(), "{answer:?}");
    scene.world.wait_turn(start.turn_id, "completed").await;
    scene
        .world
        .engine()
        .submit_input(
            &Caller::session(scene.world.ada),
            helper,
            common::queued("Continue with my own independent question"),
        )
        .await?;
    let independent = scene
        .world
        .run_turn(helper, 1, "An independent answer")
        .await;
    let ordinary = opens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .rfind(|open| open.thread_id == helper)
        .cloned()
        .ok_or("missing independent session")?;
    assert!(initial.program_work.is_some());
    assert!(ordinary.program_work.is_none());
    assert_eq!(
        ordinary.mode,
        orch_link_contract::commands::SessionMode::New
    );
    assert!(
        ordinary.native_session_ref.is_none(),
        "the discarded unit has no conversation to resume"
    );
    let mode = independent
        .native_model
        .as_ref()
        .and_then(|native| native.capabilities.as_array())
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["capability"] == "image_generation")
        })
        .and_then(|row| row["mode"].as_str())
        .map(str::to_owned);
    scene.end_original().await?;
    assert_eq!(
        initial.image_generation,
        Some(true),
        "the actual owned native request enables its required tool even when the program defaults off"
    );
    assert!(
        initial.orch_tools.is_empty(),
        "native work cannot recurse through the service's Orch tool"
    );
    assert_eq!(
        mode.as_deref(),
        Some("service"),
        "the next human input uses its ordinary inherited choice"
    );
    let mut conn = scene.world.pool.acquire().await?;
    let thread = threads::thread(&mut conn, helper)
        .await?
        .ok_or("missing helper")?;
    let tools = scene
        .world
        .engine()
        .offered_tools_for_session(&mut conn, &thread, true, scene.world.clock.time())
        .await?;
    assert!(
        tools.contains(&orch_agent_contract::events::OrchTool::ImageGeneration),
        "the ordinary helper remains usable for later work"
    );
    Ok(())
}

async fn recover_one_image_with_two_workers(lose_start_reply: bool) -> Proof {
    let (scene, _) = native_scene().await?;
    let call = "recover-one-native-image";
    expect_task(scene.thread, call);
    let (captured, release_capture) = gate(scene.thread, call, Point::ProgramCaptured);
    publish(&scene, call, "out/recovered-once.png").await;
    tokio::time::timeout(common::PATIENCE, captured).await??;
    let answer = crate::ids::tool_result(scene.thread, call);
    let record: Json = sqlx::query_scalar("SELECT value FROM thread_tool_calls c CROSS JOIN LATERAL jsonb_each(c.plan) p WHERE c.answer_id=$1 AND p.key LIKE 'delegated_capability:%'")
        .bind(answer).fetch_one(scene.world.pool.pool()).await?;
    let helper: Id = serde_json::from_value(record["helper"].clone())?;
    let started = if lose_start_reply {
        let (waiting, release_waiter) = gate(helper, "", Point::ProgramWaiting);
        let (start_ready, release_start) = gate(helper, "", Point::ProgramStart);
        release_capture
            .send(())
            .map_err(|_| "the captured request ended before its helper start")?;
        tokio::time::timeout(common::PATIENCE, start_ready).await??;
        let turn: Id = sqlx::query_scalar("SELECT current_turn_id FROM threads WHERE node_id=$1")
            .bind(helper)
            .fetch_one(scene.world.pool.pool())
            .await?;
        let native = crate::ids::turn_start(turn, 0);
        let before = scene.world.hub.faults().fired();
        scene.world.hub.faults().drop_command_reply(native);
        release_start
            .send(())
            .map_err(|_| "the native start ended before its first send")?;
        common::eventually(
            "the real native acknowledgement and result are lost",
            || async { scene.world.hub.faults().fired() == before + 2 },
        )
        .await;
        let (actual_helper, start) = begin_helper(&scene, call).await?;
        assert_eq!((actual_helper, start.turn_id), (helper, turn));
        tokio::time::timeout(common::PATIENCE, waiting).await??;
        // The provider's event stream confirmed the turn while both command replies were lost.
        // Lose the original waiter with its process, before it can checkpoint an answer.
        scene.world.engine().stop();
        interrupt_task(scene.thread, call).await?;
        drop(release_waiter);
        Some(start)
    } else {
        interrupt_task(scene.thread, call).await?;
        drop(release_capture);
        let helpers: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE id=$1")
            .bind(helper)
            .fetch_one(scene.world.pool.pool())
            .await?;
        assert_eq!(
            helpers, 0,
            "the accepted frozen request survives before its first helper effect"
        );
        None
    };
    let sources = scene
        .world
        .engine()
        .inner
        .capability_services
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or("no installed native codecs")?;
    let install = |worker: &Engine| {
        worker.attach_blobs(scene.store.clone());
        worker.attach_capability_services(sources.clone());
        worker.attach_parcels(Arc::new(crate::parcels::WorkspaceParcels::new(worker)));
    };
    scene
        .world
        .restart_api_configured(|original| original, install)
        .await;
    scene
        .world
        .wait_for_presence_words(scene.world.machine_id, 2)
        .await;
    let first = scene.world.engine();
    let second = scene.world.recovery_worker();
    install(&second);
    second.start();
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let recovering = [first.clone(), second.clone()].map(|worker| {
        let barrier = barrier.clone();
        let request = CallContext {
            machine: scene.world.machine_id,
            thread: scene.thread,
            turn: scene.start.turn_id,
            call_ref: call.into(),
            attempt: None,
            tool: orch_agent_contract::events::OrchTool::ImageGeneration,
            arguments: json!({"prompt":"An orchard","output_paths":["out/recovered-once.png"]}),
        };
        tokio::spawn(async move {
            barrier.wait().await;
            worker.answer_tool_call(&request).await
        })
    });
    barrier.wait().await;
    let start = if let Some(start) = started {
        start
    } else {
        let (actual, start) = begin_helper(&scene, call).await?;
        assert_eq!(actual, helper);
        start
    };
    // Hold one paid-result reader until its sibling has published the pixels and released its pins.
    let (has_pixels, resume_reader) = gate(scene.thread, call, Point::ProgramResult);
    let blob = image(&scene, helper, start.turn_id, "one-recovered-image", true).await?;
    tokio::time::timeout(common::PATIENCE, has_pixels).await??;
    let restored = Scene::answer(&scene.world, scene.thread, call, None).await?;
    resume_reader
        .send(())
        .map_err(|_| "the delayed reader was lost")?;
    for worker in recovering {
        tokio::time::timeout(common::PATIENCE, worker).await???;
    }
    let packets: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_tool_service_results WHERE answer_id=$1")
            .bind(answer)
            .fetch_one(scene.world.pool.pool())
            .await?;
    assert_eq!(
        packets, 0,
        "replay must leave only the permanent image pins"
    );
    assert!(restored.error.is_none(), "{restored:?}");
    let body = restored.result.ok_or("the replay lost the native image")?;
    assert_eq!(body["images"][0]["blob"], json!(blob));
    assert_eq!(body["images"][0]["status"], "ready");
    assert_eq!(
        std::fs::read(scene.directory.path().join("out/recovered-once.png"))?,
        bytes()?
    );
    let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT
        (SELECT count(*) FROM delegated_program_work WHERE request_id=$1),
        (SELECT count(*) FROM thread_inputs WHERE thread_id=$2),
        (SELECT count(*) FROM turns WHERE thread_id=$2),
        (SELECT count(*) FROM commands WHERE serial_key=$3 AND kind IN ('turn.start','turn.start.fenced'))")
        .bind(answer).bind(helper).bind(orch_link_contract::commands::SerialKey::Thread(helper).wire())
        .fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(counts, (1, 1, 1, 1));
    assert_eq!(
        scene
            .world
            .machine
            .handler
            .runs_of(crate::ids::turn_start(start.turn_id, 0)),
        1
    );
    assert_eq!(scene.world.turn_starts(helper).len(), 1);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    second.stop();
    scene.end_original().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_resume_a_crash_before_the_first_paid_image_effect_once() -> Proof {
    recover_one_image_with_two_workers(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_recovering_workers_keep_one_native_image_start_after_its_reply_is_lost() -> Proof {
    recover_one_image_with_two_workers(true).await
}
