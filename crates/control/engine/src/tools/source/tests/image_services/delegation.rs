//! Actual native helpers enter through the same recorded tool, creator and paid-command paths.
use super::*;
use orch_access_contract::permissions::Permission;
use orch_link_contract::{
    commands::{AgentStartResult, SessionOpenResult},
    foreign::ItemPayload,
};

const NATIVE_MODEL: &str = "fixture-native-images";

async fn native_scene() -> Proof<(Scene, Id)> {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    let mut descriptor: Json = scene
        .world
        .scalar(
            "SELECT descriptor FROM agents WHERE id=$1",
            scene.world.agent,
        )
        .await;
    descriptor["x_capability_program"] = json!({
    "title":"Fixture image program", "access":{"native":{"orch_tools":true,"native":{
        "generation":{"enabled":true,"modes":["native"],"default_mode":"native"},
        "image_generation":{"enabled":true,"modes":["off","on"],"default_mode":"off","delegated_mode":"on",
            "eligible_when":{"required":["facts"],"properties":{"facts":{"required":["plan"],"properties":{"plan":{"const":"fixture-paid"}}}}},
            "delegation_label":{"en":"Fixture image program","zh-CN":"测试图像程序"}}
    }}}});
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
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,groups,enabled,source,created_by) VALUES($1,$2,$3,'Native image model',$4,ARRAY['images'],true,'manual',$5)")
        .bind(new_id()).bind(provider).bind(NATIVE_MODEL).bind(json!({"input_images":true,"context_window":10000}))
        .bind(scene.world.ada).execute(scene.world.pool.pool()).await?;
    scene.world.set(scene.thread, orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION_BACKEND,
        json!({"route":"image_generation.delegation","offering":{"provider":provider,"offering":NATIVE_MODEL}})).await;
    Ok((scene, provider))
}

async fn helper(scene: &Scene, call: &str) -> Proof<Id> {
    let answer = crate::ids::tool_result(scene.thread, call);
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            let helper:Option<Id>=sqlx::query_scalar("SELECT (e.value->>'helper')::uuid FROM thread_tool_calls c CROSS JOIN LATERAL jsonb_each(c.plan) e WHERE c.answer_id=$1 AND e.key LIKE 'delegated_capability:%'")
                .bind(answer).fetch_optional(scene.world.pool.pool()).await?;
            if let Some(helper)=helper {return Ok(helper);}
            tokio::time::sleep(common::config().sweep_every).await;
        }
    }).await.map_err(|_| "the real tool did not retain its ordinary helper within the proof budget")?
}

async fn begin_helper(scene: &Scene, call: &str) -> Proof<(Id, TurnStart)> {
    let helper = helper(scene, call).await?;
    let start = tokio::time::timeout(common::PATIENCE, async {
        loop {
            if let Some(start) = scene.world.turn_starts(helper).first().cloned() {
                break start;
            }
            tokio::time::sleep(common::config().sweep_every).await;
        }
    })
    .await
    .map_err(|_| "the ordinary helper did not pass its real native paid boundary")?;
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
        "title":"Native image result","tool":{"kind":"image","blob":blob,"media_type":"image/png","caption":"A retained orchard.",
            "capability":if generated {Some("image_generation")} else {None}},
        "raw":{"name":if generated {"imageGeneration"} else {"imageView"},"input":{"inline":"{}"},"output":null,"is_error":false}
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
    assert!(outcome.error.is_some());
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
async fn the_sign_in_owners_plan_needs_existing_sharing_and_never_owns_the_helpers_content() -> Proof
{
    use orch_db::queries::access::{self, NewEntry, PageKey};
    for shared in [false, true] {
        let (scene, provider) = native_scene().await?;
        let sign_in_owner = scene.world.person("native-signin-owner").await;
        sqlx::query("UPDATE providers SET owner_id=$2 WHERE id=$1")
            .bind(provider)
            .bind(sign_in_owner)
            .execute(scene.world.pool.pool())
            .await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_image_allowance_failure_keeps_its_actual_window_and_reset_for_the_original_call()
-> Proof {
    let (scene, _) = native_scene().await?;
    scene
        .world
        .set(
            scene.thread,
            orch_settings_contract::registry::keys::CALLS_RULES,
            json!([]),
        )
        .await;
    publish(&scene, "native-limit", "out/limited.png").await;
    let (helper, start) = begin_helper(&scene, "native-limit").await?;
    let reset = Time::new(scene.world.clock.now() + chrono::Duration::minutes(15));
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
                turn_id: start.turn_id,
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
    let failed = Scene::answer(&scene.world, scene.thread, "native-limit", None).await?;
    assert!(
        failed.error.is_some(),
        "a failed native image is never reported as a successful result"
    );
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
