//! Real managed ingress, paid output retention and the ordinary guarded native file writer.
use super::*;
mod delegation;
use base64::Engine as _;
use orch_provider_contract::capabilities::{
    Binding, Counter, Declarations, Endpoint, EndpointRule, Interface, Offering, Support,
};
use std::{
    collections::BTreeMap,
    io::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};

const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
const OFFERING: &str = "fixture-images";

#[derive(Clone, Copy)]
enum Shape {
    Compatible,
    ImageApi,
    Hosted,
}

fn bytes() -> Proof<Vec<u8>> {
    Ok(base64::engine::general_purpose::STANDARD.decode(PIXEL)?)
}

fn stat(path: &str, bytes: &[u8], at: Time) -> Proof<orch_api_contract::workspace::FileStat> {
    Ok(orch_api_contract::workspace::FileStat {
        entry: orch_api_contract::workspace::DirEntry {
            name: path.rsplit('/').next().ok_or("no file name")?.into(),
            path: path.into(),
            kind: orch_api_contract::workspace::DirEntryKind::File,
            size: i64::try_from(bytes.len())?,
            mtime: at,
            mode: crate::manifest::DEFAULT_FILE_MODE,
            link_target: None,
            cached: false,
        },
        hash: Some(Hash::of(bytes)),
        media_type: "image/png".into(),
        is_text: false,
    })
}

async fn configured(shape: Shape) -> Proof<(Scene, Id, Arc<AtomicBool>)> {
    Box::pin(configured_with_input_keys(shape, false)).await
}

async fn configured_with_input_keys(
    shape: Shape,
    keyed: bool,
) -> Proof<(Scene, Id, Arc<AtomicBool>)> {
    let scene = if keyed {
        Scene::with_input_keys().await?
    } else {
        Scene::new().await?
    };
    let provider = new_id();
    let mut sources = orch_provider_contract::builtins::BuiltinSources::default();
    let mut binding = None;
    for (name, declaration) in orch_image_backends::declarations()? {
        let interface = match shape {
            Shape::ImageApi => "openrouter-images",
            _ => "openai-images",
        };
        if let Some(found) = declaration.interfaces.get(interface) {
            binding = Some(found.shape.clone());
        }
        sources.register_capabilities(name, declaration)?;
    }
    let mut binding: Binding = binding.ok_or("image declaration absent")?;
    // The fixture reports one charged image measurement, replacing the copied native counter.
    binding
        .counters
        .retain(|_, counter| counter.unit != "image" || counter.reported_cost);
    binding.counters.insert(
        "completed_images".into(),
        Counter {
            pointer: "/usage/images".into(),
            unit: "image".into(),
            successful_only: true,
            ..Counter::default()
        },
    );
    assert_eq!(
        binding
            .counters
            .values()
            .filter(|counter| counter.unit == "image" && !counter.reported_cost)
            .count(),
        1,
        "the fixture's paid receipt has exactly one declared charged quantity"
    );
    sources.register_capabilities("fixture-image-endpoint",Declarations {
        interfaces:BTreeMap::from([("fixture-images-api".into(),Interface {kind:"images".into(),implementation:"image-api".into(),shape:binding,..Interface::default()})]),
        endpoint_rules:vec![EndpointRule {
            when:json!({"required":["protocol"],"properties":{"protocol":{"const":"openai-responses"}}}),
            endpoints:BTreeMap::from([("fixture-images-api".into(),Endpoint {interface:"fixture-images-api".into(),address:"https://image.example.invalid/".into(),credential_role:Some("model".into()),..Endpoint::default()})]),
            offerings:BTreeMap::from([(OFFERING.into(),Offering {kind:"service".into(),endpoint:"fixture-images-api".into(),groups:vec!["images".into()],parameters:json!({"model":"fixture-image-model"}),capabilities:BTreeMap::from([("image_generation".into(),Support {enabled:true,..Support::default()})]),..Offering::default()})]),
            ..EndpointRule::default()
        }],..Declarations::default()
    })?;
    scene
        .world
        .engine()
        .attach_capability_services(Arc::new(sources));
    sqlx::query("INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,upstreams,protocols,machine_id,key_machine_id,arrangements,status,facts_requested_at,service_offerings) VALUES($1,'Image source service',$2,$3,$3,'api',$4,ARRAY['openai-responses'],$5,$5,ARRAY['separated'],'ready',$6,$7)")
        .bind(provider).bind(scene.world.ada).bind(scene.world.root).bind(json!([{"protocol":"openai-responses","base_url":"https://image.example.invalid/v1","options":{}}]))
        .bind(scene.world.machine_id).bind(scene.world.clock.now()).bind(json!({(OFFERING):{"enabled":true,"groups":["images"],"prices":{"image":{"amount":"2","currency":"credits","unit":"image"}}}})).execute(scene.world.pool.pool()).await?;
    let mut conn = scene.world.pool.acquire().await?;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("provider", provider, "live"),
        "any",
        "access.own",
        ("user", Some(scene.world.ada), None),
        None,
        scene.world.clock.now(),
    )
    .await?;
    common::put_secret(&mut conn, provider, "api_key", "synthetic-image-key").await;
    drop(conn);
    let route = if matches!(shape, Shape::Hosted) {
        "image_generation.service-model"
    } else {
        "image_generation.service-api"
    };
    if matches!(shape, Shape::Hosted) {
        sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,groups,enabled,source,created_by) VALUES($1,$2,'fixture-image-model','Image model','{}',ARRAY['images'],true,'manual',$3)")
            .bind(new_id()).bind(provider).bind(scene.world.ada).execute(scene.world.pool.pool()).await?;
    }
    scene
        .world
        .set(
            scene.thread,
            orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION,
            json!("service"),
        )
        .await;
    scene.world.set(scene.thread,orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION_BACKEND,
        json!({"route":route,"offering":{"provider":provider,"offering":if matches!(shape,Shape::Hosted) {"fixture-image-model"}else{OFFERING}}})).await;
    sqlx::query("UPDATE environments SET remote_write=true WHERE id=$1")
        .bind(scene.world.environment)
        .execute(scene.world.pool.pool())
        .await?;
    scene
        .world
        .engine()
        .attach_parcels(Arc::new(crate::parcels::WorkspaceParcels::new(
            &scene.world.engine(),
        )));
    let fail = Arc::new(AtomicBool::new(false));
    let failed = fail.clone();
    let directory = scene.directory.path().to_owned();
    let expected = bytes()?;
    let environment = scene.world.environment;
    let at = scene.source_file_at;
    scene
        .world
        .machine
        .handler
        .on(CommandKind::FsWrite, move |command| {
            let Command::FsWrite(body) = command else {
                return Err(CommandError::new(ErrorCode::Conflict, "not a file write"));
            };
            if failed.load(Ordering::SeqCst) {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "file delivery is temporarily unavailable",
                ));
            }
            if body.environment_id != environment
                || body.blob != Hash::of(&expected)
                || body.mode != Some(crate::manifest::DEFAULT_FILE_MODE)
                || !matches!(
                    body.expected_hash,
                    Some(orch_link_contract::commands::ExpectedHash::Absent)
                )
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "changed image output or overwrite rule",
                ));
            }
            let relative = body
                .path
                .strip_prefix("web/")
                .ok_or_else(|| CommandError::new(ErrorCode::Conflict, "wrong workspace"))?;
            let target = directory.join(relative);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)
                .map_err(|_| {
                    CommandError::new(ErrorCode::Conflict, "another file occupies the destination")
                })?;
            file.write_all(&expected)
                .map_err(|_| CommandError::new(ErrorCode::Internal, "file write failed"))?;
            Ok(CommandResult::FsWrite(
                stat(&body.path, &expected, at).map_err(|_| {
                    CommandError::new(ErrorCode::Internal, "invalid written image metadata")
                })?,
            ))
        });
    let directory = scene.directory.path().to_owned();
    scene
        .world
        .machine
        .handler
        .on(CommandKind::FsStat, move |command| {
            let Command::FsStat(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "not a file observation",
                ));
            };
            if body.environment_id != environment || body.mount.is_some() {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "changed file observation",
                ));
            }
            let relative = body
                .path
                .strip_prefix("web/")
                .ok_or_else(|| CommandError::new(ErrorCode::Conflict, "wrong workspace"))?;
            let bytes = std::fs::read(directory.join(relative))
                .map_err(|_| CommandError::new(ErrorCode::NotFound, "file absent"))?;
            Ok(CommandResult::FsStat(
                stat(&body.path, &bytes, at).map_err(|_| {
                    CommandError::new(ErrorCode::Internal, "invalid file observation")
                })?,
            ))
        });
    scene.world.machine.handler.on(CommandKind::ProviderService,move|command| {
        let Command::ProviderService(request)=command else {return Err(CommandError::new(ErrorCode::Conflict,"not the selected image service"));};
        if request.provider_id!=provider || request.offering!=OFFERING || request.capability!="image_generation" || request.input["model"]!="fixture-image-model" || request.input.get("output_paths").is_some() || request.input.get("delivery_ref").is_some() {
            return Err(CommandError::new(ErrorCode::Conflict,"local destinations or wrong image model reached the provider"));
        }
        let count=request.input["n"].as_i64().ok_or_else(||CommandError::new(ErrorCode::Conflict,"image count missing"))?;
        let produced=if request.input["prompt"]=="two actual results" {2} else {count};
        let rows:Vec<_>=(0..produced).map(|_|json!({"b64_json":PIXEL,"media_type":"image/png","revised_prompt":"A retained orchard."})).collect();
        let measured=i64::try_from(rows.len()).map_err(|_|CommandError::new(ErrorCode::Internal,"output count outside native range"))?;
        let mut observed=orch_agent_contract::requests::UsageRequest {id:"image-source-request".into(),asked:Some(at),end:Some(at),output_streamed:Some(false),..Default::default()};
        observed.finish("done",None,None);
        Ok(CommandResult::ProviderService(orch_link_contract::commands::ProviderServiceResult {output:json!({"data":rows,"usage":{"images":measured}}),output_blob:None,timing:observed.measured(),counters:BTreeMap::from([("completed_images".into(),Some(measured))])}))
    });
    scene.world.machine.handler.on(CommandKind::ProviderGenerate,move|command| {
        let Command::ProviderGenerate(request)=command else {return Err(CommandError::new(ErrorCode::Conflict,"not the selected native image model"));};
        if request.provider_id!=provider || request.model_name!="fixture-image-model" || !request.capture_native_result || request.native_capability.as_ref().is_none_or(|native|native.capability!="image_generation" || native.input.get("output_paths").is_some() || native.input["native_tool"]["type"]!="image_generation") {
            return Err(CommandError::new(ErrorCode::Conflict,"wrong native image input"));
        }
        let usage=orch_link_contract::foreign::TokenUsage {input_tokens:5,output_tokens:2,total_tokens:7,..Default::default()};
        let mut observed=orch_agent_contract::requests::UsageRequest {id:"native-image-source".into(),model:Some(request.model_name.clone()),tokens:usage.into(),asked:Some(at),end:Some(at),output_streamed:Some(false),..Default::default()};
        observed.finish("done",None,None);
        Ok(CommandResult::ProviderGenerate(orch_link_contract::commands::ProviderGenerateResult {native_result_blob:None,output:orch_model_contract::generation::GenerationOutput {message:orch_model_contract::generation::GenerationMessage {role:orch_model_contract::generation::GenerationRole::Assistant,content:vec![],continuation:vec![]},stop:orch_model_contract::generation::GenerationStop::Complete,native_result:json!({"response":{"status":"completed","output":[{"type":"image_generation_call","id":"native-picture","status":"completed","result":PIXEL,"revised_prompt":"A retained orchard."}]},"frames":[]})},usage,timing:observed.measured(),request_ref:Some("native-image-source".into())}))
    });
    Ok((scene, provider, fail))
}

async fn ask(scene: &Scene, name: &str, args: Json) -> Proof<Json> {
    let result = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        name,
        None,
        OrchTool::ImageGeneration,
        args,
    )
    .await?;
    assert!(result.error.is_none(), "{result:?}");
    result.result.ok_or_else(|| "missing image outcome".into())
}

#[tokio::test]
async fn all_three_image_shapes_reach_real_image_items_files_and_one_paid_receipt() -> Proof {
    for shape in [Shape::Compatible, Shape::ImageApi, Shape::Hosted] {
        let (scene, provider, _) = configured(shape).await?;
        let arguments = json!({"prompt":"An orchard","output_paths":["out/picture.png"]});
        let first = ask(&scene, "make-image", arguments.clone()).await?;
        assert_eq!(first["images"][0]["status"], "ready", "{first}");
        let blob = Hash::of(&bytes()?);
        assert_eq!(first["images"][0]["blob"], json!(blob));
        assert_eq!(
            std::fs::read(scene.directory.path().join("out/picture.png"))?,
            bytes()?
        );
        let items:i64=sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND kind='tool_call' AND payload->'tool'->>'kind'='image' AND payload->'tool'->>'blob'=$2")
            .bind(scene.thread).bind(blob.to_string()).fetch_one(scene.world.pool.pool()).await?;
        assert_eq!(items, 1);
        let repeated = Scene::ask(
            &scene.world,
            &scene.source,
            scene.start.turn_id,
            "make-image",
            Some(new_id()),
            OrchTool::ImageGeneration,
            arguments,
        )
        .await?;
        assert_eq!(repeated.result.as_ref(), Some(&first));
        assert_eq!(scene.world.began(CommandKind::FsWrite), 1);
        let rows:Vec<(Id,String,Option<Json>)>=sqlx::query_as("SELECT provider_id,purpose,measurement_states->'service_usage' FROM usage_events WHERE thread_id=$1 AND purpose='image_generation'").bind(scene.thread).fetch_all(scene.world.pool.pool()).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, provider);
        if matches!(shape, Shape::Hosted) {
            assert!(rows[0].2.is_none());
            assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 1);
        } else {
            assert_eq!(
                rows[0].2.as_ref().ok_or("API measurement absent")?["quantity"]["value"],
                1
            );
            assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
        }
        scene.end_original().await?;
    }
    Ok(())
}

#[tokio::test]
async fn file_failure_and_current_paid_refusal_keep_pixels_and_delivery_never_generates_again()
-> Proof {
    let (scene, _, fail) = configured(Shape::Compatible).await?;
    // This case decides direct paid refusal; a configured recovery rule may instead pause.
    scene
        .world
        .set(
            scene.world.topic,
            orch_settings_contract::registry::keys::CALLS_RULES,
            json!([]),
        )
        .await;
    fail.store(true, Ordering::SeqCst);
    let first = ask(
        &scene,
        "failed-file",
        json!({"prompt":"An orchard","output_paths":["out/retry.png"]}),
    )
    .await?;
    assert_eq!(first["images"][0]["status"], "failed", "{first}");
    assert!(!scene.directory.path().join("out/retry.png").exists());
    fail.store(false, Ordering::SeqCst);
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::SERVICES_ALLOWED,
            json!(["never-this-service"]),
        )
        .await;
    let delivered = ask(
        &scene,
        "deliver-image",
        json!({"delivery_ref":first["delivery_ref"],"output_paths":["out/retry.png"]}),
    )
    .await?;
    assert_eq!(delivered["images"][0]["status"], "ready", "{delivered}");
    assert_eq!(delivered["images"][0]["blob"], first["images"][0]["blob"]);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let new = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "refused-new-image",
        None,
        OrchTool::ImageGeneration,
        json!({"prompt":"Another orchard","output_paths":["out/new.png"]}),
    )
    .await?;
    assert!(new.error.is_some());
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let (count,credits):(i64,Option<String>)=sqlx::query_as("SELECT count(*),sum(credits)::text FROM usage_events WHERE thread_id=$1 AND purpose='image_generation'").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(count, 1);
    assert_eq!(credits.as_deref(), Some("2.000000"));
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn equal_image_outputs_keep_distinct_items_and_a_different_existing_file_is_never_replaced()
-> Proof {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    std::fs::write(
        scene.directory.path().join("out/occupied.png"),
        b"a person's other bytes",
    )?;
    let first = ask(
        &scene,
        "two-images",
        json!({"prompt":"Two orchards","output_paths":["out/occupied.png","out/second.png"]}),
    )
    .await?;
    assert_eq!(first["images"][0]["status"], "differs", "{first}");
    assert_eq!(first["images"][1]["status"], "ready");
    assert_eq!(first["images"][0]["blob"], first["images"][1]["blob"]);
    assert_ne!(first["images"][0]["item_id"], first["images"][1]["item_id"]);
    assert_eq!(
        std::fs::read(scene.directory.path().join("out/occupied.png"))?,
        b"a person's other bytes"
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref LIKE 'orch.image_generation:%'").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(count, 2);
    let private:bool=sqlx::query_scalar("SELECT bool_and(c.authority_required AND c.login_authority->>'kind'='image_write') FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.thread_id=$1 AND c.kind IN ('fs.write','fs.stat')").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert!(private);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn the_real_image_write_rechecks_its_destination_after_preparation_and_keeps_the_paid_output()
-> Proof {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    let reference = crate::ids::tool_result(scene.thread, "prepared-image");
    let (prepared, resume) = gate(scene.thread, &reference.to_string(), Point::NativePrepared);
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "prepared-image",
        None,
        OrchTool::ImageGeneration,
        json!({"prompt":"An orchard","output_paths":["out/current.png"]}),
    )
    .await;
    tokio::time::timeout(common::PATIENCE, prepared).await??;
    sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
        .bind(scene.world.environment)
        .execute(scene.world.pool.pool())
        .await?;
    resume
        .send(())
        .map_err(|_| "native image writer disappeared")?;
    let answered = Scene::answer(&scene.world, scene.thread, "prepared-image", None).await?;
    let body = answered
        .result
        .ok_or("accepted image disappeared after destination loss")?;
    assert_eq!(body["images"][0]["status"], "failed");
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let pin:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items i JOIN thread_item_blob_refs p ON p.item_id=i.id WHERE i.thread_id=$1 AND i.payload->'tool'->>'kind'='image')").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert!(pin, "the actual retained image item must keep its blob pin");
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn invalid_destinations_options_and_reply_size_never_start_a_paid_request() -> Proof {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    for (name, args) in [
        (
            "absolute",
            json!({"prompt":"An orchard","output_paths":["/outside.png"]}),
        ),
        (
            "traversal",
            json!({"prompt":"An orchard","output_paths":["../outside.png"]}),
        ),
        (
            "duplicate",
            json!({"prompt":"An orchard","output_paths":["out/same.png","out/same.png"]}),
        ),
        (
            "model-override",
            json!({"prompt":"An orchard","output_paths":["out/picture.png"],"options":{"model":"not-the-offering"}}),
        ),
        (
            "unmaterialized-input",
            json!({"prompt":"An orchard","output_paths":["out/picture.png"],"options":{"image":"secret/local.png"}}),
        ),
        (
            "too-small",
            json!({"prompt":"An orchard","output_paths":["out/picture.png"],"max_bytes":20}),
        ),
    ] {
        let result = Scene::ask(
            &scene.world,
            &scene.source,
            scene.start.turn_id,
            name,
            None,
            OrchTool::ImageGeneration,
            args,
        )
        .await?;
        assert!(result.error.is_some(), "{name}: {result:?}");
    }
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn the_queued_image_write_keeps_its_exact_native_frame_and_original_budget_at_dispatch()
-> Proof {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    let reference = crate::ids::tool_result(scene.thread, "queued-image");
    let (generating, start_generation) = gate(scene.thread, &reference.to_string(), Point::Native);
    Scene::publish(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "queued-image",
        None,
        OrchTool::ImageGeneration,
        json!({"prompt":"An orchard","output_paths":["out/frozen.png"]}),
    )
    .await;
    tokio::time::timeout(common::PATIENCE, generating)
        .await
        .map_err(|_| "the paid image command did not reach its native queue")??;
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    // Generation is the first native command. Install the next gate before letting it run,
    // so the actual file write is retained but has not reached its physical dispatcher.
    let (queued, resume) = gate(scene.thread, &reference.to_string(), Point::Native);
    start_generation
        .send(())
        .map_err(|_| "the queued paid image command disappeared")?;
    tokio::time::timeout(common::PATIENCE, queued)
        .await
        .map_err(|_| "the generated image did not reach its native file-write queue")??;
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let mut conn = scene.world.pool.acquire().await?;
    let id:Id=sqlx::query_scalar("SELECT c.id FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.answer_id=$1 AND c.kind='fs.write'").bind(reference).fetch_one(&mut *conn).await?;
    let command = orch_db::queries::link::command(&mut conn, id)
        .await?
        .ok_or("no real queued image write")?;
    let authority: crate::commands::CommandAuthority = serde_json::from_value(
        orch_db::queries::link::command_authority(&mut conn, id)
            .await?
            .ok_or("no private image source")?,
    )?;
    drop(conn);
    let mut tx = scene.world.pool.begin().await?;
    let current = authority
        .current_callers(
            &mut tx,
            &command,
            scene.world.clock.as_ref(),
            scene.world.access.as_ref(),
        )
        .await?
        .ok_or("the genuine current write was refused")?;
    assert!(!current.is_empty());
    assert!(current.iter().all(|(caller, until)| caller.is_some()
        && until.is_some_and(|until| until.get() <= command.deadline_at)));
    let mut replaced = command.clone();
    replaced.body["path"] = json!("web/out/another.png");
    assert!(
        authority
            .current_callers(
                &mut tx,
                &replaced,
                scene.world.clock.as_ref(),
                scene.world.access.as_ref()
            )
            .await?
            .is_none()
    );
    let mut extended = command.clone();
    extended.deadline_at += chrono::Duration::days(1);
    let retained = authority
        .current_callers(
            &mut tx,
            &extended,
            scene.world.clock.as_ref(),
            scene.world.access.as_ref(),
        )
        .await?
        .ok_or("an extended envelope lost its retained minimum")?;
    assert!(
        retained
            .iter()
            .all(|(_, until)| until.is_some_and(|until| until.get() <= command.deadline_at))
    );
    sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
        .bind(scene.world.environment)
        .execute(&mut *tx)
        .await?;
    assert!(
        authority
            .current_callers(
                &mut tx,
                &command,
                scene.world.clock.as_ref(),
                scene.world.access.as_ref()
            )
            .await?
            .is_none()
    );
    tx.rollback().await?;
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    resume
        .send(())
        .map_err(|_| "the actual native image writer disappeared")?;
    let answered = Scene::answer(&scene.world, scene.thread, "queued-image", None).await?;
    assert_eq!(
        answered.result.as_ref().ok_or("image delivery missing")?["images"][0]["status"],
        "ready"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn accepted_output_that_needs_more_destinations_is_read_again_without_generating_or_counting_the_requested_number()
-> Proof {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    let first = ask(
        &scene,
        "more-than-requested",
        json!({"prompt":"two actual results","output_paths":["out/first.png"]}),
    )
    .await?;
    assert_eq!(first["state"], "accepted", "{first}");
    assert_eq!(scene.world.began(CommandKind::FsWrite), 0);
    let replay=ask(&scene,"retain-two",json!({"delivery_ref":first["delivery_ref"],"output_paths":["out/first.png","out/second.png"]})).await?;
    assert_eq!(
        replay["images"]
            .as_array()
            .ok_or("retained images absent")?
            .len(),
        2
    );
    assert_eq!(replay["images"][0]["status"], "ready");
    assert_eq!(replay["images"][1]["status"], "ready");
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let (quantity,credits):(Json,Option<String>)=sqlx::query_as("SELECT measurement_states#>'{service_usage,quantity}',credits::text FROM usage_events WHERE thread_id=$1 AND purpose='image_generation'").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(
        quantity["value"], 2,
        "measured outputs win over the requested single image"
    );
    assert_eq!(credits.as_deref(), Some("4.000000"));
    scene.end_original().await?;
    Ok(())
}

async fn keep_input_image(scene: &Scene) -> Proof {
    let content = bytes()?;
    let blob = Hash::of(&content);
    scene
        .store
        .put(
            &blob,
            Some(i64::try_from(content.len())?),
            stream_of(vec![Bytes::from(content.clone())]),
        )
        .await?;
    std::fs::write(scene.directory.path().join("out/source.png"), &content)?;
    std::fs::write(scene.directory.path().join("out/mask.png"), &content)?;
    let directory = scene.directory.path().to_owned();
    let environment = scene.world.environment;
    let at = scene.source_file_at;
    scene
        .world
        .machine
        .handler
        .on(CommandKind::FsFetch, move |command| {
            let Command::FsFetch(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "not the image source fetch",
                ));
            };
            if body.environment_id != environment
                || body.mount.is_some()
                || !body
                    .max_bytes
                    .is_some_and(|limit| limit >= content.len() as i64)
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "changed image source or bound",
                ));
            }
            let relative = body
                .path
                .strip_prefix("web/")
                .ok_or_else(|| CommandError::new(ErrorCode::Conflict, "wrong image workspace"))?;
            let actual = std::fs::read(directory.join(relative))
                .map_err(|_| CommandError::new(ErrorCode::NotFound, "no source image"))?;
            if Hash::of(&actual) != blob {
                return Err(CommandError::new(
                    ErrorCode::Changed,
                    "source image changed",
                ));
            }
            Ok(CommandResult::FsFetch(
                orch_link_contract::commands::FsFetchResult {
                    hash: blob,
                    size: actual.len() as i64,
                    mtime_ns: at.get().timestamp_nanos_opt().ok_or_else(|| {
                        CommandError::new(ErrorCode::Internal, "time outside range")
                    })?,
                },
            ))
        });
    Ok(())
}

#[tokio::test]
async fn materialized_images_and_mask_keep_real_source_reads_and_local_paths_out_of_the_paid_request()
-> Proof {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    keep_input_image(&scene).await?;
    let body=ask(&scene,"edit-real-image",json!({"prompt":"Edit this orchard","images":["out/source.png"],"mask":"out/mask.png","output_paths":["out/edited.png"]})).await?;
    assert_eq!(body["images"][0]["status"], "ready", "{body}");
    assert_eq!(scene.world.began(CommandKind::FsFetch), 2);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let request:Json=sqlx::query_scalar("SELECT body FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.thread_id=$1 AND c.kind='provider.service'").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(request["input"]["image"], json!(["source-1"]));
    assert_eq!(request["input"]["mask"], "mask-1");
    assert_eq!(
        request["input_images"]
            .as_array()
            .ok_or("prepared image refs missing")?
            .len(),
        2
    );
    for part in request["input_images"]
        .as_array()
        .ok_or("prepared image refs missing")?
    {
        assert_eq!(part["blob"], json!(Hash::of(&bytes()?)));
        assert_eq!(part["size"], json!(bytes()?.len()));
        assert_eq!(part["url"], "");
        assert!(!part.to_string().contains("out/"));
    }
    assert!(!request["input"].to_string().contains("out/"));
    let private:bool=sqlx::query_scalar("SELECT bool_and(c.authority_required AND c.login_authority->>'kind'='image_input') FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.thread_id=$1 AND c.kind IN ('fs.stat','fs.fetch')").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert!(private);
    let replay=Scene::ask(&scene.world,&scene.source,scene.start.turn_id,"edit-real-image",Some(new_id()),OrchTool::ImageGeneration,json!({"prompt":"Edit this orchard","images":["out/source.png"],"mask":"out/mask.png","output_paths":["out/edited.png"]})).await?;
    assert_eq!(replay.result.as_ref(), Some(&body));
    assert_eq!(scene.world.began(CommandKind::FsFetch), 2);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn a_model_image_edit_receives_actual_image_refs_after_its_declared_prompt() -> Proof {
    let (scene, provider, _) = configured(Shape::Hosted).await?;
    keep_input_image(&scene).await?;
    sqlx::query("UPDATE provider_models SET parameters=jsonb_set(parameters,'{input_images}','true') WHERE provider_id=$1 AND model_name='fixture-image-model'").bind(provider).execute(scene.world.pool.pool()).await?;
    let body=ask(&scene,"model-image-edit",json!({"prompt":"Edit this orchard","images":["out/source.png"],"options":{"action":"edit","quality":"high"},"output_paths":["out/model-edit.png"]})).await?;
    assert_eq!(body["images"][0]["status"], "ready", "{body}");
    let request:Json=sqlx::query_scalar("SELECT body FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.thread_id=$1 AND c.kind='provider.generate'").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(
        request["input"]["messages"][0]["content"][0]["text"],
        "Edit this orchard"
    );
    assert_eq!(
        request["input"]["messages"][0]["content"][1]["blob"],
        json!(Hash::of(&bytes()?))
    );
    assert_eq!(
        request["native_capability"]["input"]["native_tool"]["action"],
        "edit"
    );
    assert_eq!(
        request["native_capability"]["input"]["native_tool"]["quality"],
        "high"
    );
    assert!(
        !request["native_capability"]["input"]
            .to_string()
            .contains("out/")
    );
    assert_eq!(scene.world.began(CommandKind::FsFetch), 1);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 1);
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn source_loss_before_a_native_image_read_starts_no_fetch_or_paid_generation() -> Proof {
    let (scene, _, _) = configured(Shape::Compatible).await?;
    keep_input_image(&scene).await?;
    let reference = crate::ids::tool_result(scene.thread, "source-image-loss");
    let (prepared, resume) = gate(scene.thread, &reference.to_string(), Point::NativePrepared);
    Scene::publish(&scene.world,&scene.source,scene.start.turn_id,"source-image-loss",None,OrchTool::ImageGeneration,json!({"prompt":"Edit this orchard","images":["out/source.png"],"output_paths":["out/lost.png"]})).await;
    tokio::time::timeout(common::PATIENCE, prepared).await??;
    sqlx::query("UPDATE threads SET workspace_dir='changed-directory' WHERE node_id=$1")
        .bind(scene.thread)
        .execute(scene.world.pool.pool())
        .await?;
    resume
        .send(())
        .map_err(|_| "the input image reader disappeared")?;
    let answered = Scene::answer(&scene.world, scene.thread, "source-image-loss", None).await?;
    assert!(answered.error.is_some());
    assert_eq!(scene.world.began(CommandKind::FsFetch), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    sqlx::query("UPDATE threads SET workspace_dir='web' WHERE node_id=$1")
        .bind(scene.thread)
        .execute(scene.world.pool.pool())
        .await?;
    scene.end_original().await?;
    Ok(())
}

#[tokio::test]
async fn declared_inline_image_references_are_verified_data_in_json_not_local_paths_or_unresolved_parts()
-> Proof {
    let (scene, _, _) = configured(Shape::ImageApi).await?;
    keep_input_image(&scene).await?;
    let body=ask(&scene,"inline-image-edit",json!({"prompt":"An orchard variation","images":["out/source.png"],"output_paths":["out/inline.png"]})).await?;
    assert_eq!(body["images"][0]["status"], "ready", "{body}");
    let request:Json=sqlx::query_scalar("SELECT body FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.thread_id=$1 AND c.kind='provider.service'").bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(request["input"]["input_references"][0]["type"], "image_url");
    assert_eq!(
        request["input"]["input_references"][0]["image_url"]["url"],
        format!("data:image/png;base64,{PIXEL}")
    );
    assert!(request["input"].get("image").is_none());
    assert!(request["input_images"].as_array().is_none_or(Vec::is_empty));
    assert!(!request["input"].to_string().contains("out/source.png"));
    assert_eq!(scene.world.began(CommandKind::FsFetch), 1);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    scene.end_original().await?;
    Ok(())
}
