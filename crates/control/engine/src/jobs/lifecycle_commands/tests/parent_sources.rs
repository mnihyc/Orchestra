//! Real reviewed Provider children keep the original Host source and independent management.
use super::*;
use crate::jobs::host_delete::reviewed::ProviderAction;
use crate::operation_scope::{
    http_origin::HttpOrigin,
    request_origin::Credential,
    sources::{self, Facts},
    targets,
};
use orch_authz::{
    CapturedOperation,
    ownership::{self, NewAccess},
};
use orch_db::queries::resources::{hosting as hosts, provider_runtime as providers};

/// Queue through the ordinary native writers; the child port is the final authority boundary.
async fn queued(
    replace: bool,
    provider_scope: bool,
    provider_manager: bool,
) -> TestResult<(World, Id, Id)> {
    let world = World::observed().await;
    let source_machine = world.add_machine("Departing native host", &["node"]).await;
    let target_machine = world.add_machine("Target native host", &["node"]).await;
    world.engine().stop();
    let actor = if provider_manager {
        world.ada
    } else {
        world.person("host-manager-only").await
    };
    let (host, target, template, key) = (new_id(), new_id(), new_id(), new_id());
    let digest = Hash::of(key.as_bytes());
    let expires = Time::new(world.clock.now() + chrono::Duration::hours(1));
    let mut rules = vec![
        json!({"target":{"kind":"host","id":host,"reach":"self"},"allow":[{"operation":"HostDeleteCreate","actions":["manage"]}]}),
        json!({"target":{"kind":"host","id":target,"reach":"self"},"allow":[{"operation":"HostDeleteCreate","actions":["write"]}]}),
        json!({"target":{"kind":"template","id":template,"reach":"self"},"allow":[{"operation":"HostDeleteCreate","actions":["write"]}]}),
    ];
    if provider_scope {
        rules.push(json!({"target":{"kind":"provider","id":world.provider,"reach":"self"},"allow":[{"operation":"HostDeleteCreate","actions":["manage"]}]}));
    }
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules}))?;
    assert!(policy.validate().is_empty());
    let mut tx = world.pool.begin().await?;
    for (id, machine, name) in [
        (host, source_machine.id, "Departing host"),
        (target, target_machine.id, "Destination host"),
    ] {
        sqlx::query("INSERT INTO hosts(id,machine_id,name,owner_id,org_id,home_id,drivers,capacity) VALUES($1,$2,$3,$4,$5,$5,$6,$7)")
            .bind(id).bind(machine).bind(name).bind(actor).bind(world.root)
            .bind(json!({"test":{"spec_schemas":{"provider":{}},"sources":["image"]}})).bind(json!({"memory_mb":4096})).execute(&mut *tx).await?;
        ownership::start_resource(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Host,
                id,
            },
            actor,
            NewAccess::OnlyMe,
            world.clock.time(),
        )
        .await?;
    }
    let definition = json!({"image":"reviewed-provider"});
    sqlx::query("INSERT INTO env_templates(id,name,owner_id,org_id,home_id,kind,driver,spec) VALUES($1,'Reviewed provider',$2,$3,$3,'provider','test',$4)")
        .bind(template).bind(actor).bind(world.root).bind(&definition).execute(&mut *tx).await?;
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ObjectKind::Template,
            id: template,
        },
        actor,
        NewAccess::OnlyMe,
        world.clock.time(),
    )
    .await?;
    sqlx::query("UPDATE providers SET runtime_config=runtime_config||jsonb_build_object('runtime.host_id',$2::uuid::text,'runtime.template_id',$3::uuid::text) WHERE id=$1")
        .bind(world.provider).bind(target).bind(template).execute(&mut *tx).await?;
    identity::create_api_token_scoped(
        &mut tx,
        &identity::NewApiToken {
            id: key,
            user_id: actor,
            name: "Reviewed Host source",
            token_hash: digest.as_bytes(),
            scopes: &[],
            node_id: None,
            at: world.clock.now(),
            expires_at: Some(expires.get()),
        },
        Some(&json!(policy)),
    )
    .await?;
    let caller = Caller::token(
        actor,
        TokenGrant::from_stored_scope(&[], None, Some(policy), Some(expires))?.for_token(key),
    );
    use orch_api_contract::host_delete_sources::{CREATE, Source};
    let facts = [
        Facts::one(
            Source::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            targets::resource_identity(
                &mut tx,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host,
                },
            )
            .await?,
        ),
        Facts::collection(Source::Managed, vec![]),
        Facts::collection(Source::Used, vec![]),
    ];
    let caller = sources::bind_required(
        <orch_api_contract::loss::HostDeleteCreate as orch_api_contract::routes::Endpoint>::ROUTE,
        &CREATE,
        &facts,
    )?
    .admit(&caller, false)?;
    let principal = Principal::load(&mut tx, caller.clone(), world.clock.time()).await?;
    let http = HttpOrigin::capture(
        CapturedOperation::capture(&caller)?,
        Credential::Token {
            id: key,
            hash: digest.as_bytes().to_vec(),
        },
        false,
        principal.has_system_admin_standing(),
        Some(expires),
    )
    .map_err(|e| format!("raw source: {e}"))?;
    let (child, reviewed) = if replace {
        let old = new_id();
        let hash = crate::updates::definition_hash("test", &definition, &json!({}), &[]);
        providers::insert_runtime(
            &mut tx,
            &providers::NewRuntime {
                id: old,
                host_id: host,
                unit_name: "departing-provider",
                provider_id: world.provider,
                driver: "test",
                template_id: template,
                spec_hash: hash.as_bytes(),
                generation: 1,
                volumes: &json!({}),
            },
            world.clock.now(),
        )
        .await?;
        assert!(hosts::adopt_machine(&mut tx, world.machine_id, old, world.clock.now()).await?);
        let recipe = crate::updates::Recipe {
            volume_carry: None,
            provider_home_storage: None,
            data_sources: None,
            capability: None,
            capability_reason: None,
            capability_grace_secs: None,
            preserve_files: false,
            keep_estimate: None,
            target: crate::updates::Target::Provider(world.provider),
            old,
            by: actor,
            reason: orch_api_contract::environment::UpdatePreflightReason::Recreate,
            template,
            definition,
            network_policy: Default::default(),
            resolved_resources: json!({}),
            tools: vec![],
            hash: hash.as_bytes().to_vec(),
            options: crate::updates::options(
                &mut tx,
                crate::updates::Target::Provider(world.provider).place(),
                None,
            )
            .await?,
            threads: vec![],
            was_stopped: false,
            old_facts: json!({}),
            auto_start: false,
            host: Some(target),
        };
        let child = jobs::recreate::queue(
            &mut tx,
            crate::updates::Draft {
                recipe,
                updates: vec![],
                kept: vec![],
                lost: vec![],
                blockers: vec![],
            },
            None,
            world.clock.now(),
        )
        .await?;
        let recipe = serde_json::from_value(child.plan.clone())?;
        (
            child,
            ProviderAction::Replace {
                recipe: Box::new(recipe),
                host: target,
            },
        )
    } else {
        let state = providers::state(&mut tx, world.provider)
            .await?
            .ok_or("provider absent")?;
        let recipe = crate::provider_recovery::Recipe {
            provider: world.provider,
            revision: state.revision,
            by: actor,
            token: Some(key),
            host: target,
            template,
            definition,
        };
        let child = crate::provider_recovery::queue(
            &world.engine(),
            &mut tx,
            &principal,
            &recipe,
            world.clock.now(),
        )
        .await?;
        (
            child,
            ProviderAction::Make {
                recipe: Box::new(recipe),
            },
        )
    };
    let recipe = jobs::host_delete::Recipe {
        host,
        machine: source_machine.id,
        by: actor,
        token: Some(key),
        schedulable: true,
        children: vec![child.id],
        delete_volumes: vec![],
        admin_override: false,
        custody: None,
    };
    let parent = rows::insert(
        &mut tx,
        &rows::NewJob {
            id: new_id(),
            kind: jobs::host_delete::KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            plan: &json!({"cascade":recipe}),
            progress: &serde_json::to_value(jobs::runner::initial_progress(
                &jobs::host_delete::PHASES,
            ))?,
            created_by: Some(actor),
        },
        world.clock.now(),
    )
    .await?;
    let origin = jobs::host_delete::authority::Origin::from_http(http)?;
    jobs::host_delete::authority::retain(&mut tx, &parent, &origin, &caller, world.clock.as_ref())
        .await?;
    let result = world
        .engine()
        .retain_host_delete_provider_child(&mut tx, &parent, child.id, &reviewed)
        .await;
    if !provider_scope || !provider_manager {
        assert!(
            matches!(result, Err(EngineError::BeforeEffect(_))),
            "an independent Provider child needs current Provider management: {result:?}"
        );
        tx.rollback().await?;
        return Ok((world, child.id, key));
    }
    result?;
    let row = rows::job(&mut tx, child.id).await?.ok_or("child absent")?;
    let check = environment::current(
        &mut tx,
        &row,
        world.clock.as_ref(),
        world.access.as_ref(),
        Some(target),
    )
    .await?;
    assert_eq!(check.principal.user(), actor);
    assert_eq!(
        check.principal.caller().admitted_operation(),
        Some("HostDeleteCreate")
    );
    assert!(check.principal.caller().admitted_required_roles().is_some());
    assert!(
        check
            .principal
            .caller()
            .admitted_facets()
            .ok_or("child facts absent")?
            .iter()
            .all(|facet| facet.action != orch_access_contract::key_scope::KeyAction::Read)
    );
    assert_eq!(row.provider_id, Some(world.provider));
    assert_eq!(row.environment_id, None);
    let encoded = serde_json::to_value(read_native(&row)?)?;
    assert_eq!(encoded["kind"], json!("provider_parent"));
    let mut changed = row.clone();
    changed.provider_id = Some(new_id());
    assert!(
        environment::current(
            &mut tx,
            &changed,
            world.clock.as_ref(),
            world.access.as_ref(),
            None
        )
        .await
        .is_err()
    );
    check.check(world.clock.as_ref())?;
    tx.commit().await?;
    assert_eq!(
        world.commands.issued_of("node.runtime.start"),
        0,
        "source capture performs no native IO"
    );
    Ok((world, child.id, key))
}

fn read_native(row: &rows::JobRow) -> TestResult<environment::Evidence> {
    Ok(serde_json::from_value(
        row.plan["environment_lifecycle"]["native"].clone(),
    )?)
}

#[tokio::test]
async fn host_provider_replace_and_make_require_both_key_management_and_ordinary_provider_management()
-> TestResult {
    for replace in [false, true] {
        for (scope, manager) in [(false, true), (true, false)] {
            let (world, child, _) = queued(replace, scope, manager).await?;
            let mut conn = world.pool.acquire().await?;
            assert!(
                rows::job(&mut conn, child).await?.is_none(),
                "refused child queue rolls back"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn host_provider_children_keep_their_real_identity_and_recheck_the_original_key() -> TestResult
{
    for replace in [false, true] {
        let (world, child, key) = queued(replace, true, true).await?;
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(key)
            .bind(world.clock.now())
            .execute(world.pool.pool())
            .await?;
        let mut tx = world.pool.begin().await?;
        let row = rows::job(&mut tx, child).await?.ok_or("child absent")?;
        assert!(
            environment::current(
                &mut tx,
                &row,
                world.clock.as_ref(),
                world.access.as_ref(),
                None
            )
            .await
            .is_err()
        );
        tx.rollback().await?;
    }
    Ok(())
}

#[tokio::test]
async fn host_provider_leave_stays_on_the_host_without_a_provider_child() -> TestResult {
    let fixture = Fixture::new().await?;
    let row = fixture.job.row().await?;
    let footprint = jobs::host_delete::reviewed::Footprint::of(
        &[],
        &[ProviderAction::Leave {
            provider: fixture.world.provider,
            runtime: new_id(),
        }],
        None,
    );
    assert!(footprint.managed.is_empty());
    assert!(footprint.used.is_empty());
    let proposal = fixture.proposal().await?;
    assert!(
        proposal.authority.is_some(),
        "the Host-only key still controls its own physical retirement"
    );
    assert!(row.provider_id.is_none());
    Ok(())
}
