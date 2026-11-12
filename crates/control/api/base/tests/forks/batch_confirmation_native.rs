//! The native batch producer keeps one actual transaction and original source at its first job insert.
use super::*;
use orch_api_contract::{fork_sources, routes::Endpoint};
use orch_authz::{Caller, Principal, TokenGrant};
use orch_engine::fork_plan::operation::{self as source, BatchConfirm};
use orch_engine::{EngineError, fork_plan::PlanError};
use orch_types::Time;
use thread_history_scope::{Proof, key, rule};
const OP: &str = "ForkBatchCreate.switch";

#[tokio::test(flavor = "multi_thread")]
async fn native_batch_confirmation_owns_its_first_insert_and_actual_original_sources_without_replaying_suppliers()
-> Proof {
    let w = Forks::new().await;
    let a =
        w.s.thread_by(&w.s.ada, "First confirmed native member")
            .await
            .base
            .id;
    let b =
        w.s.thread_by(&w.s.ada, "Second confirmed native member")
            .await
            .base
            .id;
    let plan = w
        .batch_plan(&w.s.ada, &w.batch_of(vec![b, a, a]))
        .await
        .ok();
    w.s.rig.world.app.app.engine.stop();
    let world = &w.s.rig.world;
    let grants = vec![
        rule(OP, "thread", a, "self", "manage"),
        rule(OP, "thread", b, "self", "manage"),
    ];
    let actor = key(world, w.s.ada.id(), grants.clone()).await?;
    let until = Time::new(world.now() + chrono::Duration::hours(1));
    let caller = Caller::token(
        w.s.ada.id(),
        TokenGrant::from_stored_scope(
            &[],
            None,
            Some(serde_json::from_value(json!({"rules":grants}))?),
            Some(until),
        )?
        .for_token(actor.id),
    );
    let body = ForkCreateBody {
        preflight_id: plan.preflight_id,
        notify: None,
        options: None,
    };
    let clock = world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    let mut tx = w.s.rig.db.begin_snapshot().await?;
    let native = world
        .app
        .app
        .engine
        .select_batch_confirmation(&mut tx, &caller, &body)
        .await?;
    assert_eq!(native.selections().len(), 2);
    let mut inputs = Vec::new();
    let mut targets = Vec::new();
    for member in native.selections() {
        inputs.push(source::Input::selected(&mut tx, member.selected()).await?);
        targets.push(
            orch_engine::operation_scope::targets::page_object(
                &mut tx,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Thread,
                    id: member.member().thread,
                },
            )
            .await?,
        );
    }
    let facts = vec![
        orch_engine::operation_scope::sources::Facts::collection(
            fork_sources::PlanSource::Sources,
            vec![(
                orch_access_contract::key_scope::KeyTargetKind::Object(
                    orch_access_contract::permissions::ObjectKind::Thread,
                ),
                targets,
            )],
        ),
        orch_engine::operation_scope::sources::Facts::collection(
            fork_sources::PlanSource::CopiedPages,
            Vec::new(),
        ),
    ];
    let bound = orch_engine::operation_scope::sources::bind_required(
        ForkBatchCreate::ROUTE,
        &fork_sources::BATCH_CONFIRM,
        &facts,
    )?;
    let caller = bound.admit(&caller, false)?;
    let mut suppliers = Vec::new();
    for input in &inputs {
        let origin = source::Origin::capture::<BatchConfirm>(
            &caller,
            input.clone(),
            orch_engine::operation_scope::request_origin::Credential::Token {
                id: actor.id,
                hash: orch_api_core::middleware::authentication::token_hash(&actor.secret).to_vec(),
            },
            Some(until),
        )?;
        suppliers.push(source::Preparation::new(origin));
    }
    tx.commit().await?;
    let drafts = world
        .app
        .app
        .engine
        .assemble_batch_confirmation(&mut *world.conn().await, &native, Some(&suppliers))
        .await?;
    assert!(
        drafts
            .iter()
            .all(|draft| draft.as_ref().is_some_and(|draft| draft.is_clear()))
    );
    let mut origins = Vec::new();
    for supplier in &suppliers {
        origins.push(supplier.retained().await);
    }
    let first = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for rejected in [true, false] {
        let mut tx = w.s.rig.db.begin().await?;
        orch_db::queries::transaction_guard::lock_native_actor(&mut tx, caller.user()).await?;
        orch_db::queries::jobs::forks::lock_preflight(&mut tx, body.preflight_id)
            .await?
            .ok_or("native kept plan missing")?;
        source::lock_all(&mut tx, &inputs).await?;
        let mut acting = caller.clone();
        for origin in &origins {
            let (principal, _) = origin
                .current::<BatchConfirm>(&mut tx, clock.as_ref(), &access)
                .await?;
            acting = acting.intersect_restrictions(principal.caller())?;
        }
        let acting = bound.admit(&acting, false)?;
        let principal = Principal::load(&mut tx, acting, clock.time()).await?;
        let current = world
            .app
            .app
            .engine
            .prepare_batch_confirmation(&mut tx, principal.caller(), &body, native.batch())
            .await?;
        assert!(native.preserves(&current));
        let count = first.clone();
        let queued = world
            .app
            .app
            .engine
            .commit_batch_confirmation_with_source(
                tx,
                &principal,
                &current,
                drafts.clone(),
                &origins,
                move || {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if rejected {
                        Err(EngineError::BeforeEffect(Box::new(std::io::Error::other(
                            "the actual first insertion is refused",
                        ))))
                    } else {
                        Ok(())
                    }
                },
                || Ok(()),
            )
            .await;
        if rejected {
            assert!(matches!(
                queued,
                Err(PlanError::Internal(EngineError::BeforeEffect(_)))
            ));
            assert_eq!(
                w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                    .await,
                0
            );
            assert!(
                orch_db::queries::jobs::forks::preflight(
                    &mut *world.conn().await,
                    body.preflight_id
                )
                .await?
                .is_some()
            );
        } else {
            let (batch, jobs) = queued?;
            assert_eq!(batch, native.batch());
            assert_eq!(jobs.len(), 2);
            for job in jobs {
                assert_eq!(job.batch_id, Some(batch));
                assert_eq!(job.created_by, Some(caller.user()));
                assert_eq!(
                    job.plan["confirmed_history_source"]["operation"]["origin"]["operation"],
                    OP
                );
                assert!(orch_engine::jobs::fork::operation::Preparation::from_job(&job)?.is_some());
            }
            assert!(
                orch_db::queries::jobs::forks::preflight(
                    &mut *world.conn().await,
                    body.preflight_id
                )
                .await?
                .is_none()
            );
        }
    }
    assert_eq!(
        first.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "one first insert check per native attempt, not one per member"
    );
    Ok(())
}
