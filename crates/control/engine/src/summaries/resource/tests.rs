//! Actual accepted resource acts retain their exact backend when their native source is reopened.
use super::*;
use crate::test_common as common;
use orch_api_contract::summaries::SummaryRefreshBody;
use orch_link_contract::commands::CommandKind;
use serde_json::json;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_accepted_summary_reopens_with_its_exact_backend_without_model_io() -> Proof {
    let world = common::World::on_a_key("synthetic-resource-reentry-key").await;
    world.engine().stop();
    let backend = json!({
        "route":orch_summarizer_model::capabilities::ROUTE,
        "offering":{"provider":world.provider,"offering":common::MODEL},
        "config":null,
        "x_revision":{"identity":"kept"}
    });
    world
        .set(
            world.topic,
            orch_settings_contract::registry::keys::AUTO_SUMMARIZER,
            json!({"kind":"model","backend":backend}),
        )
        .await;
    let document = world
        .document_beside(
            world.topic,
            "Recorded source",
            &["The accepted material stays current."],
        )
        .await;
    let target = SummaryTarget::Document { id: document };
    let at = world.clock.time();
    let mut tx = world.pool.begin().await?;
    let caller = Caller::session(world.ada).with_admin_mode(false);
    let selected = select(&mut tx, &caller, target, None, None, at).await?;
    let plan = operation_scope::sources::bind_required(
        SummaryRefresh::ROUTE,
        &summary_sources::RESOURCE_REFRESH,
        &selected.facts(),
    )?;
    let principal = Principal::load(&mut tx, plan.admit(&caller, false)?, at).await?;
    require_model(
        &mut tx,
        &principal,
        selected.material.identity.place(),
        &selected.config,
    )
    .await?;
    let origin = serde_json::to_value(CapturedOperation::capture(principal.caller())?)?;
    let cache = rows::ensure(&mut tx, target, Some(world.ada), at.get())
        .await?
        .ok_or("no resource cache")?;
    let client = orch_types::new_id();
    let arguments = serde_json::to_value(SummaryRefreshBody {
        client_id: client,
        model: None,
        range: None,
    })?;
    let source = json!({"target":target,"range":selected.material.range,"snapshot":selected.material.snapshot,"policy":selected.policy.fingerprint,"selection":selected.backend_selection()?});
    let rows::Enqueued::New(accepted) = rows::enqueue(
        &mut tx,
        cache.id,
        rows::Origin::Person {
            issuer: world.ada,
            client_id: client,
            accepted_operation: &origin,
        },
        &arguments,
        &source,
        at.get(),
        at.get(),
    )
    .await?
    else {
        return Err("the original act was not accepted once".into());
    };
    let until = at.get()
        + chrono::Duration::seconds(i64::from(CommandKind::ProviderComplete.deadline_secs()));
    let request = rows::claim(&mut tx, at.get(), until)
        .await?
        .ok_or("no accepted claim")?;
    assert_eq!(request.id, accepted.id);
    let work = Work::capture(&request, &principal, &selected)?;
    assert!(
        rows::prepare(
            &mut tx,
            request.id,
            request.lease_token.ok_or("no claim token")?,
            at.get(),
            &serde_json::to_value(&work)?
        )
        .await?
    );
    let fresh = work.frame(&mut tx, world.clock.as_ref()).await?;
    assert_eq!(
        serde_json::to_value(&fresh.value().config.backend)?,
        backend
    );
    assert_eq!(world.began(CommandKind::ProviderComplete), 0);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_title_uses_its_chosen_subscription_in_one_private_tool_free_helper() -> Proof {
    use orch_authz::Caller;
    use orch_db::queries::access::{self, PageKey};
    use orch_link_contract::commands::{
        AgentStartResult, Command, CommandError, CommandResult, ErrorCode, SessionOpenResult,
    };
    use std::sync::{Arc, Mutex, PoisonError};
    let world = common::World::new().await;
    let thread = world.thread("Native summary source").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            common::queued("The implementation is ready for review."),
        )
        .await?;
    world
        .run_turn(thread, 0, "Tests passed and the review is next.")
        .await;
    let mut descriptor = common::descriptor();
    descriptor["side_thread_modes"] = json!(["no_tools"]);
    descriptor["x_capability_program"] = json!({"title":"Fixture text program","access":{"native":{"orch_tools":true,"native":{
        "generation":{"enabled":true,"modes":["native"],"default_mode":"native"},
        "summary":{"enabled":true,"modes":["native"],"default_mode":"native","delegated_mode":"native",
            "execution":[{"input":"summary.request","result":"summary.result","confined_tools":true}]}
    }}}});
    world.declares(descriptor.clone()).await;
    let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor)?;
    world.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: true,
            harness_version: "1.0.0".into(),
            descriptor: descriptor.clone(),
        }))
    });
    let opens = Arc::new(Mutex::new(Vec::new()));
    for kind in [CommandKind::SessionOpen, CommandKind::SessionOpenFenced] {
        let opens = opens.clone();
        world.machine.handler.on(kind, move |command| {
            let (open, fenced) = match command {
                Command::SessionOpen(open) => (open.clone(), false),
                Command::SessionOpenFenced(open) => (open.session.as_ref().clone(), true),
                _ => return Err(CommandError::new(ErrorCode::Conflict, "not a session open")),
            };
            let result = SessionOpenResult {
                native_session_ref: format!("native-{}", open.segment_id),
                resumed: false,
                import_report: None,
                instruction_sources: vec![],
                approval_reviewer: None,
            };
            opens
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(open);
            Ok(if fenced {
                CommandResult::SessionOpenFenced(result)
            } else {
                CommandResult::SessionOpen(result)
            })
        });
    }
    let provider = orch_types::new_id();
    sqlx::query("INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,harness_kind,agent_id,machine_id,arrangements,status,facts_requested_at) VALUES($1,'Native text subscription',$2,$3,$3,'subscription','fake',$4,$5,ARRAY['separated'],'ready',$6)")
        .bind(provider).bind(world.ada).bind(world.root).bind(world.agent).bind(world.machine_id).bind(world.clock.now()).execute(world.pool.pool()).await?;
    let mut conn = world.pool.acquire().await?;
    access::testing::add_entry(
        &mut conn,
        &PageKey::new("provider", provider, "live"),
        "any",
        "access.own",
        ("user", Some(world.ada), None),
        None,
        world.clock.now(),
    )
    .await?;
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,groups,enabled,source,created_by) VALUES($1,$2,$3,'Native text model',$4,ARRAY['text'],true,'manual',$5)")
        .bind(orch_types::new_id()).bind(provider).bind(common::MODEL).bind(json!({"context_window":10000})).bind(world.ada).execute(&mut *conn).await?;
    drop(conn);
    world.set(thread, orch_settings_contract::registry::keys::AUTO_SUMMARIZER,
        json!({"kind":"model","backend":{"route":"summary.delegation","offering":{"provider":provider,"offering":common::MODEL}}})).await;
    world
        .set(
            thread,
            orch_settings_contract::registry::keys::AUTO_TITLE,
            json!("summarizer"),
        )
        .await;
    world
        .set(
            thread,
            orch_settings_contract::registry::keys::AUTO_TRIGGERS,
            json!({"min_interval_secs":0}),
        )
        .await;
    sqlx::query("UPDATE settings SET updated_by=$2 WHERE node_id=$1 AND key='auto.summarizer'")
        .bind(thread)
        .bind(world.ada)
        .execute(world.pool.pool())
        .await?;
    sqlx::query("UPDATE nodes SET title_source='auto' WHERE id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await?;
    let mut conn = world.pool.acquire().await?;
    crate::summaries::due(&mut conn, thread, "refresh", world.clock.now()).await?;
    drop(conn);
    let engine = world.engine();
    let running = tokio::spawn(async move { engine.summarize_due(thread).await });
    let helper = tokio::time::timeout(common::PATIENCE, async {
        loop {
            let helper:Option<Id> = sqlx::query_scalar("SELECT helper_id FROM delegated_program_work WHERE request_node_id=$1 AND purpose='summary' ORDER BY created_at,id LIMIT 1")
                .bind(thread).fetch_optional(world.pool.pool()).await?;
            if let Some(helper) = helper { break Ok::<_,sqlx::Error>(helper); }
            tokio::task::yield_now().await;
        }
    }).await??;
    let start = world.turn_start(helper, 0).await;
    let open = opens
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|open| open.thread_id == helper)
        .cloned()
        .ok_or("no native helper open")?;
    assert_eq!(
        open.side_reading,
        Some(orch_link_contract::commands::SideReading::NoTools)
    );
    assert!(open.orch_tools.is_empty() && open.mcp.is_empty());
    let output = start
        .native_model
        .as_ref()
        .ok_or("no native constraints")?
        .program_output()
        .map_err(EngineError::unreadable)?
        .ok_or("no output allowance")?;
    assert_eq!(output.max_output_tokens, 120);
    world
        .answer(helper, &start, "native-title", "Ready for review")
        .await;
    world
        .say(
            helper,
            common::turn_ended(
                start.turn_id,
                orch_link_contract::foreign::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let _ = running.await??;
    common::eventually("the native title is published", || async {
        sqlx::query_scalar::<_, String>("SELECT name FROM nodes WHERE id=$1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .is_ok_and(|name| name == "Ready for review")
    })
    .await;
    let name: String = sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1")
        .bind(thread)
        .fetch_one(world.pool.pool())
        .await?;
    assert_eq!(name, "Ready for review");
    let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
        .bind(helper)
        .fetch_one(world.pool.pool())
        .await?;
    assert_eq!(owner, world.ada);
    assert_eq!(
        world.began(CommandKind::ProviderComplete),
        0,
        "a subscription never falls through to an API invoice"
    );
    assert_eq!(world.turn_starts(helper).len(), 1);
    common::eventually("the completed native source is settled", || async {
        let mut conn = match world.pool.acquire().await {
            Ok(conn) => conn,
            Err(_) => return false,
        };
        if super::super::completion::recover(&world.engine(), &mut conn, 4)
            .await
            .is_err()
        {
            return false;
        }
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM summary_calls WHERE node_id=$1 AND program_backend IS NOT NULL",
        )
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .is_ok_and(|count| count == 0)
    })
    .await;
    let mut conn = world.pool.acquire().await?;
    let input: Id = sqlx::query_scalar(
        "SELECT initial_input_id FROM delegated_program_work WHERE helper_id=$1",
    )
    .bind(helper)
    .fetch_one(&mut *conn)
    .await?;
    let scope = orch_db::queries::capability_work::for_input(&mut conn, input)
        .await?
        .ok_or("no native summary usage scope")?;
    assert_eq!(
        (scope.node, scope.purpose.as_str(), scope.person),
        (thread, "summary", None)
    );
    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE thread_id=$1 AND kind='agent_message'",
    )
    .bind(helper)
    .fetch_one(&mut *conn)
    .await?;
    assert!(
        kept > 0,
        "settling work keeps its ordinary private helper history"
    );
    Ok(())
}
