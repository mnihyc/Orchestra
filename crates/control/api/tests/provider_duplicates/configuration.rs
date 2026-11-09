//! Copied configuration uses the same native source/model writers and fresh destination limits.
use super::*;
use orch_api_contract::resources::{
    ModelParameters, NewProviderSources, ProviderModelCreate, ProviderModelCreateBody,
    ProviderModelUpdate, ProviderModelUpdateBody, ProviderUpdate, ProviderUpdateBody,
    SourceChoiceSet,
};
use orch_provider_contract::{AddressAuth, SourceAddress, SourceBy};
const FIELD_SECRET: &str = "synthetic-copied-source-field";
const ADDRESS_SECRET: &str = "synthetic-copied-source-authorization";
#[tokio::test]
async fn copied_source_choices_reseal_an_owners_fields_but_drop_cached_observations_and_prior_key_sending_consent()
-> Proof {
    let s = Scene::with_sources(Some(NewProviderSources {
        facts: Some(SourceChoiceSet {
            by: SourceBy::Address,
            name: None,
            address: Some(SourceAddress {
                url: "https://source.example.test/report".into(),
                auth: AddressAuth::Bearer,
                header: None,
                send_credential: true,
            }),
            config: [("region".into(), json!("east"))].into(),
            secrets: [("tenant".into(), Some(FIELD_SECRET.into()))].into(),
            authorization: Some(Some(ADDRESS_SECRET.into())),
        }),
        models: None,
        presenter: None,
    }))
    .await?;
    let mut old: Json = sqlx::query_scalar("SELECT sources FROM providers WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        "original-observed-source",
        "Original observed source",
        "A historical observation of the source program.",
    );
    descriptor.facts = Some(Default::default());
    old["facts"]["descriptor"] = json!(descriptor);
    old["facts"]["described_at"] = json!(orch_types::Time::new(s.world.now()));
    old["facts"]["last_error"] = json!({"code":"unavailable","message":"original observation only","at":orch_types::Time::new(s.world.now())});
    sqlx::query("UPDATE providers SET sources=$2,facts=$3,facts_overrides=$4 WHERE id=$1")
        .bind(s.source)
        .bind(&old)
        .bind(json!({"spend":{"value":"historical-spend"}}))
        .bind(json!({"balances":[{"id":"historical-balance","remaining":"99"}]}))
        .execute(&s.world.db.pool)
        .await?;
    resources::name_person(
        &s.world,
        "provider",
        s.source,
        "provider.view",
        s.company.admin.id(),
    )
    .await;
    for (actor, owner) in [(&s.company.boss, true), (&s.company.admin, false)] {
        let reply = s.copy(&actor.token, &DuplicateBody::default()).await;
        let made = id(&reply)?;
        let receipt: Json = serde_json::from_slice(&reply.raw)?;
        let expected = if owner {
            json!(["source_consent_reset"])
        } else {
            json!(["credentials_not_copied", "source_consent_reset"])
        };
        assert_eq!(receipt["setup"], expected);
        let (copied, facts, overrides): (Json, Json, Json) =
            sqlx::query_as("SELECT sources,facts,facts_overrides FROM providers WHERE id=$1")
                .bind(made)
                .fetch_one(&s.world.db.pool)
                .await?;
        let choice = &copied["facts"];
        assert_eq!(choice["by"], "address");
        assert_eq!(
            choice["address"]["url"],
            "https://source.example.test/report"
        );
        assert_eq!(
            choice["address"]["send_credential"], false,
            "an old manager's permission to send a key is never new consent"
        );
        assert_eq!(choice["config"], json!({"region":"east"}));
        assert!(choice.get("descriptor").is_none_or(Json::is_null));
        assert!(choice.get("described_at").is_none_or(Json::is_null));
        assert!(choice.get("last_error").is_none_or(Json::is_null));
        assert!(!facts.to_string().contains("historical-spend"));
        assert!(!overrides.to_string().contains("historical-balance"));
        for (name, plain) in [
            ("source:facts:field:tenant", FIELD_SECRET),
            ("source:facts:authorization", ADDRESS_SECRET),
        ] {
            let value = s.secret(made, name).await?;
            if owner {
                let value = value.ok_or("the owner's declared source secret was not copied")?;
                assert_ne!(
                    value,
                    s.secret(s.source, name)
                        .await?
                        .ok_or("source secret missing")?
                );
                assert_eq!(
                    MasterKey::new([7; 32]).open(&value, &binding("provider", made, name))?,
                    plain.as_bytes()
                );
            } else {
                assert!(
                    value.is_none(),
                    "a nonowner copy must ask for its own source credentials"
                );
            }
        }
        assert!(!String::from_utf8_lossy(&reply.raw).contains(FIELD_SECRET));
        assert!(!String::from_utf8_lossy(&reply.raw).contains(ADDRESS_SECRET));
    }
    Ok(())
}
async fn add(s: &Scene, name: &str, enabled: bool) -> Proof<Id> {
    let model = s
        .world
        .app
        .call(ProviderModelCreate, &[&s.source])
        .token(&s.company.boss.token)
        .body(&ProviderModelCreateBody {
            model_name: name.into(),
            display_name: Some(format!("Named {name}")),
            parameters: Some(ModelParameters {
                context_window: Some(8192),
                context_max: Some(16384),
                default_context: Some(8192),
                ..Default::default()
            }),
            groups: Some(vec!["fast".into(), "reasoning".into()]),
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native model source missing")?
        .id;
    if !enabled {
        s.world
            .app
            .call(ProviderModelUpdate, &[&s.source, &model])
            .token(&s.company.boss.token)
            .body(&ProviderModelUpdateBody {
                enabled: Some(false),
                ..Default::default()
            })
            .send()
            .await
            .ok();
    }
    Ok(model)
}
#[tokio::test]
async fn copied_model_definitions_keep_configuration_and_disabled_state_but_obey_the_current_native_group_limit()
-> Proof {
    let s = Scene::new().await?;
    let one = add(&s, "first-model", true).await?;
    let two = add(&s, "disabled-model", false).await?;
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    let groups = json!({"fast":{"request_options":{"native":{"type":"object","properties":{"mode":{"type":"string"}}}},"future_marker":"readable group configuration"}});
    let update: ProviderUpdateBody =
        serde_json::from_value(json!({"expected_revision":revision,"model_group_options":groups}))?;
    s.world
        .app
        .call(ProviderUpdate, &[&s.source])
        .token(&s.company.boss.token)
        .body(&update)
        .send()
        .await
        .ok();
    let made = id(&s
        .copy(&s.company.boss.token, &DuplicateBody::default())
        .await)?;
    let mut conn = s.world.conn().await;
    let original = orch_db::queries::resources::routes::models_of(&mut conn, &[s.source]).await?;
    let copied = orch_db::queries::resources::routes::models_of(&mut conn, &[made]).await?;
    assert_eq!(copied.len(), 2);
    for old in &original {
        let new = copied
            .iter()
            .find(|new| new.model_name == old.model_name)
            .ok_or("a copied model definition disappeared")?;
        assert_ne!(old.id, new.id);
        assert!(![one, two].contains(&new.id));
        assert_eq!(new.provider_id, made);
        assert_eq!(
            (
                &new.model_name,
                &new.display_name,
                &new.parameters,
                &new.groups,
                new.enabled
            ),
            (
                &old.model_name,
                &old.display_name,
                &old.parameters,
                &old.groups,
                old.enabled
            )
        );
        assert_eq!(
            new.source, "manual",
            "the new entries do not adopt the old discovery identity"
        );
    }
    let actual: Json = sqlx::query_scalar("SELECT model_group_options FROM providers WHERE id=$1")
        .bind(made)
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(actual, groups);
    drop(conn);
    s.world
        .set_at(
            orch_settings::Target::Node(s.company.acme),
            "providers.model_groups_max",
            json!(1),
        )
        .await;
    let denied = s
        .copy(&s.company.boss.token, &DuplicateBody::default())
        .await;
    assert!(
        !denied.status.is_success(),
        "copy must not grandfather model groups past current native creation policy"
    );
    assert_eq!(
        s.count().await?,
        2,
        "a native configuration refusal rolls back the provider and its initial Make"
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM provider_models WHERE provider_id IN (SELECT id FROM providers WHERE org_id=$1)").bind(s.company.acme).fetch_one(&s.world.db.pool).await?;
    assert_eq!(count, 4);
    Ok(())
}

#[tokio::test]
async fn provider_copy_rechecks_the_real_source_scope_after_its_new_models_wait_to_insert() -> Proof
{
    if std::env::var_os("ORCH_TEST_PROVIDER_COPY_ERRORS").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("orch_api::native_operations=debug,orch_engine::jobs::provider_provision::creator_authority=debug,orch_api_core::middleware::requests=error,orch_api_base::middleware::requests=error")
            .with_test_writer()
            .try_init();
    }
    for change in ["live", "source_scope", "ownership"] {
        let s = Scene::new().await?;
        add(&s, "waiting-model", true).await?;
        resources::name_person(
            &s.world,
            "provider",
            s.source,
            "provider.view",
            s.company.boss.id(),
        )
        .await;
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule("ProviderDuplicate", "provider", s.source, "self", "read"),
                rule("ProviderDuplicate", "org", s.company.acme, "self", "write"),
            ],
        )
        .await?;
        let mut blocker = s.world.db.pool.begin().await?;
        sqlx::query("LOCK TABLE provider_models IN SHARE MODE")
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.world.app.clone();
        let source = s.source;
        let secret = token.secret.clone();
        let task = tokio::spawn(async move {
            let request = app
                .call(ProviderDuplicate, &[&source])
                .token(&secret)
                .body(&DuplicateBody::default());
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!task.is_finished());
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if waiting {break;}tokio::task::yield_now().await;
            }
        }).await?;
        if change == "source_scope" {
            thread_history_scope::narrow(
                &s.world,
                &token,
                vec![rule(
                    "ProviderDuplicate",
                    "org",
                    s.company.acme,
                    "self",
                    "write",
                )],
            )
            .await?;
        }
        if change == "ownership" {
            resources::name_person(
                &s.world,
                "provider",
                s.source,
                "access.own",
                s.company.admin.id(),
            )
            .await;
            sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_kind='provider' AND object_id=$1 AND permission='access.own' AND holder_id=$2 AND removed_at IS NULL")
                .bind(s.source).bind(s.company.boss.id()).bind(s.world.now()).execute(&s.world.db.pool).await?;
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), task).await??;
        if change != "live" {
            assert_eq!(
                reply.status.as_u16(),
                403,
                "{change}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(s.count().await?, 1);
        } else {
            id(&reply)?;
            assert_eq!(s.count().await?, 2);
        }
    }
    Ok(())
}

#[tokio::test]
async fn runtime_configuration_is_copied_through_native_validation_and_its_declared_secret_values_remain_owner_only()
-> Proof {
    use orch_api_contract::resources::{ProviderSecretSet, ProviderSecretSetBody};
    let s = Scene::new().await?;
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    let runtime = json!({"runtime.variables":{"COPY_MODE":"native"},"runtime.idle_stop_minutes":5,"runtime.secret_names":["COPY_TOKEN"]});
    let body: ProviderUpdateBody =
        serde_json::from_value(json!({"expected_revision":revision,"runtime_config":runtime}))?;
    s.world
        .app
        .call(ProviderUpdate, &[&s.source])
        .token(&s.company.boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    s.world
        .app
        .call(ProviderSecretSet, &[&s.source, &"runtime:COPY_TOKEN"])
        .token(&s.company.boss.token)
        .body(&ProviderSecretSetBody {
            value: FIELD_SECRET.into(),
        })
        .send()
        .await
        .ok();
    resources::name_person(
        &s.world,
        "provider",
        s.source,
        "provider.view",
        s.company.admin.id(),
    )
    .await;
    for (actor, owner) in [(&s.company.boss, true), (&s.company.admin, false)] {
        let made = id(&s.copy(&actor.token, &DuplicateBody::default()).await)?;
        let config: Json = sqlx::query_scalar("SELECT runtime_config FROM providers WHERE id=$1")
            .bind(made)
            .fetch_one(&s.world.db.pool)
            .await?;
        assert_eq!(config, runtime);
        let secret = s.secret(made, "runtime:COPY_TOKEN").await?;
        if owner {
            let secret = secret.ok_or("declared runtime secret missing")?;
            assert_ne!(
                secret,
                s.secret(s.source, "runtime:COPY_TOKEN")
                    .await?
                    .ok_or("source runtime secret missing")?
            );
            assert_eq!(
                MasterKey::new([7; 32])
                    .open(&secret, &binding("provider", made, "runtime:COPY_TOKEN"))?,
                FIELD_SECRET.as_bytes()
            );
        } else {
            assert!(secret.is_none());
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_copied_program_definition_cannot_claim_registration_or_reuse_the_original_keeper()
-> Proof {
    let sources = NewProviderSources {
        facts: Some(SourceChoiceSet {
            by: SourceBy::Program,
            name: Some("copy-plan".into()),
            ..Default::default()
        }),
        models: None,
        presenter: None,
    };
    let s = Scene::with_location(Some(sources.clone()), true).await?;
    let (old_machine, old_keeper): (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT machine_id,key_machine_id FROM providers WHERE id=$1")
            .bind(s.source)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert!(old_machine.is_some());
    assert_eq!(old_machine, old_keeper);
    let made = id(&s
        .copy(&s.company.boss.token, &DuplicateBody::default())
        .await)?;
    let (machine, keeper, definitions): (Option<Id>, Option<Id>, Json) =
        sqlx::query_as("SELECT machine_id,key_machine_id,sources FROM providers WHERE id=$1")
            .bind(made)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert_eq!((machine, keeper), (None, None));
    assert_eq!(definitions["facts"]["by"], "program");
    assert_eq!(definitions["facts"]["name"], "copy-plan");
    assert!(
        definitions["facts"]
            .get("descriptor")
            .is_none_or(Json::is_null),
        "a stored definition is never the old keeper's executable/program registration"
    );
    let ordinary = s
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.company.boss.token)
        .body(&CreateProvider::Api {
            name: "An unregistered ordinary program is still refused".into(),
            home_id: s.company.acme,
            upstreams: vec![Upstream {
                protocol: "openai-responses".into(),
                base_url: "https://api.example.test/v1".into(),
                options: json!({}),
            }],
            sources: Some(sources),
            runtime_machine_id: None,
            key_machine_id: None,
            access: None,
            arrangements: None,
            runtime_config: None,
            api_key: None,
            secrets: None,
        })
        .send_bytes()
        .await;
    assert!(!ordinary.status.is_success());
    assert_eq!(s.count().await?, 2);
    Ok(())
}
