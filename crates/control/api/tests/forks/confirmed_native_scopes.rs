//! The native retained source keeps its real key, operation, binding and earliest accepted budget.
use super::*;
use orch_api_contract::{fork_sources, routes::Endpoint};
use orch_authz::{Caller, TokenGrant};
use orch_engine::{
    EngineError,
    fork_plan::operation::{self as source, Confirm},
};
use orch_types::Time;
use thread_history_scope::{Proof, key, narrow, rule};
const OP: &str = "ThreadForkCreate.switch";

#[tokio::test(flavor = "multi_thread")]
async fn confirmed_native_source_keeps_its_original_operation_raw_key_minimum_and_binding_without_a_startup_alias()
-> Proof {
    for mode in [
        "live",
        "scope",
        "expired",
        "minimum",
        "association",
        "malformed",
    ] {
        let w = Forks::new().await;
        let thread =
            w.s.thread_by(&w.s.ada, "Native retained source")
                .await
                .base
                .id;
        let plan = w.plan(&w.s.ada, thread, &w.to_second()).await.ok();
        let world = &w.s.rig.world;
        world.app.app.engine.stop();
        let grants = vec![rule(OP, "thread", thread, "self", "manage")];
        let selected = key(world, w.s.ada.id(), grants.clone()).await?;
        let until = Time::new(world.now() + chrono::Duration::hours(1));
        let caller = Caller::token(
            w.s.ada.id(),
            TokenGrant::from_stored_scope(
                &[],
                None,
                Some(serde_json::from_value(json!({"rules":grants}))?),
                Some(until),
            )?
            .for_token(selected.id),
        );
        let body = ForkCreateBody {
            preflight_id: plan.preflight_id,
            notify: None,
            options: None,
        };
        let mut tx = w.s.rig.db.begin().await?;
        let prepared = world
            .app
            .app
            .engine
            .prepare_fork_confirmation(&mut tx, &caller, thread, &body)
            .await?;
        let input = source::Input::selected(&mut tx, prepared.selected()).await?;
        let facts = source::facts::<Confirm>(&mut tx, &input).await?;
        let bound = orch_engine::operation_scope::sources::bind_required(
            ThreadForkCreate::ROUTE,
            &fork_sources::CONFIRM_SWITCH,
            &facts,
        )?;
        let caller = bound.admit(&caller, false)?;
        let origin = source::Origin::capture::<Confirm>(
            &caller,
            input,
            orch_engine::operation_scope::request_origin::Credential::Token {
                id: selected.id,
                hash: orch_api_core::middleware::authentication::token_hash(&selected.secret)
                    .to_vec(),
            },
            Some(until),
        )?;
        let clock = world.app.app.clock.clone();
        let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
        let (principal, _) = origin
            .current::<Confirm>(&mut tx, clock.as_ref(), &access)
            .await?;
        let draft = world
            .app
            .app
            .engine
            .assemble_prepared_fork(&mut tx, prepared.selected())
            .await?;
        let row = world
            .app
            .app
            .engine
            .commit_fork_confirmation_with_source(
                tx,
                principal.caller(),
                &prepared,
                draft,
                &origin,
                || Ok(()),
                || Ok(()),
            )
            .await?;
        assert_eq!(
            row.plan["confirmed_history_source"]["operation"]["origin"]["operation"],
            OP
        );
        assert_eq!(
            row.plan["confirmed_history_source"]["operation"]["credential"]["kind"],
            "token"
        );
        assert!(
            row.plan["confirmed_history_source"]
                .get("token")
                .is_some_and(Json::is_null)
        );
        let mut preparation = orch_engine::jobs::fork::operation::Preparation::from_job(&row)?
            .ok_or("retained operation missing")?;
        let mut tx = w.s.rig.db.begin().await?;
        let live = preparation
            .current(&mut tx, clock.as_ref(), &access)
            .await?;
        assert_eq!(live.caller().admitted_operation(), Some(OP));
        live.check(clock.as_ref())?;
        tx.rollback().await?;
        match mode {
            "scope" => {
                narrow(
                    world,
                    &selected,
                    vec![rule(OP, "thread", thread, "self", "read")],
                )
                .await?
            }
            "expired" => world.advance(chrono::Duration::hours(2)),
            "minimum" => {
                let earlier = Time::new(world.now() + chrono::Duration::minutes(10));
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(selected.id)
                    .bind(earlier.get())
                    .execute(w.s.rig.db.pool())
                    .await?;
                let mut tx = w.s.rig.db.begin().await?;
                let narrower = preparation
                    .current(&mut tx, clock.as_ref(), &access)
                    .await?;
                preparation = preparation.accepted_at(&narrower)?;
                assert_eq!(narrower.until(), Some(earlier));
                tx.rollback().await?;
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(selected.id)
                    .bind(world.now() + chrono::Duration::hours(3))
                    .execute(w.s.rig.db.pool())
                    .await?;
                world.advance(chrono::Duration::minutes(20));
            }
            "association" => {
                let other =
                    another_environment(&w.s, "Replaced native source", Some(w.s.rig.machine_id))
                        .await;
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(thread)
                    .bind(other)
                    .execute(w.s.rig.db.pool())
                    .await?;
            }
            "malformed" => {
                sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{confirmed_history_source,operation}','null') WHERE id=$1")
                    .bind(row.id).execute(w.s.rig.db.pool()).await?;
            }
            _ => {}
        }
        let mut tx = w.s.rig.db.begin().await?;
        let current = preparation.current(&mut tx, clock.as_ref(), &access).await;
        if mode == "live" {
            current?.check(clock.as_ref())?;
        } else {
            assert!(
                matches!(current, Err(EngineError::BeforeEffect(_))),
                "native source did not refuse {mode}"
            );
        }
        tx.rollback().await?;
        assert_eq!(
            w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                .await,
            1
        );
        assert_eq!(w.s.rig.commands.issued_of("session.export"), 0);
    }
    Ok(())
}
