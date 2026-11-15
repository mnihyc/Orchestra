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
