//! Real native tool ingress, source receipts and the existing ledger with one capability-only fake extension.
use super::*;
use orch_provider_contract::capabilities::{
    Binding, Counter, Declarations, Endpoint, EndpointRule, Interface, Offering, Support,
};
use orch_types::Clock as _;
use std::collections::BTreeMap;

const OFFERING: &str = "fixture-search";
const FETCH: &str = "fixture-fetch";

async fn provider_owner(
    conn: &mut orch_db::Conn,
    world: &common::World,
    provider: Id,
    person: Id,
) -> Proof {
    // Creation records real Owners entries; owner_id alone is attribution.
    orch_db::queries::access::testing::add_entry(
        conn,
        &orch_db::queries::access::PageKey::new("provider", provider, "live"),
        "any",
        "access.own",
        ("user", Some(person), None),
        None,
        world.clock.now(),
    )
    .await?;
    Ok(())
}

pub(super) async fn install(world: &common::World, thread: Id, fetch: bool) -> Proof {
    let provider = new_id();
    let declarations = Declarations {
        interfaces: BTreeMap::from([(
            "fixture-search-api".into(),
            Interface {
                implementation: "http-json".into(),
                kind: "search".into(),
                shape: Binding {
                    input_schema: json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"},"count":{"type":"integer","minimum":1,"maximum":10}},"additionalProperties":false}),
                    request: json!({"method":"POST","path":"/search","body":{"query":{"from_input":"/query"}}}),
                    result: json!({"schema":{"type":"object","required":["results"]},"collections":[{"pointer":"/results/*","item":{"title":{"from_result":"/title"},"url":{"from_result":"/url"}}}]}),
                    counters: BTreeMap::from([(
                        "web_searches".into(),
                        Counter {
                            constant: Some(1),
                            unit: "search".into(),
                            successful_only: true,
                            ..Counter::default()
                        },
                    )]),
                    ..Binding::default()
                },
                ..Interface::default()
            },
        )]),
        endpoint_rules: vec![EndpointRule {
            when: json!({"required":["protocol"],"properties":{"protocol":{"const":"openai-responses"}}}),
            endpoints: BTreeMap::from([(
                "fixture-search-api".into(),
                Endpoint {
                    interface: "fixture-search-api".into(),
                    address: "https://service.example.invalid/".into(),
                    credential_role: Some("model".into()),
                    ..Endpoint::default()
                },
            )]),
            offerings: BTreeMap::from([
                (
                    FETCH.into(),
                    Offering {
                        kind: "service".into(),
                        endpoint: "fixture-search-api".into(),
                        groups: vec!["web".into()],
                        capabilities: BTreeMap::from([(
                            "page_fetch".into(),
                            Support {
                                enabled: true,
                                ..Support::default()
                            },
                        )]),
                        native_tools: BTreeMap::from([(
                            "page_fetch".into(),
                            Some(Binding {
                                input_schema: json!({"type":"object","required":["url"],"properties":{"url":{"type":"string"}},"additionalProperties":false}),
                                request: json!({"method":"POST","path":"/fetch","body":{"url":{"from_input":"/url"}}}),
                                result: json!({"schema":{"type":"object","required":["content"]},"page":{"url":{"from_input":"/url"},"content":{"from_result":"/content"},"media_type":"text/markdown"}}),
                                counters: BTreeMap::from([(
                                    "web_fetches".into(),
                                    Counter {
                                        constant: Some(1),
                                        unit: "page".into(),
                                        successful_only: true,
                                        ..Counter::default()
                                    },
                                )]),
                                ..Binding::default()
                            }),
                        )]),
                        ..Offering::default()
                    },
                ),
                (
                    OFFERING.into(),
                    Offering {
                        kind: "service".into(),
                        endpoint: "fixture-search-api".into(),
                        groups: vec!["web".into()],
                        capabilities: BTreeMap::from([(
                            "web_search".into(),
                            Support {
                                enabled: true,
                                ..Support::default()
                            },
                        )]),
                        ..Offering::default()
                    },
                ),
            ]),
            ..EndpointRule::default()
        }],
        ..Declarations::default()
    };
    let mut sources = orch_provider_contract::builtins::BuiltinSources::default();
    sources.register_capabilities("fixture-search", declarations)?;
    world.engine().attach_capability_services(Arc::new(sources));
    sqlx::query("INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,upstreams,protocols,machine_id,key_machine_id,arrangements,status,facts_requested_at,service_offerings)
        VALUES($1,'Source search service',$2,$3,$3,'api',$4,ARRAY['openai-responses'],$5,$5,ARRAY['separated'],'ready',$6,$7)")
        .bind(provider).bind(world.ada).bind(world.root)
        .bind(json!([{"protocol":"openai-responses","base_url":"https://service.example.invalid/v1","options":{}}]))
        .bind(world.machine_id).bind(world.clock.now())
        .bind(json!({(OFFERING):{"enabled":true,"groups":["web"],"prices":{"search":{"amount":"2","currency":"credits","unit":"search"}}},(FETCH):{"enabled":true,"groups":["web"],"prices":{"page":{"amount":"3","currency":"credits","unit":"page"}}}}))
        .execute(world.pool.pool()).await?;
    let mut conn = world.pool.acquire().await?;
    provider_owner(&mut conn, world, provider, world.ada).await?;
    common::put_secret(&mut conn, provider, "api_key", "synthetic-service-key").await;
    drop(conn);
    world
        .set(
            world.root,
            orch_settings_contract::registry::keys::AGENTS_WEB_SEARCH_BACKEND,
            json!({"kind":"api","provider_id":provider,"search_service":OFFERING,"fetch_service":FETCH}),
        )
        .await;
    world
        .set(
            thread,
            orch_settings_contract::registry::keys::AGENT_PARAMS,
            json!({"web_search":"service"}),
        )
        .await;
    // Ordinary guard cases observe the refusal itself; recovery cases install their own explicit rule.
    world
        .set(
            world.project,
            orch_settings_contract::registry::keys::CALLS_RULES,
            json!([]),
        )
        .await;
    if fetch {
        world
            .set(
                thread,
                orch_settings_contract::registry::keys::AGENTS_PAGE_FETCH,
                json!("service"),
            )
            .await;
    }
    let at = world.clock.time();
    world.machine.handler.on(CommandKind::ProviderService, move |command| {
        let Command::ProviderService(request) = command else { return Err(CommandError::new(ErrorCode::Conflict,"wrong service command")) };
        let is_fetch = request.capability == "page_fetch" && request.offering == FETCH;
        if request.provider_id != provider || !is_fetch && (request.offering != OFFERING || request.capability != "web_search") {
            return Err(CommandError::new(ErrorCode::Conflict,"wrong selected service"));
        }
        let mut observed = orch_agent_contract::requests::UsageRequest { id:"fixture-service-request".into(),asked:Some(at),end:Some(at),output_streamed:Some(false),..Default::default() };
        observed.finish("done",None,None);
        let quantity = if request.input["query"] == "unknown" { None } else { Some(1) };
        Ok(CommandResult::ProviderService(orch_link_contract::commands::ProviderServiceResult {
            output:if is_fetch {json!({"content":"The exact page returned by the backend"})} else {json!({"results":[{"title":"First source","url":"https://first.example.invalid/"},{"title":"Second source","url":"https://second.example.invalid/"}]})},
            output_blob:None,timing:observed.measured(),counters:BTreeMap::from([((if is_fetch {"web_fetches"} else {"web_searches"}).into(),quantity)]),
        }))
    });
    Ok(())
}

async fn configured() -> Proof<Scene> {
    let scene = Scene::configured_with_web_services(false, true).await?;
    if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
        let mut conn = scene.world.pool.acquire().await?;
        let state:Json=sqlx::query_scalar("SELECT jsonb_build_object('status',p.status,'kind',p.kind,'machine',p.machine_id IS NOT NULL,'key_machine',p.key_machine_id IS NOT NULL,'key',EXISTS(SELECT 1 FROM secrets s WHERE s.owner_kind='provider' AND s.owner_id=p.id AND s.name='api_key'),'search_enabled',p.service_offerings->'fixture-search'->'enabled') FROM providers p WHERE p.name='Source search service' AND p.org_id=$1")
            .bind(scene.world.root).fetch_one(&mut *conn).await?;
        let sources = scene
            .world
            .engine()
            .inner
            .capability_services
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or("fixture declarations missing")?;
        let selected = crate::tools::capability_service::selection::load(
            &mut conn,
            scene.thread,
            sources.capabilities().declarations(),
            "web_search",
        )
        .await?
        .ok_or("fixture selection missing")?;
        let reference = selected
            .offering()
            .ok_or("fixture offering reference missing")?;
        let row = orch_db::queries::resources::routes::providers(&mut conn, &[reference.provider])
            .await?
            .into_iter()
            .next()
            .ok_or("fixture provider missing")?;
        let catalogue = orch_provider_contract::services::catalogue(
            sources.capabilities().declarations(),
            &crate::tools::capability_service::endpoints(&row.upstreams)?,
            &serde_json::from_value(row.service_offerings)?,
        );
        let current = selected.is_current(&mut conn, scene.thread).await?;
        let params: Json = sqlx::query_scalar("SELECT params FROM turns WHERE id=$1")
            .bind(scene.start.turn_id)
            .fetch_one(&mut *conn)
            .await?;
        drop(conn);
        let preview = backend_preview(&scene, true).await?;
        let search = preview
            .iter()
            .find(|resolution| resolution.capability == "web_search")
            .ok_or("fixture search projection missing")?;
        eprintln!(
            "service_fixture stage=ready state={state} installed={} catalogued={} enabled={} opt_in={} frozen_service={} reason={:?}",
            sources.capabilities().extension("fixture-search").is_some(),
            catalogue.offerings.contains_key(OFFERING),
            catalogue
                .offerings
                .get(OFFERING)
                .is_some_and(|offering| offering.enabled),
            current,
            params.get("web_search").and_then(Json::as_str) == Some("service"),
            search.reason
        );
    }
    Ok(scene)
}

#[tokio::test]
async fn declared_backend_references_use_their_own_setting_and_cannot_become_implicit_execution()
-> Proof {
    let scene = configured().await?;
    let sources = scene
        .world
        .engine()
        .inner
        .capability_services
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or("missing installed sources")?;
    let provider: Id = sqlx::query_scalar(
        "SELECT id FROM providers WHERE name='Source search service' AND org_id=$1",
    )
    .bind(scene.world.root)
    .fetch_one(scene.world.pool.pool())
    .await?;
    let mut conn = scene.world.pool.acquire().await?;
    assert!(
        crate::tools::capability_service::selection::load(
            &mut conn,
            scene.thread,
            sources.capabilities().declarations(),
            "image_generation"
        )
        .await?
        .is_none()
    );
    drop(conn);
    scene
        .world
        .set(
            scene.thread,
            orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION,
            json!("service"),
        )
        .await;
    scene.world.set(scene.thread,orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION_BACKEND,
        json!({"route":"image_generation.service-api","offering":{"provider":provider,"offering":OFFERING}})).await;
    let mut conn = scene.world.pool.acquire().await?;
    let selected = crate::tools::capability_service::selection::load(
        &mut conn,
        scene.thread,
        sources.capabilities().declarations(),
        "image_generation",
    )
    .await?
    .ok_or("missing exact configured backend")?;
    assert_eq!(
        selected
            .reference
            .offering
            .as_ref()
            .ok_or("missing offering")?
            .offering,
        OFFERING
    );
    assert!(selected.is_current(&mut conn, scene.thread).await?);
    drop(conn);
    scene
        .world
        .set(
            scene.thread,
            orch_settings_contract::registry::keys::AGENTS_IMAGE_GENERATION,
            json!("off"),
        )
        .await;
    let mut conn = scene.world.pool.acquire().await?;
    assert!(!selected.is_current(&mut conn, scene.thread).await?);
    assert_eq!(
        scene.world.began(CommandKind::ProviderService),
        0,
        "a backend projection is no execution proof"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    Ok(())
}

async fn model_scene() -> Proof<Scene> {
    let scene = configured().await?;
    let provider: Id = sqlx::query_scalar(
        "SELECT id FROM providers WHERE name='Source search service' AND org_id=$1",
    )
    .bind(scene.world.root)
    .fetch_one(scene.world.pool.pool())
    .await?;
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,groups,enabled,source,created_by)
        VALUES($1,$2,'fixture-native-model','Fixture native model',$3,ARRAY['web'],true,'manual',$4)")
        .bind(new_id()).bind(provider).bind(json!({"web_search":["live"]})).bind(scene.world.ada)
        .execute(scene.world.pool.pool()).await?;
    scene
        .world
        .set(
            scene.world.root,
            orch_settings_contract::registry::keys::AGENTS_WEB_SEARCH_BACKEND,
            json!({"kind":"model","provider_id":provider,"model_name":"fixture-native-model"}),
        )
        .await;
    let at = scene.world.clock.time();
    scene.world.machine.handler.on(CommandKind::ProviderGenerate,move|command| {
        let Command::ProviderGenerate(request)=command else {return Err(CommandError::new(ErrorCode::Conflict,"wrong model command"))};
        if request.provider_id!=provider || request.model_name!="fixture-native-model" || !request.capture_native_result
            || request.native_capability.as_ref().is_none_or(|native|native.capability!="web_search") {
            return Err(CommandError::new(ErrorCode::Conflict,"wrong native model service"));
        }
        let usage=orch_link_contract::foreign::TokenUsage {input_tokens:5,output_tokens:2,total_tokens:7,..Default::default()};
        let mut observed=orch_agent_contract::requests::UsageRequest {
            id:"fixture-model-service".into(),model:Some(request.model_name.clone()),tokens:usage.into(),
            asked:Some(at),end:Some(at),web_searches:Some(1),output_streamed:Some(false),..Default::default()
        };
        observed.finish("done",None,None);
        Ok(CommandResult::ProviderGenerate(orch_link_contract::commands::ProviderGenerateResult {
            native_result_blob:None,output:orch_model_contract::generation::GenerationOutput {
                message:orch_model_contract::generation::GenerationMessage {role:orch_model_contract::generation::GenerationRole::Assistant,
                    content:vec![orch_model_contract::generation::GenerationPart::Text {text:"Found the actual source".into()}],continuation:vec![]},
                stop:orch_model_contract::generation::GenerationStop::Complete,
                native_result:json!({"response":{"output":[{"type":"web_search_call","status":"completed","action":{"type":"search","sources":[{"title":"Native model source","url":"https://native.example.invalid/"}]}}]},"frames":[]}),
            },usage,timing:observed.measured(),request_ref:Some("fixture-model-request".into()),
        }))
    });
    Ok(scene)
}

#[tokio::test]
async fn model_backed_search_uses_the_same_native_receipt_and_one_actual_model_ledger_row() -> Proof
{
    let scene = model_scene().await?;
    let args = json!({"query":"Native model source"});
    let result = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "paid-model-search",
        None,
        OrchTool::WebSearch,
        args.clone(),
    )
    .await?;
    assert!(result.error.is_none(), "{result:?}");
    assert_eq!(
        result
            .result
            .as_ref()
            .ok_or("missing native model result")?["results"][0]["url"],
        "https://native.example.invalid/"
    );
    let repeated = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "paid-model-search",
        Some(new_id()),
        OrchTool::WebSearch,
        args,
    )
    .await?;
    assert_eq!(result.result, repeated.result);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 1);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    let rows:Vec<(String,i64,i64,Option<Json>)>=sqlx::query_as("SELECT model,input_tokens,output_tokens,service FROM usage_events WHERE thread_id=$1 AND purpose='web_search'")
        .bind(scene.thread).fetch_all(scene.world.pool.pool()).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "fixture-native-model");
    assert_eq!((rows[0].1, rows[0].2), (5, 2));
    assert!(
        rows[0].3.is_none(),
        "a model-backed call never adds another API unit charge"
    );
    let context: (Option<Id>, Option<Id>) = sqlx::query_as(
        "SELECT turn_id,segment_id FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(scene.world.pool.pool())
    .await?;
    let segment: Id = sqlx::query_scalar("SELECT segment_id FROM turns WHERE id=$1")
        .bind(scene.start.turn_id)
        .fetch_one(scene.world.pool.pool())
        .await?;
    assert_eq!(
        context,
        (Some(scene.start.turn_id), Some(segment)),
        "ledger context keeps the actual turn and segment in their native order"
    );

    Ok(())
}

#[tokio::test]
async fn model_backed_search_rechecks_its_own_model_policy_before_any_paid_request() -> Proof {
    let scene = model_scene().await?;
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::MODELS_ALLOWED,
            json!([scene.start.model.clone()]),
        )
        .await;
    let refused = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "paid-model-blocked",
        None,
        OrchTool::WebSearch,
        json!({"query":"Refused native model source"}),
    )
    .await?;
    assert!(refused.error.is_some());
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    Ok(())
}

#[tokio::test]
async fn native_service_calls_keep_the_original_source_and_count_once_after_tool_replay() -> Proof {
    let scene = configured().await?;
    let args = json!({"query":"two sources","count":2});
    let first = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "paid-search",
        None,
        OrchTool::WebSearch,
        args.clone(),
    )
    .await?;
    assert!(first.error.is_none(), "{first:?}");
    assert_eq!(
        first.result.as_ref().ok_or("missing search result")?["results"]
            .as_array()
            .ok_or("missing links")?
            .len(),
        2
    );
    let replay = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "paid-search",
        Some(new_id()),
        OrchTool::WebSearch,
        args,
    )
    .await?;
    assert_eq!(first.result, replay.result);
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let row:(Id,Id,Id,String,String,Option<String>,Json) = sqlx::query_as("SELECT provider_id,user_id,turn_id,purpose,model,credits::text,service FROM usage_events WHERE thread_id=$1 AND purpose='web_search'")
        .bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_ne!(
        row.0, scene.world.provider,
        "the service pays through its own actual provider"
    );
    assert_eq!(row.1, scene.world.ada);
    assert_eq!(row.2, scene.start.turn_id);
    assert_eq!(row.3, "web_search");
    assert_eq!(row.4, "");
    assert_eq!(row.5.as_deref(), Some("2.000000"));
    assert_eq!(
        row.6["quantity"]["value"], 1,
        "two links are one actually reported charged call"
    );
    let private:bool=sqlx::query_scalar("SELECT bool_and(c.authority_required AND c.login_authority->>'kind'='capability_service' AND NOT (c.body ? 'login_authority')) FROM commands c JOIN thread_tool_commands e ON e.command_id=c.id WHERE e.thread_id=$1 AND c.kind='provider.service' AND e.queue_provider_id=($2::uuid)")
        .bind(scene.thread).bind(row.0).fetch_one(scene.world.pool.pool()).await?;
    assert!(
        private,
        "private original authority and the provider queue belong to the actual receipt"
    );
    Ok(())
}

#[tokio::test]
async fn a_service_disabled_after_preparation_is_refused_before_its_first_paid_command() -> Proof {
    let scene = Arc::new(configured().await?);
    let answer = crate::ids::tool_result(scene.thread, "paid-disable-after-prepare");
    let (waiting, resume) = gate(scene.thread, &answer.to_string(), Point::NativePrepared);
    let work = Arc::clone(&scene);
    let task = tokio::spawn(async move {
        Scene::ask(
            &work.world,
            &work.source,
            work.start.turn_id,
            "paid-disable-after-prepare",
            None,
            OrchTool::WebSearch,
            json!({"query":"one source"}),
        )
        .await
    });
    reached("service-prepared", waiting).await?;
    sqlx::query("UPDATE providers SET service_offerings=jsonb_set(service_offerings,ARRAY[$2,'enabled'],'false'::jsonb) WHERE name='Source search service' AND org_id=$1")
        .bind(scene.world.root).bind(OFFERING).execute(scene.world.pool.pool()).await?;
    resume
        .send(())
        .map_err(|_| "service preparation waiter disappeared")?;
    let result = task.await??;
    assert!(result.error.is_some());
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    let calls:i64=sqlx::query_scalar("SELECT count(*) FROM thread_tool_commands e JOIN commands c ON c.id=e.command_id WHERE e.thread_id=$1 AND c.kind='provider.service'")
        .bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert_eq!(calls, 0);
    Ok(())
}

#[tokio::test]
async fn current_service_policy_rechecks_every_scope_and_preserves_an_unknown_native_quantity()
-> Proof {
    let scene = configured().await?;
    scene
        .world
        .set(
            scene.world.root,
            orch_settings_contract::registry::keys::SERVICES_ALLOWED,
            json!(["fixture-*"]),
        )
        .await;
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::SERVICES_ALLOWED,
            json!(["@web"]),
        )
        .await;
    let result = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "paid-unknown",
        None,
        OrchTool::WebSearch,
        json!({"query":"unknown"}),
    )
    .await?;
    assert!(result.error.is_none(), "{result:?}");
    let row:(Option<String>,Json)=sqlx::query_as("SELECT credits::text,service FROM usage_events WHERE thread_id=$1 AND purpose='web_search'")
        .bind(scene.thread).fetch_one(scene.world.pool.pool()).await?;
    assert!(row.0.is_none());
    assert_ne!(row.1["quantity"]["state"], "told");
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::SERVICES_ALLOWED,
            json!([]),
        )
        .await;
    let refused = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "paid-policy-blocked",
        None,
        OrchTool::WebSearch,
        json!({"query":"now blocked"}),
    )
    .await?;
    assert!(refused.error.is_some());
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    Ok(())
}

#[tokio::test]
async fn standalone_paid_work_needs_its_own_native_source_and_cannot_borrow_a_turn_receipt() -> Proof
{
    use crate::tools::capability_service::PaidAttribution;
    let scene = configured().await?;
    let result = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "native-boundary",
        None,
        OrchTool::WebSearch,
        json!({"query":"source-bound"}),
    )
    .await?;
    assert!(result.error.is_none(), "{result:?}");
    let retained = {
        let mut conn = scene.world.pool.acquire().await?;
        let id: Id = sqlx::query_scalar("SELECT c.id FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.thread_id=$1 AND c.kind='provider.service'")
            .bind(scene.thread).fetch_one(&mut *conn).await?;
        orch_db::queries::link::command_with_authority(&mut conn, id)
            .await?
            .ok_or("missing actual command")?
    };
    let request: orch_link_contract::commands::ProviderService =
        serde_json::from_value(retained.command.body.clone())?;
    let outgoing = crate::commands::Outgoing::new(
        retained.command.id,
        retained.command.machine_id,
        orch_link_contract::commands::SerialKey::Provider(request.provider_id),
        &request,
    )?;
    let attribution = PaidAttribution {
        place: scene.thread,
        org: scene.world.root,
        project: scene.world.project,
        topic: scene.world.topic,
        thread: Some(scene.thread),
        conversation: None,
        person: Some(scene.world.ada),
        purpose: "web_search".into(),
        harness_kind: None,
    };
    assert!(
        scene
            .world
            .engine()
            .prepare_paid_service_command(outgoing.clone(), attribution.clone())
            .await
            .is_err(),
        "provider/offering/body ids and a billing person cannot create native source authority"
    );
    let authority: crate::commands::CommandAuthority =
        serde_json::from_value(retained.authority.ok_or("missing actual native source")?)?;
    assert!(
        scene
            .world
            .engine()
            .prepare_paid_service_command(outgoing.with_authority(authority), attribution)
            .await
            .is_err(),
        "a managed tool command cannot be recast as standalone work"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let counted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(scene.world.pool.pool())
    .await?;
    assert_eq!(counted, 1);
    Ok(())
}

#[tokio::test]
async fn paid_fetch_is_independent_of_search_and_keeps_its_actual_page_unit() -> Proof {
    let search_only = configured().await?;
    let mut conn = search_only.world.pool.acquire().await?;
    let thread = orch_db::queries::threads::thread(&mut conn, search_only.thread)
        .await?
        .ok_or("missing thread")?;
    let tools = search_only
        .world
        .engine()
        .offered_tools_for_session(&mut conn, &thread, true, search_only.world.clock.time())
        .await?;
    assert!(tools.contains(&orch_agent_contract::events::OrchTool::WebSearch));
    assert!(
        !tools.contains(&orch_agent_contract::events::OrchTool::WebFetch),
        "a configured fetch backend plus paid search never opts into page fetching"
    );
    drop(conn);
    assert_eq!(search_only.world.began(CommandKind::ProviderService), 0);
    let fetch = Scene::configured_with_web_modes(false, true, true).await?;
    let result = Scene::ask(
        &fetch.world,
        &fetch.source,
        fetch.start.turn_id,
        "paid-page",
        None,
        OrchTool::WebFetch,
        json!({"url":"https://page.example.invalid/actual"}),
    )
    .await?;
    assert!(result.error.is_none(), "{result:?}");
    let page = result.result.ok_or("missing page")?;
    assert_eq!(page["url"], "https://page.example.invalid/actual");
    assert_eq!(page["content"], "The exact page returned by the backend");
    let row:(String,String,Option<String>,Json)=sqlx::query_as("SELECT purpose,model,credits::text,service FROM usage_events WHERE thread_id=$1 AND purpose='page_fetch'")
        .bind(fetch.thread).fetch_one(fetch.world.pool.pool()).await?;
    assert_eq!(row.0, "page_fetch");
    assert!(row.1.is_empty());
    assert_eq!(row.2.as_deref(), Some("3"));
    assert_eq!(row.3["offering"], FETCH);
    assert_eq!(row.3["unit"], "page");
    fetch
        .world
        .set(
            fetch.thread,
            orch_settings_contract::registry::keys::AGENTS_PAGE_FETCH,
            json!("off"),
        )
        .await;
    let refused = Scene::ask(
        &fetch.world,
        &fetch.source,
        fetch.start.turn_id,
        "paid-page-off",
        None,
        OrchTool::WebFetch,
        json!({"url":"https://page.example.invalid/refused"}),
    )
    .await?;
    assert!(refused.error.is_some());
    assert_eq!(fetch.world.began(CommandKind::ProviderService), 1);
    Ok(())
}

async fn backend_preview(
    scene: &Scene,
    include_backend: bool,
) -> Proof<Vec<orch_provider_contract::capabilities::Resolution>> {
    let mut descriptor: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(common::descriptor())?;
    descriptor.extensions.insert("x_capability_program",json!({
        "access":{"forwarded":{"orch_tools":true,"interfaces":["openai-responses"]}},
        "native_switches":{"native_lookup":{"capability":"web_search","values":{"disabled":false,"live":true}}}
    }))?;
    backend_preview_for_program(
        scene,
        include_backend,
        &descriptor,
        "forwarded",
        Some("service"),
    )
    .await
}

async fn backend_preview_for_program(
    scene: &Scene,
    include_backend: bool,
    descriptor: &orch_agent_contract::descriptor::AgentDescriptor,
    access: &str,
    mode: Option<&str>,
) -> Proof<Vec<orch_provider_contract::capabilities::Resolution>> {
    use crate::capability_routes::{self, ThreadProjection};
    let mut conn = scene.world.pool.acquire().await?;
    let sources = scene
        .world
        .engine()
        .inner
        .capability_services
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or("missing installed sources")?;
    let mut settings = BTreeMap::from([("agent.params".into(), json!({"web_search":mode}))]);
    if include_backend {
        let backend = crate::tools::capability_service::selection::setting(
            &mut conn,
            scene.thread,
            orch_settings_contract::registry::keys::AGENTS_WEB_SEARCH_BACKEND,
        )
        .await?
        .ok_or("missing actual backend")?;
        settings.insert("agents.web_search_backend".into(), backend);
    }
    let mut snapshot = capability_routes::project_with_sources(
        ThreadProjection {
            descriptor,
            access,
            arrangement: None,
            provider_id: scene.world.provider,
            provider: None,
            models: &[],
            current: Some(&scene.start.model),
            allowed: true,
            settings: settings.clone(),
        },
        &sources,
    )?;
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(scene.world.ada),
        scene.world.clock.time(),
    )
    .await?;
    scene
        .world
        .engine()
        .join_capability_backends(
            &mut conn,
            &mut snapshot,
            &settings,
            capability_routes::BackendRead {
                place: scene.thread,
                principals: &[principal],
                person: Some(scene.world.ada),
                harness_kind: Some(&descriptor.kind),
                provider: None,
            },
        )
        .await?;
    Ok(snapshot.resolve())
}

// EN: Thread → Parameters → Web search: inspect Program default, explicitly choose Service, then Disabled.
// 中文：线程 → 参数 → 网络搜索：查看程序默认值，明确选择“服务”，然后选择“关闭”。
#[tokio::test]
async fn actual_managed_program_controls_join_the_same_opt_in_backend_without_native_model_support()
-> Proof {
    use orch_agent_contract::credentials::CredentialPath;
    use orch_agent_contract::descriptor::{Placement, ProviderKind};
    use orch_agent_contract::parameters::{Eligibility, ModelParameters, Resolution};
    let scene = configured().await?;
    for descriptor in [
        orch_adapter_codex::descriptor::descriptor("0.167.0"),
        orch_adapter_claude_code::descriptor::descriptor("2.1.296", false),
    ] {
        let control = descriptor
            .parameters
            .iter()
            .find(|parameter| parameter.key == "web_search")
            .ok_or("the actual descriptor omitted its managed search control")?;
        for (access, path) in [
            ("native", CredentialPath::Native),
            ("forwarded", CredentialPath::Forwarded),
            ("shared", CredentialPath::Shared),
            ("handover", CredentialPath::Handover),
        ] {
            let context = Eligibility {
                credential_path: path,
                provider_kind: if access == "forwarded" {
                    ProviderKind::Api
                } else {
                    ProviderKind::Subscription
                },
                environment_placement: Placement::Managed,
                model: ModelParameters::default(),
                harness_version: None,
            };
            assert_eq!(
                control.resolve(&context, &BTreeMap::new(), Some(&json!("service"))),
                Resolution::Send(json!("service")),
                "{}/{access}",
                descriptor.kind
            );
            let explicit =
                backend_preview_for_program(&scene, true, &descriptor, access, Some("service"))
                    .await?;
            let search = explicit
                .iter()
                .find(|resolution| resolution.capability == "web_search")
                .ok_or("missing search")?;
            assert_eq!(
                search.mode.as_deref(),
                Some("service"),
                "{}/{access}",
                descriptor.kind
            );
            assert_eq!(
                search
                    .route_view
                    .as_ref()
                    .ok_or("the genuine enabled backend is missing")?
                    .kind,
                "service"
            );
            assert!(search.modes.iter().any(|mode| mode == "service"));
            assert!(
                search.provider.is_none() && search.offering.is_none() && search.prices.is_empty(),
                "a selected service never borrows metadata Read"
            );
            if descriptor.kind == "codex" {
                assert_eq!(
                    search.native_config.get("web_search"),
                    Some(&json!("disabled"))
                );
            }
            let unset =
                backend_preview_for_program(&scene, true, &descriptor, access, None).await?;
            let search = unset
                .iter()
                .find(|resolution| resolution.capability == "web_search")
                .ok_or("missing default search")?;
            assert_ne!(
                search.mode.as_deref(),
                Some("service"),
                "opening never opts into paid work"
            );
            assert!(
                search
                    .route_view
                    .as_ref()
                    .is_none_or(|route| route.kind != "service")
            );
        }
    }
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    Ok(())
}

#[tokio::test]
async fn paid_backend_previews_share_actual_use_policies_and_never_read_hidden_backend_settings_or_metadata()
-> Proof {
    let scene = configured().await?;
    let resolutions = backend_preview(&scene, true).await?;
    let search = resolutions
        .iter()
        .find(|resolved| resolved.capability == "web_search")
        .ok_or("missing search")?;
    assert_eq!(
        search
            .route_view
            .as_ref()
            .ok_or("enabled service missing")?
            .kind,
        "service"
    );
    assert!(search.modes.iter().any(|mode| mode == "service"));
    assert!(
        search.provider.is_none() && search.offering.is_none() && search.prices.is_empty(),
        "Use never borrows metadata Read"
    );
    assert_eq!(
        search.native_config.get("native_lookup"),
        Some(&json!(false))
    );
    let hidden = backend_preview(&scene, false).await?;
    let hidden = hidden
        .iter()
        .find(|resolved| resolved.capability == "web_search")
        .ok_or("missing hidden choice")?;
    assert!(
        hidden.route_view.is_none(),
        "a database choice hidden from this operation is never loaded behind its reader"
    );
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::SERVICES_ALLOWED,
            json!([]),
        )
        .await;
    let refused = backend_preview(&scene, true).await?;
    let refused = refused
        .iter()
        .find(|resolved| resolved.capability == "web_search")
        .ok_or("missing refused search")?;
    assert!(refused.route_view.is_none());
    assert_eq!(refused.reason.as_deref(), Some("current_authority_refused"));
    assert_eq!(
        refused.native_config.get("native_lookup"),
        Some(&json!(false))
    );
    assert_eq!(
        scene.world.began(CommandKind::ProviderService),
        0,
        "projection is no paid execution"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    Ok(())
}

#[tokio::test]
async fn a_model_backend_preview_uses_its_actual_composed_endpoint_and_own_allowed_model_list()
-> Proof {
    let scene = model_scene().await?;
    let resolutions = backend_preview(&scene, true).await?;
    let search = resolutions
        .iter()
        .find(|resolved| resolved.capability == "web_search")
        .ok_or("missing search")?;
    assert_eq!(
        search
            .route_view
            .as_ref()
            .ok_or("missing enabled model backend")?
            .kind,
        "service"
    );
    assert_eq!(
        search
            .route
            .as_ref()
            .and_then(|route| route.offering.as_ref())
            .map(|reference| reference.offering.as_str()),
        Some("fixture-native-model")
    );
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::MODELS_ALLOWED,
            json!([scene.start.model.clone()]),
        )
        .await;
    let refused = backend_preview(&scene, true).await?;
    assert!(
        refused
            .iter()
            .find(|resolved| resolved.capability == "web_search")
            .ok_or("missing search")?
            .route_view
            .is_none()
    );
    assert_eq!(scene.world.began(CommandKind::ProviderGenerate), 0);
    Ok(())
}

async fn recorded_service_pause(
    scene: &Scene,
    reset: Option<Time>,
) -> Proof<crate::tools::capability_service::selection::Selection> {
    use orch_provider_contract::calls::Purpose;
    let until = if reset.is_some() { "reset" } else { "person" };
    scene.world.set(scene.world.project,orch_settings_contract::registry::keys::CALLS_RULES,
        json!([{"id":"pause-web","when":{"kind":"unavailable","purpose":"web_search"},"steps":[{"type":"wait","until":until}]}])).await;
    let sources = scene
        .world
        .engine()
        .inner
        .capability_services
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or("missing sources")?;
    let mut tx = scene.world.pool.begin().await?;
    let selected = crate::tools::capability_service::selection::load(
        &mut tx,
        scene.thread,
        sources.capabilities().declarations(),
        "web_search",
    )
    .await?
    .ok_or("missing actual backend")?;
    let provider = selected.offering().ok_or("missing offering")?.provider;
    let root = new_id();
    let row=crate::calls::reserve_service(&mut tx,&crate::calls::ServiceCall {
        node:scene.thread,purpose:Purpose::WebSearch,root,workflow:None,task:None,provider,model:"",dimensions:None,
    },root,&json!({"code":"unavailable","message":"The selected service is unavailable","retryable":true,"resets_at":reset}),
        json!({"provider_id":provider,"model":"","capability_route":selected.reference}),scene.world.clock.now()).await?.ok_or("rule was not reserved")?;
    let pause = crate::calls::tool_pause::retain(
        &mut tx,
        &row,
        &selected,
        if reset.is_some() {
            "reset_after_deadline"
        } else {
            "wait_for_person"
        },
        scene.world.clock.now(),
    )
    .await?;
    assert_eq!(pause.until, reset);
    assert_eq!(pause.needs_person, reset.is_none());
    assert_eq!(pause.result()["state"], "paused");
    tx.commit().await?;
    Ok(selected)
}

#[tokio::test]
async fn a_service_pause_uses_the_existing_resume_control_without_replaying_its_closed_request()
-> Proof {
    use orch_provider_contract::calls::Purpose;
    let scene = configured().await?;
    let selected = recorded_service_pause(&scene, None).await?;
    let mut conn = scene.world.pool.acquire().await?;
    let pause = crate::calls::tool_pause::active(
        &mut conn,
        scene.thread,
        Purpose::WebSearch,
        &selected,
        scene.world.clock.now(),
    )
    .await?
    .ok_or("service was not paused")?;
    assert!(pause.needs_person);
    let view = crate::calls::view_at(&mut conn, scene.thread, scene.world.clock.now())
        .await?
        .ok_or("pause missing from existing recovery UI")?;
    assert_eq!(view.service.as_deref(), Some("web_search"));
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(scene.world.ada),
        scene.world.clock.time(),
    )
    .await?;
    drop(conn);
    scene
        .world
        .engine()
        .control_call_recovery(
            scene.world.pool.begin().await?,
            principal,
            scene.thread,
            orch_api_contract::calls::CallRecoveryAction::RetryNow,
        )
        .await?;
    let mut conn = scene.world.pool.acquire().await?;
    assert!(
        crate::calls::tool_pause::active(
            &mut conn,
            scene.thread,
            Purpose::WebSearch,
            &selected,
            scene.world.clock.now()
        )
        .await?
        .is_none()
    );
    assert_eq!(
        scene.world.began(CommandKind::ProviderService),
        0,
        "resuming a service never executes the closed request"
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(rows, 0);
    Ok(())
}

#[tokio::test]
async fn a_reset_pause_expires_at_its_real_time_without_spend_or_a_stale_backend_gate() -> Proof {
    use orch_provider_contract::calls::Purpose;
    let scene = configured().await?;
    let reset = Time::new(scene.world.clock.now() + chrono::Duration::seconds(600));
    let selected = recorded_service_pause(&scene, Some(reset)).await?;
    let mut conn = scene.world.pool.acquire().await?;
    let pause = crate::calls::tool_pause::active(
        &mut conn,
        scene.thread,
        Purpose::WebSearch,
        &selected,
        scene.world.clock.now(),
    )
    .await?
    .ok_or("missing reset pause")?;
    assert_eq!(pause.result()["until"], serde_json::to_value(reset)?);
    assert!(!pause.needs_person);
    assert!(
        crate::calls::tool_pause::active(
            &mut conn,
            scene.thread,
            Purpose::WebSearch,
            &selected,
            reset.get()
        )
        .await?
        .is_none()
    );
    assert!(
        crate::calls::view_at(&mut conn, scene.thread, reset.get())
            .await?
            .is_none()
    );
    drop(conn);
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::CALLS_RULES,
            json!([]),
        )
        .await;
    let mut conn = scene.world.pool.acquire().await?;
    assert!(
        crate::calls::tool_pause::active(
            &mut conn,
            scene.thread,
            Purpose::WebSearch,
            &selected,
            scene.world.clock.now()
        )
        .await?
        .is_none(),
        "a removed rule does not retain an obsolete service pause"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    Ok(())
}

#[tokio::test]
async fn a_managed_tool_keeps_the_first_proxy_deadline_across_replay_and_refuses_expired_work()
-> Proof {
    let scene = configured().await?;
    let first_deadline = Time::new(scene.world.clock.now() + chrono::Duration::seconds(20));
    let stream = Stream::Session {
        thread_id: scene.source.thread_id,
        segment_id: scene.source.segment_id,
    };
    scene
        .world
        .machine
        .publish(
            &stream,
            &Event::OrchToolCall(OrchToolCall {
                deadline: Some(first_deadline),
                turn_id: scene.start.turn_id,
                call_ref: "bounded-service".into(),
                attempt: None,
                tool: OrchTool::WebSearch,
                arguments: json!({"query":"bounded"}),
            }),
        )
        .await;
    let first = Scene::answer(&scene.world, scene.thread, "bounded-service", None).await?;
    assert!(first.error.is_none(), "{first:?}");
    let answer = crate::ids::tool_result(scene.thread, "bounded-service");
    let mut conn = scene.world.pool.acquire().await?;
    let deadline: Json = sqlx::query_scalar(
        "SELECT plan->'capability_deadline' FROM thread_tool_calls WHERE answer_id=$1",
    )
    .bind(answer)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(deadline, serde_json::to_value(first_deadline)?);
    let issued:chrono::DateTime<chrono::Utc>=sqlx::query_scalar("SELECT c.deadline_at FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.answer_id=$1 AND c.kind='provider.service'")
        .bind(answer).fetch_one(&mut *conn).await?;
    assert_eq!(issued, first_deadline.get());
    drop(conn);
    let attempt = new_id();
    scene
        .world
        .machine
        .publish(
            &stream,
            &Event::OrchToolCall(OrchToolCall {
                deadline: Some(Time::new(
                    scene.world.clock.now() + chrono::Duration::hours(1),
                )),
                turn_id: scene.start.turn_id,
                call_ref: "bounded-service".into(),
                attempt: Some(attempt),
                tool: OrchTool::WebSearch,
                arguments: json!({"query":"bounded"}),
            }),
        )
        .await;
    let repeated =
        Scene::answer(&scene.world, scene.thread, "bounded-service", Some(attempt)).await?;
    assert_eq!(first.result, repeated.result);
    scene
        .world
        .machine
        .publish(
            &stream,
            &Event::OrchToolCall(OrchToolCall {
                deadline: Some(Time::new(
                    scene.world.clock.now() - chrono::Duration::seconds(1),
                )),
                turn_id: scene.start.turn_id,
                call_ref: "expired-service".into(),
                attempt: None,
                tool: OrchTool::WebSearch,
                arguments: json!({"query":"must not run"}),
            }),
        )
        .await;
    assert!(
        Scene::answer(&scene.world, scene.thread, "expired-service", None)
            .await?
            .error
            .is_some()
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let mut conn = scene.world.pool.acquire().await?;
    let unchanged: Json = sqlx::query_scalar(
        "SELECT plan->'capability_deadline' FROM thread_tool_calls WHERE answer_id=$1",
    )
    .bind(answer)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(unchanged, deadline);
    Ok(())
}

#[tokio::test]
async fn a_wait_person_rule_returns_a_paused_tool_before_spend_and_fresh_calls_wait_for_resume()
-> Proof {
    let scene = configured().await?;
    scene.world.set(scene.world.project,orch_settings_contract::registry::keys::CALLS_RULES,
        json!([{"id":"person-before-spend","when":{"kind":"unavailable","purpose":"web_search"},"steps":[{"type":"wait","until":"person"}]}])).await;
    // The installed tool stays offered; a current policy denial is handled before paid IO.
    scene
        .world
        .set(
            scene.world.project,
            orch_settings_contract::registry::keys::SERVICES_ALLOWED,
            json!([]),
        )
        .await;
    let paused = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "person-rule",
        None,
        OrchTool::WebSearch,
        json!({"query":"needs a person"}),
    )
    .await?;
    assert!(paused.error.is_none(), "{paused:?}");
    let result = paused.result.ok_or("missing paused result")?;
    assert_eq!(result["state"], "paused");
    assert_eq!(result["reason"], "wait_for_person");
    assert_eq!(result["needs_person"], true);
    assert!(
        result.get("results").is_none(),
        "a paused tool never pretends it completed an empty search"
    );
    scene
        .world
        .unset(
            scene.world.project,
            orch_settings_contract::registry::keys::SERVICES_ALLOWED,
        )
        .await;
    let still_paused = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "fresh-but-paused",
        None,
        OrchTool::WebSearch,
        json!({"query":"new actual call"}),
    )
    .await?;
    assert_eq!(
        still_paused.result.ok_or("missing service pause")?["state"],
        "paused"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 0);
    let mut conn = scene.world.pool.acquire().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(count, 0);
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(scene.world.ada),
        scene.world.clock.time(),
    )
    .await?;
    drop(conn);
    scene
        .world
        .engine()
        .control_call_recovery(
            scene.world.pool.begin().await?,
            principal,
            scene.thread,
            orch_api_contract::calls::CallRecoveryAction::RetryNow,
        )
        .await?;
    let completed = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "after-person-resume",
        None,
        OrchTool::WebSearch,
        json!({"query":"actual resumed call"}),
    )
    .await?;
    assert!(completed.error.is_none(), "{completed:?}");
    assert!(completed.result.ok_or("missing actual search")?["results"].is_array());
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    Ok(())
}

#[tokio::test]
async fn reset_beyond_original_tool_deadline_returns_its_time_then_the_next_call_runs_after_reset()
-> Proof {
    use std::sync::atomic::{AtomicBool, Ordering};
    let clock = Arc::new(orch_types::ManualClock::new(chrono::Utc::now()));
    let scene = Scene::configured_with_web_clock(false, true, false, Some(clock.clone())).await?;
    let reset = Time::new(clock.now() + chrono::Duration::seconds(90));
    scene.world.set(scene.world.project,orch_settings_contract::registry::keys::CALLS_RULES,
        json!([{"id":"reset-after-budget","when":{"kind":"rate_limited","purpose":"web_search"},"steps":[{"type":"wait","until":"reset"}]}])).await;
    let first = Arc::new(AtomicBool::new(true));
    let current = clock.clone();
    scene.world.machine.handler.on(CommandKind::ProviderService,move|_| {
        if first.swap(false,Ordering::SeqCst) {
            let mut error=CommandError::new(ErrorCode::Conflict,"synthetic native pre-request rate limit");
            error.details=Some(json!({"failure":{"code":"rate_limited","message":"Synthetic rate limit","retryable":true,"resets_at":reset}}));
            return Err(error);
        }
        let at=current.time();
        let mut observed=orch_agent_contract::requests::UsageRequest {id:"after-reset".into(),asked:Some(at),end:Some(at),output_streamed:Some(false),..Default::default()};
        observed.finish("done",None,None);
        Ok(CommandResult::ProviderService(orch_link_contract::commands::ProviderServiceResult {
            output:json!({"results":[{"title":"After reset","url":"https://source.example.invalid/reset"}]}),output_blob:None,
            timing:observed.measured(),counters:BTreeMap::from([("web_searches".into(),Some(1))]),
        }))
    });
    let paused = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "reset-rule",
        None,
        OrchTool::WebSearch,
        json!({"query":"native reset"}),
    )
    .await?;
    assert!(paused.error.is_none(), "{paused:?}");
    let result = paused.result.ok_or("missing paused reset")?;
    assert_eq!(result["state"], "paused");
    assert_eq!(result["reason"], "reset_after_deadline");
    assert_eq!(result["until"], serde_json::to_value(reset)?);
    assert_eq!(result["needs_person"], false);
    let again = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "before-reset",
        None,
        OrchTool::WebSearch,
        json!({"query":"still paused"}),
    )
    .await?;
    assert_eq!(again.result.ok_or("missing pause")?["state"], "paused");
    assert_eq!(
        scene.world.began(CommandKind::ProviderService),
        1,
        "the pre-request refusal is the only native request before reset"
    );
    let mut conn = scene.world.pool.acquire().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(
        count, 0,
        "no measurement was reported for the pre-request refusal"
    );
    drop(conn);
    clock.advance(chrono::Duration::seconds(91));
    let done = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "after-real-reset",
        None,
        OrchTool::WebSearch,
        json!({"query":"new actual request after reset"}),
    )
    .await?;
    assert!(done.error.is_none(), "{done:?}");
    assert_eq!(
        done.result.ok_or("missing actual result")?["results"][0]["title"],
        "After reset"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 2);
    let mut conn = scene.world.pool.acquire().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(count, 1);
    Ok(())
}

async fn extra_service_provider(scene: &Scene, name: &str, owner: Id) -> Proof<Id> {
    let provider = new_id();
    sqlx::query("INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,upstreams,protocols,machine_id,key_machine_id,arrangements,status,facts_requested_at,service_offerings)
        SELECT $1,$2,$3,org_id,home_id,kind,upstreams,protocols,machine_id,key_machine_id,arrangements,status,facts_requested_at,service_offerings
        FROM providers WHERE org_id=$4 AND name='Source search service'")
        .bind(provider).bind(name).bind(owner).bind(scene.world.root).execute(scene.world.pool.pool()).await?;
    let mut conn = scene.world.pool.acquire().await?;
    provider_owner(&mut conn, &scene.world, provider, owner).await?;
    common::put_secret(&mut conn, provider, "api_key", "synthetic-candidate-key").await;
    Ok(provider)
}
fn route_candidate(provider: Id) -> Json {
    json!({"provider_id":provider,"model":"","params":{},"route":{"route":"web_search.service-api","offering":{"provider":provider,"offering":OFFERING}}})
}
fn native_service_error(code: &str) -> CommandError {
    let mut error = CommandError::new(ErrorCode::Conflict, "synthetic pre-request service refusal");
    error.details = Some(
        json!({"failure":{"code":code,"message":"Synthetic service refusal","retryable":true}}),
    );
    error
}
fn successful_service(at: Time) -> CommandResult {
    let mut observed = orch_agent_contract::requests::UsageRequest {
        id: "recovered-search".into(),
        asked: Some(at),
        end: Some(at),
        output_streamed: Some(false),
        ..Default::default()
    };
    observed.finish("done", None, None);
    CommandResult::ProviderService(orch_link_contract::commands::ProviderServiceResult {
        output: json!({"results":[{"title":"Recovered source","url":"https://source.example.invalid/recovered"}]}),
        output_blob: None,
        timing: observed.measured(),
        counters: BTreeMap::from([("web_searches".into(), Some(1))]),
    })
}

#[tokio::test]
async fn a_managed_retry_uses_a_new_bound_attempt_and_one_measured_row_without_extending_the_deadline()
-> Proof {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let scene = configured().await?;
    scene.world.set(scene.world.project,orch_settings_contract::registry::keys::CALLS_RULES,
        json!([{"id":"retry-web","when":{"kind":"provider_busy","purpose":"web_search"},"steps":[{"type":"retry","times":1,"delay_secs":0,"backoff":1}]}])).await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let at = scene.world.clock.time();
    scene
        .world
        .machine
        .handler
        .on(CommandKind::ProviderService, move |_| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(native_service_error("provider_busy"))
            } else {
                Ok(successful_service(at))
            }
        });
    let done = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "bounded-retry",
        None,
        OrchTool::WebSearch,
        json!({"query":"one recovery"}),
    )
    .await?;
    assert!(done.error.is_none(), "{done:?}");
    assert_eq!(
        done.result.ok_or("missing recovered output")?["results"][0]["title"],
        "Recovered source"
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let mut conn = scene.world.pool.acquire().await?;
    let attempts:Vec<(Id,chrono::DateTime<chrono::Utc>)>=sqlx::query_as("SELECT c.id,c.deadline_at FROM commands c JOIN thread_tool_commands t ON t.command_id=c.id WHERE t.thread_id=$1 AND c.kind='provider.service' ORDER BY c.created_at,c.id")
        .bind(scene.thread).fetch_all(&mut *conn).await?;
    assert_eq!(attempts.len(), 2);
    assert_ne!(attempts[0].0, attempts[1].0);
    assert_eq!(attempts[0].1, attempts[1].1);
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(
        rows, 1,
        "only the second request reported a native measurement"
    );
    Ok(())
}

#[tokio::test]
async fn an_explicit_service_chain_keeps_its_rule_after_a_candidate_fails_differently() -> Proof {
    let scene = configured().await?;
    let first = extra_service_provider(&scene, "First explicit candidate", scene.world.ada).await?;
    let second =
        extra_service_provider(&scene, "Second explicit candidate", scene.world.ada).await?;
    scene.world.set(scene.world.project,orch_settings_contract::registry::keys::CALLS_RULES,
        json!([{"id":"service-chain","when":{"kind":"provider_busy","purpose":"web_search"},"steps":[{"type":"failover","candidate":route_candidate(first)},{"type":"failover","candidate":route_candidate(second)}]}])).await;
    let at = scene.world.clock.time();
    scene
        .world
        .machine
        .handler
        .on(CommandKind::ProviderService, move |command| {
            let Command::ProviderService(request) = command else {
                return Err(native_service_error("harness_error"));
            };
            if request.provider_id == second {
                Ok(successful_service(at))
            } else if request.provider_id == first {
                Err(native_service_error("provider_unreachable"))
            } else {
                Err(native_service_error("provider_busy"))
            }
        });
    let done = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "explicit-service-chain",
        None,
        OrchTool::WebSearch,
        json!({"query":"chain source"}),
    )
    .await?;
    assert!(done.error.is_none(), "{done:?}");
    assert_eq!(scene.world.began(CommandKind::ProviderService), 3);
    let mut conn = scene.world.pool.acquire().await?;
    let actual: (Id, String) = sqlx::query_as(
        "SELECT provider_id,model FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(actual, (second, String::new()));
    let rule:String=sqlx::query_scalar("SELECT rule->>'id' FROM model_call_recovery WHERE node_id=$1 AND purpose='web_search' AND state='completed'")
        .bind(scene.thread).fetch_one(&mut *conn).await?;
    assert_eq!(rule, "service-chain");
    Ok(())
}

#[tokio::test]
async fn a_forbidden_service_candidate_is_skipped_before_io_without_rewriting_the_root_choice()
-> Proof {
    let scene = configured().await?;
    let owner = scene.world.person("candidate-owner").await;
    let forbidden =
        extra_service_provider(&scene, "Unavailable to the original actor", owner).await?;
    let permitted =
        extra_service_provider(&scene, "Permitted explicit candidate", scene.world.ada).await?;
    scene.world.set(scene.world.project,orch_settings_contract::registry::keys::CALLS_RULES,
        json!([{"id":"skip-service","when":{"kind":"provider_busy","purpose":"web_search"},"steps":[{"type":"failover","candidate":route_candidate(forbidden)},{"type":"failover","candidate":route_candidate(permitted)}]}])).await;
    let at = scene.world.clock.time();
    scene
        .world
        .machine
        .handler
        .on(CommandKind::ProviderService, move |command| {
            let Command::ProviderService(request) = command else {
                return Err(native_service_error("harness_error"));
            };
            assert_ne!(
                request.provider_id, forbidden,
                "an unpermitted candidate must not reach the keeper"
            );
            if request.provider_id == permitted {
                Ok(successful_service(at))
            } else {
                Err(native_service_error("provider_busy"))
            }
        });
    let mut conn = scene.world.pool.acquire().await?;
    let before = crate::tools::capability_service::selection::setting(
        &mut conn,
        scene.thread,
        orch_settings_contract::registry::keys::AGENTS_WEB_SEARCH_BACKEND,
    )
    .await?;
    drop(conn);
    let done = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "skip-forbidden-candidate",
        None,
        OrchTool::WebSearch,
        json!({"query":"safe explicit chain"}),
    )
    .await?;
    assert!(done.error.is_none(), "{done:?}");
    assert_eq!(scene.world.began(CommandKind::ProviderService), 2);
    let mut conn = scene.world.pool.acquire().await?;
    assert_eq!(
        crate::tools::capability_service::selection::setting(
            &mut conn,
            scene.thread,
            orch_settings_contract::registry::keys::AGENTS_WEB_SEARCH_BACKEND
        )
        .await?,
        before
    );
    Ok(())
}

#[tokio::test]
async fn pending_native_holds_cannot_starve_a_later_completed_paid_receipt_out_of_bounded_sweeps()
-> Proof {
    let scene = configured().await?;
    let done = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "fair-reconciliation",
        None,
        OrchTool::WebSearch,
        json!({"query":"one real paid result"}),
    )
    .await?;
    assert!(done.error.is_none(), "{done:?}");
    let mut conn = scene.world.pool.acquire().await?;
    let completed:Id=sqlx::query_scalar("SELECT c.id FROM commands c JOIN thread_tool_commands e ON e.command_id=c.id WHERE e.thread_id=$1 AND c.kind='provider.service'")
        .bind(scene.thread).fetch_one(&mut *conn).await?;
    let offline = orch_db::queries::resources::testing::insert_machine(
        &mut conn,
        "Offline retained queue",
        "connected",
    )
    .await?;
    let waiting: Vec<Id> = (0..scene.world.engine().inner.config.sweep_batch)
        .map(|_| new_id())
        .collect();
    // These inert accepted receipts model older offline work, not a new source admission or IO.
    sqlx::query("WITH inserted AS (
        INSERT INTO commands(id,machine_id,kind,body,serial_key,state,deadline_at,created_at,updated_at)
        SELECT waiting.id,$2,'machine.probe','{}'::jsonb,c.serial_key,'accepted',c.deadline_at+interval '1 day',c.created_at-interval '1 hour',c.updated_at
        FROM unnest($1::uuid[]) waiting(id) CROSS JOIN commands c WHERE c.id=$3 RETURNING id
    ) INSERT INTO thread_tool_commands(command_id,thread_id,turn_id,segment_id,continuation_id,answer_id,environment_id,queue_provider_id)
        SELECT inserted.id,e.thread_id,e.turn_id,e.segment_id,e.continuation_id,e.answer_id,e.environment_id,e.queue_provider_id
        FROM inserted CROSS JOIN thread_tool_commands e WHERE e.command_id=$3")
        .bind(&waiting).bind(offline).bind(completed).execute(&mut *conn).await?;
    // Model completion after earlier queue checks, without altering the native result or ledger.
    sqlx::query("UPDATE thread_tool_commands SET settled=false,check_order=nextval('thread_tool_command_checks') WHERE command_id=$1")
        .bind(completed).execute(&mut *conn).await?;
    drop(conn);
    super::super::reap(&scene.world.engine(), None).await?;
    super::super::reap(&scene.world.engine(), None).await?;
    let mut conn = scene.world.pool.acquire().await?;
    let settled: bool =
        sqlx::query_scalar("SELECT settled FROM thread_tool_commands WHERE command_id=$1")
            .bind(completed)
            .fetch_one(&mut *conn)
            .await?;
    assert!(
        settled,
        "the real completed receipt advances within two bounded cuts despite the older pending prefix"
    );
    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_tool_commands WHERE command_id=ANY($1) AND NOT settled",
    )
    .bind(&waiting)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(
        kept,
        i64::try_from(waiting.len())?,
        "an unfinished native hold still needs its original completion proof"
    );
    let counted: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE id=$1")
        .bind(completed)
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(counted, 1);
    assert_eq!(
        scene.world.began(CommandKind::ProviderService),
        1,
        "sweeping never repeats paid work"
    );
    Ok(())
}

#[tokio::test]
async fn prepared_input_and_an_accepted_receipt_survive_materializer_loss_without_generating_again()
-> Proof {
    let scene = configured().await?;
    let installed = scene
        .world
        .engine()
        .inner
        .capability_services
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or("missing installed source")?;
    let mut declarations = installed
        .capabilities()
        .extension("fixture-search")
        .ok_or("missing fixture extension")?
        .clone();
    let interface = declarations
        .interfaces
        .get_mut("fixture-search-api")
        .ok_or("missing fixture interface")?;
    interface.implementation = "image-api".into();
    interface.shape.input_schema["required"] = json!(["query", "model"]);
    interface.shape.input_schema["properties"]["model"] = json!({"const":"fixture-image-model"});
    interface.shape.request["body"]["model"] = json!({"from_input":"/model"});
    let mut sources = orch_provider_contract::builtins::BuiltinSources::default();
    sources.register_capabilities("fixture-search", declarations)?;
    scene
        .world
        .engine()
        .attach_capability_services(Arc::new(sources));
    PREPARED_SERVICE_INPUTS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| "prepared fixture poisoned")?
        .insert(
            scene.thread,
            json!({"query":"provider prompt only","model":"fixture-image-model"}),
        );
    let at = scene.world.clock.time();
    scene
        .world
        .machine
        .handler
        .on(CommandKind::ProviderService, move |command| {
            let Command::ProviderService(request) = command else {
                return Err(native_service_error("harness_error"));
            };
            assert_eq!(
                request.input,
                json!({"query":"provider prompt only","model":"fixture-image-model"})
            );
            assert!(
                !serde_json::to_string(request)
                    .unwrap_or_default()
                    .contains("/private/destination.png")
            );
            Ok(successful_service(at))
        });
    let done = Scene::ask(
        &scene.world,
        &scene.source,
        scene.start.turn_id,
        "prepared-accepted-result",
        None,
        OrchTool::WebSearch,
        json!({"query":"caller request keeps /private/destination.png"}),
    )
    .await?;
    assert!(done.error.is_none(), "{done:?}");
    assert_eq!(
        done.result.ok_or("missing actual recovered data")?["results"][0]["title"],
        "Recovered source"
    );
    assert_eq!(scene.world.began(CommandKind::ProviderService), 1);
    let mut conn = scene.world.pool.acquire().await?;
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND purpose='web_search'",
    )
    .bind(scene.thread)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(rows, 1);
    let retained:i64=sqlx::query_scalar("SELECT count(*) FROM thread_tool_service_results r JOIN thread_tool_calls t ON t.answer_id=r.answer_id WHERE t.thread_id=$1").bind(scene.thread).fetch_one(&mut *conn).await?;
    assert_eq!(
        retained, 0,
        "explicit delivery acknowledgement releases the temporary packet"
    );
    Ok(())
}
