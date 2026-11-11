//! Actual native helpers enter through the same recorded tool, creator and paid-command paths.
use super::*;
use orch_access_contract::permissions::Permission;
use orch_db::queries::access::{self, PageKey};
use orch_link_contract::{
    commands::{AgentStartResult, SessionOpenResult},
    foreign::ItemPayload,
};

const NATIVE_MODEL: &str = "fixture-native-images";

async fn native_scene() -> Proof<(Scene, Id)> {
    let (scene, provider, _) = native_scene_with_file_failure().await?;
    Ok((scene, provider))
}

async fn native_scene_with_file_failure() -> Proof<(Scene, Id, Arc<AtomicBool>)> {
    Box::pin(native_scene_with_input_keys(false)).await
}

async fn native_scene_with_input_keys(keyed: bool) -> Proof<(Scene, Id, Arc<AtomicBool>)> {
    let (scene, _, fail) = Box::pin(configured_with_input_keys(Shape::Compatible, keyed)).await?;
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
            "eligible_when":{"required":["plan"],"properties":{"plan":{"const":"fixture-paid"}}},
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
    Ok(usable.granted().is_some_and(|granted| {
        orch_authz::options::allows_model(&granted.provider(), NATIVE_MODEL, &["images".into()])
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_provider_use_controls_a_shared_sign_in_without_changing_helper_ownership() -> Proof
{
    use orch_db::queries::access::NewEntry;
    for shared in [false, true] {
        let (scene, provider) = native_scene().await?;
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
