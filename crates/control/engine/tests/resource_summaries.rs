//! Real native summary material, command IO, publication and normalized ledger; no external model.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]
mod common;
mod shared;
use common::{MODEL, World, eventually};
use orch_api_contract::{
    routes::Endpoint,
    summaries::{SummaryRefresh, SummaryRefreshBody, SummaryTarget},
    summary_sources,
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::queries::summaries as rows;
use orch_engine::{operation_scope, summaries::resource};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, ProviderCompleteResult,
};
use orch_types::{Id, Time, new_id};
use serde_json::json;
use shared::Places;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

async fn accept(world: &World, target: SummaryTarget) -> Id {
    accept_as(world, target, world.ada).await
}

async fn accept_as(world: &World, target: SummaryTarget, issuer: Id) -> Id {
    let mut tx = world.pool.begin().await.unwrap();
    let caller = Caller::session(issuer).with_admin_mode(false);
    let selected = resource::select(
        &mut tx,
        &caller,
        target,
        None,
        None,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let plan = operation_scope::sources::bind_required(
        SummaryRefresh::ROUTE,
        &summary_sources::RESOURCE_REFRESH,
        &selected.facts(),
    )
    .unwrap();
    let principal = Principal::load(
        &mut tx,
        plan.admit(&caller, false).unwrap(),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    resource::require_model(
        &mut tx,
        &principal,
        selected.material.identity.place(),
        &selected.config,
    )
    .await
    .unwrap();
    let origin =
        serde_json::to_value(CapturedOperation::capture(principal.caller()).unwrap()).unwrap();
    let cache = rows::ensure(&mut tx, target, Some(issuer), world.clock.now())
        .await
        .unwrap()
        .unwrap();
    let arguments = serde_json::to_value(SummaryRefreshBody {
        client_id: new_id(),
        model: None,
        range: None,
    })
    .unwrap();
    let source = json!({"target":target,"range":selected.pinned_range(),"snapshot":selected.material.snapshot,"policy":selected.policy.fingerprint});
    let accepted = rows::enqueue(
        &mut tx,
        cache.id,
        rows::Origin::Person {
            issuer,
            client_id: serde_json::from_value(arguments["client_id"].clone()).unwrap(),
            accepted_operation: &origin,
        },
        &arguments,
        &source,
        world.clock.now(),
        world.clock.now(),
    )
    .await
    .unwrap();
    let rows::Enqueued::New(request) = accepted else {
        panic!("one new original act")
    };
    tx.commit().await.unwrap();
    request.id
}
async fn state(world: &World, id: Id) -> String {
    rows::request(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap()
        .state
}
async fn problem_of(world: &World, id: Id) -> serde_json::Value {
    rows::request(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap()
        .problem
        .unwrap_or_default()
}
fn answer(text: &str) -> Result<CommandResult, CommandError> {
    Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
        text: text.into(),
        usage: orch_agent_contract::events::TokenUsage::from_parts(
            orch_api_contract::threads::TokensReportedApart {
                input: 1000,
                output: 0,
                ..Default::default()
            },
        ),
        request_ref: Some("synthetic-resource-summary".into()),
        timing: Default::default(),
    }))
}
async fn chosen(world: &World) {
    world
        .set(
            world.topic,
            "auto.summarizer",
            json!({"kind":"model","provider_id":world.provider,"model_name":MODEL}),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_document_refresh_uses_the_issuer_not_the_setting_writer_and_stops_at_their_weekly_cap()
 {
    let world = World::on_a_key("synthetic-resource-summary-key").await;
    world.engine().stop();
    chosen(&world).await;
    let setter = world.person("different-summary-setter").await;
    sqlx::query("UPDATE settings SET updated_by=$2 WHERE node_id=$1 AND key='auto.summarizer'")
        .bind(world.topic)
        .bind(setter)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$1::jsonb) WHERE provider_id=$2 AND model_name=$3")
        .bind(json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"2"})).bind(world.provider).bind(MODEL).execute(world.pool.pool()).await.unwrap();
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,warn_percent,created_by) VALUES($1,$2,'members',$3,'each','credits','week',0.001,ARRAY['summary'],50,$4)")
        .bind(new_id()).bind(world.project).bind(world.root).bind(world.ada).execute(world.pool.pool()).await.unwrap();
    let document = world
        .document_beside(
            world.topic,
            "Current plan",
            &["The implementation is complete; review is next."],
        )
        .await;
    let target = SummaryTarget::Document { id: document };
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            let Command::ProviderComplete(request) = command else {
                return Err(CommandError::new(ErrorCode::Internal, "not a completion"));
            };
            assert!(request.input.contains("review is next"));
            observed.fetch_add(1, Ordering::SeqCst);
            answer("The implementation is complete. Review is next.")
        });
    let first = accept(&world, target).await;
    world.engine().summarize_resources().await.unwrap();
    eventually("the original summary publishes", || async {
        state(&world, first).await == "done"
    })
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let cache = rows::get(
        &mut world.pool.acquire().await.unwrap(),
        target,
        Some(world.ada),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(cache.text.unwrap().contains("Review is next"));
    let ledger:(Option<Id>,String,i64)=sqlx::query_as("SELECT user_id,purpose,count(*) OVER() FROM usage_events WHERE request_ref='synthetic-resource-summary'").fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(ledger, (Some(world.ada), "summary".into(), 1));
    let mut conn = world.pool.acquire().await.unwrap();
    let authority = orch_db::queries::link::command_authority(&mut conn, first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        authority["kind"], "capability_service",
        "the native refresh uses the common paid command guard"
    );
    assert_eq!(
        authority.pointer("/authority/origin/attribution/person"),
        Some(&json!(world.ada))
    );
    drop(conn);
    for _ in 0..2 {
        world
            .engine()
            .reconcile_paid_service_command(first)
            .await
            .unwrap();
    }
    let once: i64 =
        sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE id=$1 AND purpose='summary'")
            .bind(first)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        once, 1,
        "the common command-id completion path remains idempotent without a consumer ledger write"
    );
    let marker: (Option<Id>, serde_json::Value) = sqlx::query_as(
        "SELECT viewer_id,source_refs FROM resource_summary_model_inputs WHERE request_id=$1",
    )
    .bind(first)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(marker, (Some(world.ada), json!([target])));
    let second = accept(&world, target).await;
    world.engine().summarize_resources().await.unwrap();
    eventually(
        "the issuer's exhausted cap refuses another request",
        || async { state(&world, second).await == "failed" },
    )
    .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the second act never reaches model IO"
    );
    let problem = problem_of(&world, second).await;
    assert_eq!(
        (problem["reason"].as_str(), problem["message"].as_str()),
        (
            Some("cap_no_room"),
            Some(resource::Reason::CapNoRoom.sentence())
        ),
        "the person is told that a usage limit is what stopped it"
    );
    assert_eq!(
        rows::get(
            &mut world.pool.acquire().await.unwrap(),
            target,
            Some(world.ada)
        )
        .await
        .unwrap()
        .unwrap()
        .text
        .as_deref(),
        Some("The implementation is complete. Review is next.")
    );
    let automatic = orch_engine::caps::OwnCapAsk {
        place: world.topic,
        provider: world.provider,
        model: MODEL,
        tier: None,
        purpose: "summary",
        person: None,
    };
    assert!(
        world
            .engine()
            .room_for_own_use(&mut world.pool.acquire().await.unwrap(), &automatic)
            .await
            .unwrap(),
        "the person's cap does not become the automatic writer's cap"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_refused_by_a_new_cap_reaches_neither_the_paid_command_nor_the_ledger() {
    let world = World::on_a_key("synthetic-capped-summary-key").await;
    world.engine().stop();
    chosen(&world).await;
    let doc = world
        .document_beside(world.topic, "Capped source", &["This text is readable."])
        .await;
    let request = accept(&world, SummaryTarget::Document { id: doc }).await;
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,warn_percent,created_by) VALUES($1,$2,'members',$3,'each','credits','week',0,ARRAY['summary'],50,$4)")
        .bind(new_id()).bind(world.project).bind(world.root).bind(world.ada).execute(world.pool.pool()).await.unwrap();
    let called = Arc::new(AtomicUsize::new(0));
    let seen = called.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            answer("Must not be requested")
        });
    world.engine().summarize_resources().await.unwrap();
    eventually(
        "the new cap closes the accepted refresh before paid IO",
        || async { !matches!(state(&world, request).await.as_str(), "queued" | "running") },
    )
    .await;
    assert_eq!(called.load(Ordering::SeqCst), 0);
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commands WHERE id=$1),(SELECT count(*) FROM usage_events WHERE id=$1)")
        .bind(request).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(counts, (0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_the_actual_issuers_provider_use_after_acceptance_records_no_paid_command_or_usage()
 {
    use orch_db::queries::access::{self, NewEntry, PageKey};
    let world = World::on_a_key("synthetic-revoked-summary-key").await;
    world.engine().stop();
    chosen(&world).await;
    let issuer = world.person("shared-summary-issuer").await;
    let doc = world
        .document_beside(
            world.topic,
            "Readable shared source",
            &["This request needs the actual issuer's current Use."],
        )
        .await;
    let grant = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    for (id, page, permission) in [
        (
            grant,
            PageKey::new("provider", world.provider, "live"),
            "provider.use",
        ),
        (new_id(), PageKey::new("document", doc, "live"), "doc.view"),
    ] {
        access::insert_entry(
            &mut conn,
            &NewEntry {
                id,
                page: &page,
                profile: "any",
                permission,
                holder_kind: "user",
                holder_id: Some(issuer),
                holder_role: None,
                options: &json!({}),
                expires_at: None,
                note: "summary deciding case",
                source: "page",
                created_by: Some(world.ada),
                at: world.clock.now(),
            },
        )
        .await
        .unwrap();
    }
    orch_authz::epoch::bump(&mut conn).await.unwrap();
    drop(conn);
    let request = accept_as(&world, SummaryTarget::Document { id: doc }, issuer).await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        access::remove_entry(&mut conn, grant, world.ada, world.clock.now())
            .await
            .unwrap()
    );
    orch_authz::epoch::bump(&mut conn).await.unwrap();
    drop(conn);
    let called = Arc::new(AtomicUsize::new(0));
    let seen = called.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            answer("Must not be requested")
        });
    world.engine().summarize_resources().await.unwrap();
    eventually("current native Use closes the original refresh", || async {
        !matches!(state(&world, request).await.as_str(), "queued" | "running")
    })
    .await;
    assert_eq!(called.load(Ordering::SeqCst), 0);
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commands WHERE id=$1),(SELECT count(*) FROM usage_events WHERE id=$1)")
        .bind(request).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(counts, (0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_changed_during_model_io_discards_only_the_answer_and_keeps_its_actual_usage() {
    let world = World::on_a_key("synthetic-resource-source-key").await;
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(
            world.topic,
            "Changing plan",
            &["Original material before the request."],
        )
        .await;
    let target = SummaryTarget::Document { id: document };
    let entered = Arc::new(AtomicUsize::new(0));
    let observed = entered.clone();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            let (lock, wake) = &*held;
            let release = lock.lock().unwrap();
            let (release, _) = wake
                .wait_timeout_while(release, Duration::from_secs(10), |release| !*release)
                .unwrap();
            assert!(*release, "the fixture releases its native request");
            answer("An answer about the original material.")
        });
    let request = accept(&world, target).await;
    world.engine().summarize_resources().await.unwrap();
    eventually("the actual native model request starts", || async {
        entered.load(Ordering::SeqCst) == 1
    })
    .await;
    // A real document writer must complete while the model is still waiting: no source,
    // policy, cache, request or transaction lock may span the external call.
    let changed = tokio::time::timeout(
        Duration::from_secs(5),
        world.change_block(
            document,
            "Original material",
            "Changed material after the request.",
        ),
    )
    .await;
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    changed.expect("native content editing does not wait for model IO");
    eventually("the old answer is discarded", || async {
        state(&world, request).await == "discarded"
    })
    .await;
    assert_eq!(
        problem_of(&world, request).await["reason"],
        "source_changed",
        "the person is told why there is no current paragraph and can summarize again"
    );
    assert_eq!(
        rows::get(
            &mut world.pool.acquire().await.unwrap(),
            target,
            Some(world.ada)
        )
        .await
        .unwrap()
        .unwrap()
        .text,
        None
    );
    let ledger: (Option<Id>, String) = sqlx::query_as(
        "SELECT user_id,purpose FROM usage_events WHERE request_ref='synthetic-resource-summary'",
    )
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(ledger, (Some(world.ada), "summary".into()));
    world.engine().summarize_resources().await.unwrap();
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "a source change is no implicit paid retry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_changed_before_the_first_model_call_is_read_again_and_only_its_new_prompt_is_sent()
 {
    use orch_engine::summaries::resource::command::observation;
    let world = Arc::new(World::on_a_key("synthetic-unpaid-rebuild-key").await);
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(
            world.topic,
            "A new source read",
            &["Original material before the model."],
        )
        .await;
    let calls = recording(&world, "The new source is summarized.");
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    let moved = Arc::new(AtomicUsize::new(0));
    let (observed, source) = (moved.clone(), world.clone());
    observation::follow(
        move |id| {
            let (source, observed) = (source.clone(), observed.clone());
            Box::pin(async move {
                let prepared = rows::request(&mut source.pool.acquire().await.unwrap(), id)
                    .await
                    .unwrap()
                    .unwrap()
                    .prepared
                    .is_some();
                if prepared && observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    source
                        .change_block(
                            document,
                            "Original material",
                            "Fresh material before the first model call.",
                        )
                        .await;
                }
            })
        },
        world.engine().summarize_resources(),
    )
    .await
    .unwrap();
    eventually("the unpaid rebuild publishes one new answer", || async {
        state(&world, request).await == "done"
    })
    .await;
    let asked = calls.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0]
            .input
            .contains("Fresh material before the first model call")
    );
    assert!(
        !asked[0]
            .input
            .contains("Original material before the model")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_that_keeps_changing_before_any_model_call_says_so_without_an_access_refusal() {
    use orch_engine::summaries::resource::command::observation;
    let world = Arc::new(World::on_a_key("synthetic-churning-source-key").await);
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(
            world.topic,
            "A moving source",
            &["Churning material revision zero."],
        )
        .await;
    let calls = recording(&world, "No paid call is needed.");
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    let moved = Arc::new(AtomicUsize::new(0));
    let (source, observed) = (world.clone(), moved.clone());
    observation::follow(
        move |id| {
            let (source, observed) = (source.clone(), observed.clone());
            Box::pin(async move {
                if rows::request(&mut source.pool.acquire().await.unwrap(), id)
                    .await
                    .unwrap()
                    .unwrap()
                    .prepared
                    .is_some()
                {
                    let at = observed.fetch_add(1, Ordering::SeqCst);
                    if at < 4 {
                        source
                            .change_block(
                                document,
                                "Churning material",
                                &format!("Churning material revision {at}."),
                            )
                            .await;
                    }
                }
            })
        },
        world.engine().summarize_resources(),
    )
    .await
    .unwrap();
    eventually(
        "the changing unpaid read ends with its factual cause",
        || async {
            matches!(
                state(&world, request).await.as_str(),
                "failed" | "discarded" | "done"
            )
        },
    )
    .await;
    assert_eq!(
        problem_of(&world, request).await["reason"],
        "content_changing"
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unpaid_prompt_is_rebuilt_even_after_its_original_model_input_was_journaled() {
    use orch_engine::summaries::resource::command::observation;
    let world = Arc::new(World::on_a_key("synthetic-late-unpaid-rebuild-key").await);
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(world.topic, "Late source change", &["Original material."])
        .await;
    let calls = recording(&world, "The replacement input is summarized.");
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    let moved = Arc::new(AtomicUsize::new(0));
    let (source, observed) = (world.clone(), moved.clone());
    observation::follow(move |id| {
        let (source, observed) = (source.clone(), observed.clone());
        Box::pin(async move {
            let journaled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_summary_model_inputs WHERE request_id=$1)")
                .bind(id).fetch_one(source.pool.pool()).await.unwrap();
            if journaled && observed.fetch_add(1, Ordering::SeqCst) == 0 {
                source.change_block(document, "Original material", "Fresh material after the first input was journaled.").await;
            }
        })
    }, world.engine().summarize_resources()).await.unwrap();
    eventually(
        "the journaled unpaid prompt is replaced by one actual call",
        || async { state(&world, request).await == "done" },
    )
    .await;
    let asked = calls.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0]
            .input
            .contains("Fresh material after the first input was journaled")
    );
    let retained = rows::request(&mut world.pool.acquire().await.unwrap(), request)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(retained.call_id, request);
    assert_eq!(retained.read_attempt, 1);
    assert!(retained.model_admitted_at.is_some());
    let inputs: Vec<(String, String)> = sqlx::query_as("SELECT m.reference,m.state FROM model_inputs m JOIN resource_summary_model_inputs g ON g.model_input_id=m.id WHERE g.request_id=$1 ORDER BY m.reference")
        .bind(request).fetch_all(world.pool.pool()).await.unwrap();
    assert!(inputs.contains(&(request.to_string(), "failed".to_owned())));
    assert!(inputs.contains(&(retained.call_id.to_string(), "sent".to_owned())));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summarys_unpaid_read_allowance_can_be_zero_or_greater_than_the_shipped_default() {
    use orch_engine::summaries::resource::command::observation;
    for limit in [0, 5] {
        let world = Arc::new(World::on_a_key("synthetic-configurable-summary-read-key").await);
        world.engine().stop();
        chosen(&world).await;
        world
            .set(world.topic, "summary.read_repeats", json!(limit))
            .await;
        let document = world
            .document_beside(
                world.topic,
                "Configurable source read",
                &["Moving material revision zero."],
            )
            .await;
        let calls = recording(&world, "The settled source is summarized.");
        let request = accept(&world, SummaryTarget::Document { id: document }).await;
        let moved = Arc::new(AtomicUsize::new(0));
        let (source, observed) = (world.clone(), moved.clone());
        observation::follow(
            move |id| {
                let (source, observed) = (source.clone(), observed.clone());
                Box::pin(async move {
                    if rows::request(&mut source.pool.acquire().await.unwrap(), id)
                        .await
                        .unwrap()
                        .unwrap()
                        .prepared
                        .is_some()
                    {
                        let at = observed.fetch_add(1, Ordering::SeqCst);
                        if at < 4 {
                            source
                                .change_block(
                                    document,
                                    "Moving material",
                                    &format!("Moving material revision {at}."),
                                )
                                .await;
                        }
                    }
                })
            },
            world.engine().summarize_resources(),
        )
        .await
        .unwrap();
        eventually("the configured unpaid read settles", || async {
            matches!(
                state(&world, request).await.as_str(),
                "done" | "failed" | "discarded"
            )
        })
        .await;
        if limit == 0 {
            assert_eq!(
                problem_of(&world, request).await["reason"],
                "content_changing"
            );
            assert!(calls.lock().unwrap().is_empty());
        } else {
            assert_eq!(state(&world, request).await, "done");
            let asked = calls.lock().unwrap().clone();
            assert_eq!(asked.len(), 1);
            assert!(asked[0].input.contains("Moving material revision 3"));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_document_events_are_paid_by_the_setter_unattributed_and_keep_the_original_pacing()
 {
    let world = World::on_a_key("synthetic-auto-paragraph-key").await;
    world.engine().stop();
    chosen(&world).await;
    world
        .set(
            world.topic,
            "summary.targets",
            json!({"document":{"automatic":true,"min_interval_secs":120}}),
        )
        .await;
    let document = world
        .document_beside(
            world.topic,
            "Automatic paragraph",
            &["The first current source."],
        )
        .await;
    let target = SummaryTarget::Document { id: document };
    orch_db::queries::documents::describe(
        &mut world.pool.acquire().await.unwrap(),
        document,
        "A deliberately written description",
        &orch_documents::versions::Writer::User(world.ada),
        world.clock.now(),
    )
    .await
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            let Command::ProviderComplete(request) = command else {
                return Err(CommandError::new(ErrorCode::Internal, "not a completion"));
            };
            assert!(!request.input.contains("deliberately written description"));
            observed.fetch_add(1, Ordering::SeqCst);
            answer("The current source has been summarized.")
        });
    world.engine().summarize_resources().await.unwrap();
    eventually("the automatic source event publishes", || async {
        rows::get(&mut world.pool.acquire().await.unwrap(), target, None)
            .await
            .unwrap()
            .is_some_and(|row| row.text.is_some())
    })
    .await;
    let cache = rows::get(&mut world.pool.acquire().await.unwrap(), target, None)
        .await
        .unwrap()
        .unwrap();
    let first = rows::latest(&mut world.pool.acquire().await.unwrap(), cache.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.origin, "automatic");
    assert_eq!(first.paying_user, Some(world.ada));
    assert_eq!(first.requested_by, None);
    assert_eq!(first.accepted_operation, None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let issuer: Option<Id> = sqlx::query_scalar(
        "SELECT user_id FROM usage_events WHERE request_ref='synthetic-resource-summary'",
    )
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(issuer, None);
    world
        .change_block(document, "first current source", "A second current source.")
        .await;
    world.engine().summarize_resources().await.unwrap();
    let queued = rows::latest(&mut world.pool.acquire().await.unwrap(), cache.id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(queued.id, first.id);
    assert_eq!(queued.state, "queued");
    assert_eq!(
        queued.due_at,
        first.created_at + chrono::Duration::seconds(120)
    );
    let captured = queued.accepted_source.clone();
    world
        .change_block(
            document,
            "second current source",
            "A third current source while refresh is paced.",
        )
        .await;
    world.engine().summarize_resources().await.unwrap();
    let retained = rows::latest(&mut world.pool.acquire().await.unwrap(), cache.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.id, queued.id);
    assert_eq!(retained.accepted_source, captured);
    assert_eq!(retained.due_at, queued.due_at);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "pending events do not bypass the configured gap"
    );
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM resource_summary_events WHERE kind='document' AND target_id=$1",
    )
    .bind(document)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert!(
        events > 0,
        "the newer source event is retained for after the original claim settles"
    );
    assert_eq!(
        orch_db::queries::documents::get(&mut world.pool.acquire().await.unwrap(), document)
            .await
            .unwrap()
            .unwrap()
            .description,
        "A deliberately written description"
    );
}

async fn group(world: &World, name: Option<&str>) -> Id {
    use orch_db::queries::chats;
    let id = new_id();
    let mut tx = world.pool.begin().await.unwrap();
    chats::insert_chat_at(
        &mut tx,
        &chats::NewChat {
            id,
            kind: chats::KIND_GROUP,
            org_id: world.root,
            direct_key: None,
            name,
            created_by: world.ada,
            at: world.clock.now(),
        },
        Some(world.topic),
    )
    .await
    .unwrap()
    .unwrap();
    let seq = chats::next_seq(&mut tx, id).await.unwrap().unwrap();
    chats::add_member(
        &mut tx,
        &chats::NewMember {
            chat_id: id,
            user_id: world.ada,
            added_by: Some(world.ada),
            guest: false,
            seq,
            at: world.clock.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}
async fn message(world: &World, chat: Id, text: &str) -> i64 {
    use orch_db::queries::chats;
    let mut tx = world.pool.begin().await.unwrap();
    chats::lock(&mut tx, chat).await.unwrap().unwrap();
    let seq = chats::next_seq(&mut tx, chat).await.unwrap().unwrap();
    chats::insert_message(
        &mut tx,
        &chats::NewMessage {
            id: new_id(),
            chat_id: chat,
            kind: chats::MESSAGE,
            author_id: Some(world.ada),
            author_thread_id: None,
            author_agent: None,
            content: &json!([{"type":"text","text":text}]),
            reply_to_id: None,
            mentions: &[],
            mentions_all: false,
            client_id: None,
            search_text: None,
            seq,
            at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    chats::note_message(&mut tx, chat, seq, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    seq
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_title_uses_full_chat_material_and_its_own_cache_without_renaming_or_advancing_unread_state()
 {
    use orch_db::queries::chats;
    let world = World::on_a_key("synthetic-chat-title-key").await;
    world.engine().stop();
    chosen(&world).await;
    let chat = group(&world, Some("Keep this explicit group name")).await;
    let last = message(&world, chat, "Deployment checks are ready for review.").await;
    chats::mark_read(
        &mut world.pool.acquire().await.unwrap(),
        world.ada,
        chat,
        last,
    )
    .await
    .unwrap();
    let title = SummaryTarget::ChatTitle { id: chat };
    let caller = Caller::session(world.ada).with_admin_mode(false);
    let mut conn = world.pool.acquire().await.unwrap();
    let selected = resource::select(
        &mut conn,
        &caller,
        title,
        None,
        None,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert_eq!(selected.policy.limits().max_chars, 80);
    assert_eq!(selected.material.range.unwrap().after, 0);
    assert!(
        !selected.material.activity.is_empty(),
        "a title is not restricted to unread messages"
    );
    let catch_up = resource::select(
        &mut conn,
        &caller,
        SummaryTarget::Chat { id: chat },
        None,
        None,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert!(catch_up.material.activity.is_empty());
    drop(conn);
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            let Command::ProviderComplete(request) = command else {
                return Err(CommandError::new(ErrorCode::Internal, "not a completion"));
            };
            assert!(request.system.contains("Write a title"));
            assert!(request.input.contains("Deployment checks"));
            answer("Deployment checks")
        });
    let request = accept(&world, title).await;
    world.engine().summarize_resources().await.unwrap();
    eventually("the separate chat title publishes", || async {
        state(&world, request).await == "done"
    })
    .await;
    let cache = rows::get(
        &mut world.pool.acquire().await.unwrap(),
        title,
        Some(world.ada),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cache.text.as_deref(), Some("Deployment checks"));
    assert!(
        rows::get(
            &mut world.pool.acquire().await.unwrap(),
            SummaryTarget::Chat { id: chat },
            Some(world.ada)
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        chats::chat(&mut world.pool.acquire().await.unwrap(), chat)
            .await
            .unwrap()
            .unwrap()
            .name
            .as_deref(),
        Some("Keep this explicit group name")
    );
    assert_eq!(
        chats::state(&mut world.pool.acquire().await.unwrap(), world.ada, chat)
            .await
            .unwrap()
            .unwrap()
            .last_read_seq,
        last
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_chat_titles_leave_explicit_group_names_and_the_model_budget_untouched() {
    let world = World::on_a_key("synthetic-kept-chat-name-key").await;
    world.engine().stop();
    chosen(&world).await;
    world
        .set(
            world.topic,
            "summary.targets",
            json!({"chat_title":{"automatic":true,"min_interval_secs":0}}),
        )
        .await;
    let chat = group(&world, Some("Written group title")).await;
    message(&world, chat, "An ordinary new message.").await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            answer("Do not replace the written title")
        });
    world.engine().summarize_resources().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        rows::get(
            &mut world.pool.acquire().await.unwrap(),
            SummaryTarget::ChatTitle { id: chat },
            None
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        orch_db::queries::chats::chat(&mut world.pool.acquire().await.unwrap(), chat)
            .await
            .unwrap()
            .unwrap()
            .name
            .as_deref(),
        Some("Written group title")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_title_changed_before_its_first_model_call_is_rebuilt_from_the_latest_chat() {
    use orch_engine::summaries::resource::command::observation;
    let world = Arc::new(World::on_a_key("synthetic-title-source-rebuild-key").await);
    world.engine().stop();
    chosen(&world).await;
    let chat = group(&world, Some("Keep the chosen group name")).await;
    message(&world, chat, "The original subject.").await;
    let calls = recording(&world, "The latest subject");
    let request = accept(&world, SummaryTarget::ChatTitle { id: chat }).await;
    let moved = Arc::new(AtomicUsize::new(0));
    let (source, observed) = (world.clone(), moved.clone());
    observation::follow(
        move |id| {
            let (source, observed) = (source.clone(), observed.clone());
            Box::pin(async move {
                if rows::request(&mut source.pool.acquire().await.unwrap(), id)
                    .await
                    .unwrap()
                    .unwrap()
                    .prepared
                    .is_some()
                    && observed.fetch_add(1, Ordering::SeqCst) == 0
                {
                    message(
                        &source,
                        chat,
                        "The new subject before the first title call.",
                    )
                    .await;
                }
            })
        },
        world.engine().summarize_resources(),
    )
    .await
    .unwrap();
    eventually(
        "the unpaid title rebuild publishes its latest source",
        || async { state(&world, request).await == "done" },
    )
    .await;
    let asked = calls.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0]
            .input
            .contains("The new subject before the first title call")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_title_tracks_new_messages_while_catch_up_keeps_its_pinned_range_during_model_io() {
    for title in [true, false] {
        let world = World::on_a_key("synthetic-chat-source-boundary-key").await;
        world.engine().stop();
        chosen(&world).await;
        let chat = group(&world, Some("Keep the chosen group name")).await;
        let through = message(&world, chat, "The original subject.").await;
        let target = if title {
            SummaryTarget::ChatTitle { id: chat }
        } else {
            SummaryTarget::Chat { id: chat }
        };
        let entered = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (observed, lock) = (entered.clone(), gate.clone());
        world
            .machine
            .handler
            .on(CommandKind::ProviderComplete, move |command| {
                let Command::ProviderComplete(request) = command else {
                    panic!("a completion");
                };
                assert!(request.input.contains("The original subject"));
                observed.fetch_add(1, Ordering::SeqCst);
                let (release, wake) = &*lock;
                let release = release.lock().unwrap();
                let (release, _) = wake
                    .wait_timeout_while(release, Duration::from_secs(10), |release| !*release)
                    .unwrap();
                assert!(*release, "the fixture releases its native request");
                answer("The original subject")
            });
        let request = accept(&world, target).await;
        world.engine().summarize_resources().await.unwrap();
        eventually("the actual model call starts", || async {
            entered.load(Ordering::SeqCst) == 1
        })
        .await;
        message(
            &world,
            chat,
            "A later message outside the captured catch-up range.",
        )
        .await;
        rows::invalidate(
            &mut world.pool.acquire().await.unwrap(),
            target,
            world.clock.now(),
        )
        .await
        .unwrap();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        eventually(
            "the paid answer is settled without another call",
            || async {
                matches!(
                    state(&world, request).await.as_str(),
                    "done" | "discarded" | "failed"
                )
            },
        )
        .await;
        if title {
            assert_eq!(state(&world, request).await, "discarded");
            assert_eq!(
                problem_of(&world, request).await["reason"],
                "source_changed"
            );
        } else {
            assert_eq!(state(&world, request).await, "done");
            let cache = rows::get(
                &mut world.pool.acquire().await.unwrap(),
                target,
                Some(world.ada),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(cache.text.as_deref(), Some("The original subject"));
            assert_eq!(
                cache.source_snapshot.unwrap()["range"]["through"],
                json!(through)
            );
            assert_eq!(cache.completed_dirty_revision, Some(cache.dirty_revision));
        }
        world.engine().summarize_resources().await.unwrap();
        assert_eq!(
            entered.load(Ordering::SeqCst),
            1,
            "source movement creates no second paid call"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_whose_providers_key_machine_went_offline_fails_and_says_that_machine_is_missing()
{
    let world = World::on_a_key("synthetic-offline-keeper-key").await;
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(world.topic, "Offline keeper", &["The current source."])
        .await;
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    sqlx::query("UPDATE machines SET online = false WHERE id = $1")
        .bind(world.machine_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().summarize_resources().await.unwrap();
    eventually("the refresh fails, instead of ending unseen", || async {
        state(&world, request).await == "failed"
    })
    .await;
    let problem = problem_of(&world, request).await;
    assert_eq!(
        (problem["reason"].as_str(), problem["message"].as_str()),
        (
            Some("keeper_offline"),
            Some(resource::Reason::KeeperOffline.sentence())
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_whose_model_was_switched_off_fails_and_says_that_the_model_is_off() {
    let world = World::on_a_key("synthetic-model-off-key").await;
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(world.topic, "Model off", &["The current source."])
        .await;
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    sqlx::query("UPDATE provider_models SET enabled = false WHERE provider_id = $1")
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().summarize_resources().await.unwrap();
    eventually("the refresh fails, instead of ending unseen", || async {
        state(&world, request).await == "failed"
    })
    .await;
    let problem = problem_of(&world, request).await;
    assert_eq!(
        (problem["reason"].as_str(), problem["message"].as_str()),
        (
            Some("model_off"),
            Some(resource::Reason::ModelOff.sentence())
        )
    );
}

/// What the scripted machine was asked, in order, and the model's answer to every request.
fn recording(
    world: &World,
    text: &'static str,
) -> Arc<Mutex<Vec<orch_link_contract::commands::ProviderComplete>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = seen.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            let Command::ProviderComplete(request) = command else {
                return Err(CommandError::new(ErrorCode::Internal, "not a completion"));
            };
            observed.lock().unwrap().push(request.clone());
            answer(text)
        });
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summary_asked_of_a_reasoning_model_leaves_room_for_its_reasoning_and_asks_its_lowest_effort()
 {
    let world = World::on_a_key("synthetic-reasoning-room-key").await;
    world.engine().stop();
    chosen(&world).await;
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('efforts',$1::jsonb) WHERE provider_id=$2 AND model_name=$3")
        .bind(json!(["medium", "low", "high"]))
        .bind(world.provider)
        .bind(MODEL)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let document = world
        .document_beside(world.topic, "Reasoning room", &["The current source."])
        .await;
    let seen = recording(&world, "A short summary.");
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    world.engine().summarize_resources().await.unwrap();
    eventually("the summary is written", || async {
        state(&world, request).await == "done"
    })
    .await;
    let asked = seen.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].max_output_tokens,
        1_024 + 2_000,
        "the visible answer's allowance and the room the model reasons in"
    );
    let params = asked[0].params.as_ref().unwrap().to_string();
    assert!(
        params.contains("\"low\"") && !params.contains("\"high\""),
        "the lowest effort the model offers, whatever order its list is in: {params}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_with_unrecorded_or_disabled_tuning_keeps_room_for_its_upstream_reasoning() {
    for parameters in [
        json!({}),
        json!({"efforts":[],"offered":{"report":{"capabilities":{"thinking":{"supported":true}}}}}),
    ] {
        let world = World::on_a_key("synthetic-unrecorded-reasoning-key").await;
        world.engine().stop();
        chosen(&world).await;
        sqlx::query(
            "UPDATE provider_models SET parameters=$3 WHERE provider_id=$1 AND model_name=$2",
        )
        .bind(world.provider)
        .bind(MODEL)
        .bind(&parameters)
        .execute(world.pool.pool())
        .await
        .unwrap();
        let document = world
            .document_beside(world.topic, "Upstream reasoning", &["The current source."])
            .await;
        let seen = recording(&world, "A short summary.");
        let request = accept(&world, SummaryTarget::Document { id: document }).await;
        world.engine().summarize_resources().await.unwrap();
        eventually("the summary with no enabled tuning is written", || async {
            state(&world, request).await == "done"
        })
        .await;
        let asked = seen.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(
            asked[0].max_output_tokens,
            1_024 + 2_000,
            "missing or disabled controls are not proof that the upstream spends no reasoning tokens"
        );
        assert!(
            asked[0]
                .params
                .as_ref()
                .is_none_or(|params| params.get("effort").is_none()),
            "reserving output does not enable an unoffered effort control"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summary_asked_of_a_model_that_does_not_reason_asks_for_the_visible_allowance_only() {
    let world = World::on_a_key("synthetic-plain-allowance-key").await;
    world.engine().stop();
    chosen(&world).await;
    // Absence of controls is not a negative inference fact: this endpoint explicitly reports no thinking.
    sqlx::query("UPDATE provider_models SET parameters=(parameters - 'efforts' - 'default_effort') || $3::jsonb WHERE provider_id=$1 AND model_name=$2")
        .bind(world.provider)
        .bind(MODEL)
        .bind(json!({"offered":{"report":{"capabilities":{"thinking":{"supported":false}}}}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let document = world
        .document_beside(world.topic, "Plain allowance", &["The current source."])
        .await;
    let seen = recording(&world, "A short summary.");
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    world.engine().summarize_resources().await.unwrap();
    eventually("the summary is written", || async {
        state(&world, request).await == "done"
    })
    .await;
    let asked = seen.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].max_output_tokens, 1_024);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_whose_answer_ran_out_of_output_fails_and_says_to_raise_the_allowance() {
    let world = World::on_a_key("synthetic-cut-answer-key").await;
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(world.topic, "Cut answer", &["The current source."])
        .await;
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, |_| {
            Err(CommandError::new(
                ErrorCode::Internal,
                "the model stopped before its answer was whole: max_output_tokens",
            )
            .with_details(json!({"failure":{
                "code":"harness_error","reason":"max_output_tokens","output_limit":true,"retryable":false
            }})))
        });
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    world.engine().summarize_resources().await.unwrap();
    eventually("the refresh fails, instead of ending unseen", || async {
        state(&world, request).await == "failed"
    })
    .await;
    let problem = problem_of(&world, request).await;
    assert_eq!(
        (problem["reason"].as_str(), problem["message"].as_str()),
        (
            Some("output_cut"),
            Some(resource::Reason::OutputCut.sentence())
        )
    );
}

/// The refresh of an empty source ends with its reason and has asked no model: no command, no model input and no use.
async fn assert_nothing_new_and_nothing_asked(world: &World, request: Id, calls: &AtomicUsize) {
    eventually("the refresh ends without a model", || async {
        state(world, request).await == "failed"
    })
    .await;
    let problem = problem_of(world, request).await;
    assert_eq!(
        (problem["reason"].as_str(), problem["message"].as_str()),
        (
            Some("nothing_new"),
            Some(resource::Reason::NothingNew.sentence())
        )
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the model was not asked");
    let asked: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commands WHERE kind='provider.complete'),(SELECT count(*) FROM usage_events WHERE purpose='summary'),(SELECT count(*) FROM resource_summary_model_inputs)")
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        asked,
        (0, 0, 0),
        "no command, no use booked, no model input"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_catch_up_with_nothing_unread_asks_no_model_books_no_use_and_says_there_is_nothing_new() {
    use orch_db::queries::chats;
    let world = World::on_a_key("synthetic-nothing-unread-key").await;
    world.engine().stop();
    chosen(&world).await;
    let chat = group(&world, Some("Everything here was read")).await;
    let last = message(&world, chat, "A message its sender has read.").await;
    chats::mark_read(
        &mut world.pool.acquire().await.unwrap(),
        world.ada,
        chat,
        last,
    )
    .await
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            answer("No activity material was provided to summarize")
        });
    let request = accept(&world, SummaryTarget::Chat { id: chat }).await;
    world.engine().summarize_resources().await.unwrap();
    assert_nothing_new_and_nothing_asked(&world, request, &calls).await;
    assert!(
        rows::get(
            &mut world.pool.acquire().await.unwrap(),
            SummaryTarget::Chat { id: chat },
            Some(world.ada)
        )
        .await
        .unwrap()
        .is_none_or(|cache| cache.text.is_none()),
        "no paragraph is written about nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_of_a_document_with_nothing_written_asks_no_model_and_books_no_use() {
    let world = World::on_a_key("synthetic-empty-document-key").await;
    world.engine().stop();
    chosen(&world).await;
    let document = world
        .document_beside(world.topic, "Nothing written yet", &[])
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            answer("No activity material was provided to summarize")
        });
    let request = accept(&world, SummaryTarget::Document { id: document }).await;
    world.engine().summarize_resources().await.unwrap();
    assert_nothing_new_and_nothing_asked(&world, request, &calls).await;
}
