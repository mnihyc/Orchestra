//! Accepted Stops withdraw only their original session admissions, before native preparation.
use super::*;
use orch_db::queries::environments::hosting as environment_rows;
use orch_db::queries::resources::{access_fixtures as fixtures, provider_runtime as provider_rows};
use orch_db::queries::{runtime_controls, runtime_incarnations};
use orch_link_contract::commands::{TicketSlot, TicketWanted};
use orch_link_contract::native_target::RuntimeSource;
use orch_link_contract::tool_sessions::ToolSessionQuery;
use orch_link_contract::tunnels::TunnelKind;

#[derive(Clone, Copy, Debug)]
enum UnitKind {
    Environment,
    Provider,
}

async fn managed_runtime(office: &Office, environment: Id, provider: Id, kind: UnitKind) -> Id {
    let mut tx = office.db.pool.begin().await.unwrap();
    let (host, template, runtime) = (new_id(), new_id(), new_id());
    let label = match kind {
        UnitKind::Environment => "environment",
        UnitKind::Provider => "provider",
    };
    sqlx::query("INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Tool Stop host')")
        .bind(host)
        .bind(office.other)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO env_templates(id,name,kind,driver,spec) VALUES($1,'Tool Stop template',$2,'test','{}')")
        .bind(template).bind(label).execute(&mut *tx).await.unwrap();
    let volumes = json!({"state":{"name":"tool-stop-data","storage":null}});
    match kind {
        UnitKind::Environment => {
            sqlx::query("UPDATE environments SET placement='managed',template_id=$2 WHERE id=$1")
                .bind(environment)
                .bind(template)
                .execute(&mut *tx)
                .await
                .unwrap();
            environment_rows::insert_runtime(
                &mut tx,
                &environment_rows::NewRuntime {
                    id: runtime,
                    host_id: host,
                    unit_name: "tool-stop-unit",
                    environment_id: environment,
                    driver: "test",
                    template_id: template,
                    spec_hash: &[0; 32],
                    generation: 1,
                    volumes: &volumes,
                },
                office.app.clock.now(),
            )
            .await
            .unwrap();
        }
        UnitKind::Provider => {
            provider_rows::insert_runtime(
                &mut tx,
                &provider_rows::NewRuntime {
                    id: runtime,
                    host_id: host,
                    unit_name: "tool-stop-unit",
                    provider_id: provider,
                    driver: "test",
                    template_id: template,
                    spec_hash: &[0; 32],
                    generation: 1,
                    volumes: &volumes,
                },
                office.app.clock.now(),
            )
            .await
            .unwrap();
        }
    }
    assert!(
        orch_db::queries::resources::hosting::adopt_machine(
            &mut tx,
            office.owner,
            runtime,
            office.app.clock.now(),
        )
        .await
        .unwrap()
    );
    if matches!(kind, UnitKind::Provider) {
        assert!(
            provider_rows::set_machine(
                &mut tx,
                provider,
                Some(office.owner),
                office.app.clock.now(),
            )
            .await
            .unwrap()
        );
        let registered = provider_rows::state(&mut tx, provider)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(registered.machine_id, Some(office.owner));
        assert_eq!(registered.placement, "managed");
    }
    assert!(
        environment_rows::set_runtime_state(
            &mut tx,
            runtime,
            &["creating"],
            "running",
            None,
            office.app.clock.now(),
        )
        .await
        .unwrap()
    );
    assert!(
        runtime_incarnations::observe(
            &mut tx,
            host,
            runtime,
            RuntimeSource {
                incarnation: Some(new_id())
            },
            false,
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
    runtime
}

async fn prepare_stop(
    office: &Office,
    environment: Id,
    provider: Id,
    runtime: Id,
    kind: UnitKind,
) -> runtime_incarnations::Stop {
    let mut tx = office.db.pool.begin().await.unwrap();
    let stop = prepare_stop_in(office, &mut tx, environment, provider, runtime, kind).await;
    tx.commit().await.unwrap();
    stop
}

async fn prepare_stop_in(
    office: &Office,
    conn: &mut Conn,
    environment: Id,
    provider: Id,
    runtime: Id,
    kind: UnitKind,
) -> runtime_incarnations::Stop {
    match kind {
        UnitKind::Environment => {
            environment_rows::lock_environment(conn, environment)
                .await
                .unwrap()
                .unwrap();
        }
        UnitKind::Provider => {
            provider_rows::lock(conn, provider).await.unwrap().unwrap();
        }
    }
    let control = runtime_controls::accept(conn, runtime, new_id())
        .await
        .unwrap();
    assert!(
        runtime_incarnations::prepare_stop(conn, runtime, &control, office.app.clock.now())
            .await
            .unwrap()
    );
    let stop = runtime_incarnations::stop(conn, runtime)
        .await
        .unwrap()
        .unwrap();
    assert!(!stop.ready);
    stop
}

async fn question(office: &Office, thread: Id, segment: Id, environment: Id) -> ToolSessionQuery {
    question_in(
        office,
        &mut office.db.pool.acquire().await.unwrap(),
        thread,
        segment,
        environment,
    )
    .await
}

async fn question_in(
    office: &Office,
    conn: &mut Conn,
    thread: Id,
    segment: Id,
    environment: Id,
) -> ToolSessionQuery {
    let wanted = TicketWanted {
        kind: TunnelKind::Exec,
        more: json!({"service":"exec/orch","dst":office.owner,"thread_id":thread,
            "environment_id":environment,"environment_generation":1})
        .as_object()
        .unwrap()
        .clone(),
    };
    let mut slot = TicketSlot::ask(TunnelKind::Exec);
    if let TicketSlot::Asked(ask) = &mut slot {
        ask.ticket = wanted.clone();
    }
    let body = json!({"thread_id":thread,"segment_id":segment,
        "exec":{"environment_id":environment,"generation":1,"ticket":slot},
        "mcp":[],"skills":[]});
    admitted_tool_question(office, conn, &body, &wanted).await
}

async fn independent_question(office: &Office) -> (Id, ToolSessionQuery) {
    let mut conn = office.db.pool.acquire().await.unwrap();
    let topic: Id = sqlx::query_scalar("SELECT topic_id FROM threads WHERE node_id=$1")
        .bind(office.thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let source = threads::segment(&mut conn, office.segment)
        .await
        .unwrap()
        .unwrap();
    let environment = fixtures::insert_environment(
        &mut conn,
        office.person,
        Some(topic),
        "Independent tools",
        "connected",
    )
    .await
    .unwrap();
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(office.person),
        Some(topic),
        "Independent provider",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2,status='running',generation=1 WHERE id=$1")
        .bind(environment)
        .bind(office.owner)
        .execute(&mut *conn)
        .await
        .unwrap();
    let (thread, segment) = (new_id(), new_id());
    assert!(
        threads::insert_thread(
            &mut conn,
            thread,
            topic,
            "Independent session",
            office.person,
            office.app.clock.now(),
        )
        .await
        .unwrap()
    );
    threads::insert_segment(
        &mut conn,
        &threads::NewSegment {
            id: segment,
            thread_id: thread,
            agent_id: source.agent_id,
            provider_id: provider,
            environment_id: environment,
            env_generation: 1,
            arrangement: "together",
            executor: None,
            tool_set: None,
            credential_path: "native",
            native_session_ref: None,
            started_by: Some(office.person),
            started_at: office.app.clock.now(),
        },
        1,
    )
    .await
    .unwrap();
    threads::set_current(&mut conn, thread, Some(segment), None, 1)
        .await
        .unwrap();
    assert!(
        machine_streams::bind_segment(&mut conn, thread, segment, office.owner)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE threads SET environment_id=$2,provider_id=$3,agent_id=$4 WHERE node_id=$1")
        .bind(thread)
        .bind(environment)
        .bind(provider)
        .bind(source.agent_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    (
        segment,
        question(office, thread, segment, environment).await,
    )
}

async fn a_preparing_stop_withdraws_its_tool_channels_and_keeps_an_independent_session_usable() {
    for kind in [UnitKind::Environment, UnitKind::Provider] {
        let (office, environment, provider) = tool_office().await;
        let runtime = managed_runtime(&office, environment, provider, kind).await;
        let original = question(&office, office.thread, office.segment, environment).await;
        let (other_segment, independent) = independent_question(&office).await;
        let link = &office.app.app.link;
        assert!(link.authorize_tool_session(&original).await.allowed);
        assert!(link.authorize_tool_session(&independent).await.allowed);
        let stop = prepare_stop(&office, environment, provider, runtime, kind).await;
        let current =
            environment_rows::runtime(&mut office.db.pool.acquire().await.unwrap(), runtime)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            current.state, "running",
            "{kind:?}: preparation is not a physical Stop"
        );
        assert!(
            !link.authorize_tool_session(&original).await.allowed,
            "{kind:?}: an accepted Stop ends its original admission before a native reply"
        );
        let withdrawn = link
            .tool_session_snapshot(office.segment)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !withdrawn.live && withdrawn.members.is_empty(),
            "{kind:?}: existing channels receive the same durable withdrawal"
        );
        assert_eq!(
            withdrawn.opening_id,
            original.claims.tool_session.as_ref().map(|s| s.opening_id)
        );
        assert!(withdrawn.revision > original.claims.tool_session.as_ref().unwrap().revision);
        assert!(link.authorize_tool_session(&independent).await.allowed);
        let other = link
            .tool_session_snapshot(other_segment)
            .await
            .unwrap()
            .unwrap();
        assert!(
            other.live && !other.members.is_empty(),
            "an unrelated session on the same machine stays live"
        );

        let during_stop = question(&office, office.thread, office.segment, environment).await;
        assert!(
            !link.authorize_tool_session(&during_stop).await.allowed,
            "preparation cannot admit a replacement tool opening"
        );
        let mut tx = office.db.pool.begin().await.unwrap();
        match kind {
            UnitKind::Environment => {
                environment_rows::lock_environment(&mut tx, environment)
                    .await
                    .unwrap()
                    .unwrap();
            }
            UnitKind::Provider => {
                provider_rows::lock(&mut tx, provider)
                    .await
                    .unwrap()
                    .unwrap();
            }
        }
        runtime_controls::accept(&mut tx, runtime, new_id())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(
            !link.authorize_tool_session(&during_stop).await.allowed,
            "superseding Stop never revives a nonce attempted during withdrawal"
        );
        assert!(!link.authorize_tool_session(&original).await.allowed);
        assert!(link.authorize_tool_session(&independent).await.allowed);
        assert_eq!(
            runtime_incarnations::stop(&mut office.db.pool.acquire().await.unwrap(), runtime)
                .await
                .unwrap(),
            Some(stop),
            "a channel withdrawal does not arm or replace the Stop"
        );
        let next = question(&office, office.thread, office.segment, environment).await;
        assert!(
            link.authorize_tool_session(&next).await.allowed,
            "a distinct current opening remains available after the old control ends"
        );
    }
}

async fn an_old_stop_does_not_withdraw_tools_admitted_in_a_changed_home() {
    for kind in [UnitKind::Environment, UnitKind::Provider] {
        let (office, environment, provider) = tool_office().await;
        let runtime = managed_runtime(&office, environment, provider, kind).await;
        let stop = prepare_stop(&office, environment, provider, runtime, kind).await;
        let mut conn = office.db.pool.acquire().await.unwrap();
        let next_home: Id = sqlx::query_scalar("SELECT id FROM nodes WHERE kind='project'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        match kind {
            UnitKind::Environment => {
                sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
                    .bind(environment)
                    .bind(next_home)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
            UnitKind::Provider => {
                sqlx::query("UPDATE providers SET home_id=$2 WHERE id=$1")
                    .bind(provider)
                    .bind(next_home)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
        }
        assert!(
            !runtime_incarnations::withdrawn(&mut conn, runtime)
                .await
                .unwrap()
        );
        drop(conn);
        let current = question(&office, office.thread, office.segment, environment).await;
        let link = &office.app.app.link;
        assert!(
            link.authorize_tool_session(&current).await.allowed,
            "{kind:?}: a new home cannot inherit the old Stop"
        );
        sqlx::query("UPDATE runtimes SET native_stop=native_stop,state_detail='The old Stop is still unconfirmed' WHERE id=$1")
            .bind(runtime).execute(&office.db.pool).await.unwrap();
        assert!(link.authorize_tool_session(&current).await.allowed);
        assert!(
            link.tool_session_snapshot(office.segment)
                .await
                .unwrap()
                .unwrap()
                .live
        );
        assert_eq!(
            runtime_incarnations::stop(&mut office.db.pool.acquire().await.unwrap(), runtime)
                .await
                .unwrap(),
            Some(stop)
        );
    }
}

async fn a_tool_opening_and_a_stop_have_one_order_before_preparation_even_for_a_new_session() {
    for kind in [UnitKind::Environment, UnitKind::Provider] {
        for stop_first in [true, false] {
            let (office, environment, provider) = tool_office().await;
            let runtime = managed_runtime(&office, environment, provider, kind).await;
            let (other_segment, independent) = independent_question(&office).await;
            let link = &office.app.app.link;
            let mut winner = office.db.pool.begin().await.unwrap();
            let winner_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *winner)
                .await
                .unwrap();
            let current = if stop_first {
                prepare_stop_in(&office, &mut winner, environment, provider, runtime, kind).await;
                let mut opening = Box::pin(question(
                    &office,
                    office.thread,
                    office.segment,
                    environment,
                ));
                tokio::time::timeout(ACK_WAIT, async {
                    loop {
                        tokio::select! {
                            result = &mut opening => panic!("a new opening crossed the held original Stop: {result:?}"),
                            blocked = sqlx::query_scalar::<_,bool>(
                                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%runtimes%')"
                            ).bind(winner_pid).fetch_one(&office.db.pool) => {
                                if blocked.unwrap() { break; }
                            }
                        }
                    }
                }).await.expect("the opening waits on the actual original runtime row");
                assert!(
                    link.authorize_tool_session(&independent).await.allowed,
                    "the held Stop does not lock another session on the same machine"
                );
                winner.commit().await.unwrap();
                tokio::time::timeout(ACK_WAIT, opening).await.unwrap()
            } else {
                let current = question_in(
                    &office,
                    &mut winner,
                    office.thread,
                    office.segment,
                    environment,
                )
                .await;
                let mut stopping =
                    Box::pin(prepare_stop(&office, environment, provider, runtime, kind));
                tokio::time::timeout(ACK_WAIT, async {
                    loop {
                        tokio::select! {
                            result = &mut stopping => panic!("Stop crossed the uncommitted original admission: {result:?}"),
                            blocked = sqlx::query_scalar::<_,bool>(
                                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%runtimes%')"
                            ).bind(winner_pid).fetch_one(&office.db.pool) => {
                                if blocked.unwrap() { break; }
                            }
                        }
                    }
                }).await.expect("Stop waits on the actual admission's runtime row");
                winner.commit().await.unwrap();
                tokio::time::timeout(ACK_WAIT, stopping).await.unwrap();
                current
            };
            assert!(
                !link.authorize_tool_session(&current).await.allowed,
                "{kind:?}, Stop first={stop_first}: neither order leaves an admitted channel"
            );
            assert!(link.authorize_tool_session(&independent).await.allowed);
            let update = link
                .tool_session_snapshot(office.segment)
                .await
                .unwrap()
                .unwrap();
            assert!(!update.live && update.members.is_empty());
            assert!(
                link.tool_session_snapshot(other_segment)
                    .await
                    .unwrap()
                    .unwrap()
                    .live
            );
        }
    }
}

#[doc(hidden)]
pub fn __api_run_a_preparing_stop_withdraws_its_tool_channels_and_keeps_an_independent_session_usable()
-> futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(a_preparing_stop_withdraws_its_tool_channels_and_keeps_an_independent_session_usable())
}

#[doc(hidden)]
pub fn __api_run_an_old_stop_does_not_withdraw_tools_admitted_in_a_changed_home()
-> futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(an_old_stop_does_not_withdraw_tools_admitted_in_a_changed_home())
}

#[doc(hidden)]
pub fn __api_run_a_tool_opening_and_a_stop_have_one_order_before_preparation_even_for_a_new_session()
-> futures::future::LocalBoxFuture<'static, ()> {
    Box::pin(a_tool_opening_and_a_stop_have_one_order_before_preparation_even_for_a_new_session())
}
