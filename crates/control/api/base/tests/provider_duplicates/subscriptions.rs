//! Native machine profiles retain their physical Use gates while each copied subscription gets a new sign-in home.
use super::*;
use orch_api_contract::resources::{MachineAgentCreate, MachineAgentCreateBody};
use orch_driver_contract::runtime::EnvironmentDataSource;
#[tokio::test]
async fn a_machine_bound_subscription_copy_keeps_native_profile_use_but_never_its_existing_login_home()
-> Proof {
    let s = Scene::new().await?;
    let machine = resources::machine_forwarding_to(
        &s.world,
        s.company.boss.id(),
        &["https://api.example.test/v1"],
    )
    .await;
    let policy = orch_link_contract::policy::LocalPolicy {
        agent_harnesses: vec![orch_link_contract::policy::PolicyHarness {
            kind: "fake".into(),
            path: "/test/fake".into(),
        }],
        environment_roots: vec!["/native".into()],
        ..Default::default()
    };
    let facts = json!({"adapters":[{"kind":"fake","adapter_version":1,"builtin":true,"default_home_owned":false}],"harnesses":[{"kind":"fake","path":"/test/fake","version":"1.0.0","daemon":false}],"policy":policy});
    sqlx::query("UPDATE machines SET roles=ARRAY['agent','provider'],facts=facts||$2 WHERE id=$1")
        .bind(machine)
        .bind(facts)
        .execute(&s.world.db.pool)
        .await?;
    let profile=s.world.app.call(MachineAgentCreate,&[&machine]).token(&s.company.boss.token).body(&MachineAgentCreateBody {
        home_id:s.company.acme,harness_kind:"fake".into(),name:"Existing native account profile".into(),profile:Some(json!({"config":{},"env":{},"attach":{"mode":"spawn","home":{"kind":"existing_directory","path":"/native/existing-account"}}})),
    }).send().await.ok().into_full().map_err(|_|"native profile source missing")?.base.id;
    let source = s
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.company.boss.token)
        .body(&CreateProvider::Subscription {
            name: "Native signed-in account".into(),
            home_id: s.company.acme,
            agent_id: profile,
            runtime_machine_id: Some(machine),
            key_machine_id: None,
            access: Some(NewResourceAccess::OnlyMe),
            arrangements: None,
            runtime_config: None,
            config: None,
            sign_in: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native subscription source missing")?
        .provider
        .base
        .id;
    sqlx::query("UPDATE providers SET login_state=$2,status='ready' WHERE id=$1")
        .bind(source)
        .bind(json!({"state":"completed","account":"synthetic source account"}))
        .execute(&s.world.db.pool)
        .await?;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("ProviderDuplicate", "provider", source, "self", "read"),
            rule("ProviderDuplicate", "org", s.company.acme, "self", "write"),
        ],
    )
    .await?;
    let copied = s
        .world
        .app
        .call(ProviderDuplicate, &[&source])
        .token(&token.secret)
        .body(&DuplicateBody::default())
        .send_bytes()
        .await;
    let made = id(&copied)?;
    type CopiedLogin = (
        String,
        Option<Id>,
        Option<Id>,
        Option<Json>,
        Option<Vec<u8>>,
        Option<Json>,
    );
    let row: CopiedLogin = sqlx::query_as("SELECT status,machine_id,key_machine_id,login_state,login_pending,credential_home FROM providers WHERE id=$1").bind(made).fetch_one(&s.world.db.pool).await?;
    assert_eq!(
        row,
        (
            "needs_setup".into(),
            None,
            Some(machine),
            None,
            None,
            Some(json!({"kind":"made_by_orch"}))
        )
    );
    let mut conn = s.world.conn().await;
    assert_eq!(
        orch_settings::credential_home_choice(&mut conn, source).await?,
        Some(EnvironmentDataSource::ExistingDirectory {
            path: "/native/existing-account".into()
        })
    );
    assert_eq!(
        orch_settings::credential_home_choice(&mut conn, made).await?,
        Some(EnvironmentDataSource::MadeByOrch)
    );
    assert_eq!(
        orch_settings::provider_home_choice(&mut conn, made).await?,
        Some(EnvironmentDataSource::MadeByOrch)
    );
    let original: Json = sqlx::query_scalar("SELECT login_state FROM providers WHERE id=$1")
        .bind(source)
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(original["state"], "completed");
    Ok(())
}

#[tokio::test]
async fn subscription_copy_honors_destination_home_locks_without_reusing_an_existing_sign_in_directory()
-> Proof {
    use orch_api_contract::resources::{OrgAgentCreate, OrgAgentCreateBody};
    use orch_db::audit::{Actor, Origin};
    use orch_settings::Target;
    use orch_settings::write::{Write, write};
    for borrowed in [false, true] {
        let s = Scene::new().await?;
        let profile = s
            .world
            .app
            .call(OrgAgentCreate, &[&s.company.acme])
            .token(&s.company.boss.token)
            .body(&OrgAgentCreateBody {
                name: "Copy home policy profile".into(),
                harness_kind: "fake".into(),
                profile: json!({}),
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "native profile source missing")?
            .base
            .id;
        let source = s
            .world
            .app
            .call(ProviderCreate, &[])
            .token(&s.company.boss.token)
            .body(&CreateProvider::Subscription {
                name: "Copy home policy account".into(),
                home_id: s.company.acme,
                agent_id: profile,
                runtime_machine_id: None,
                key_machine_id: None,
                access: None,
                arrangements: None,
                runtime_config: None,
                config: None,
                sign_in: None,
                sources: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "native account source missing")?
            .provider
            .base
            .id;
        let home = if borrowed {
            json!({"kind":"existing_directory","path":"/destination/existing-account"})
        } else {
            json!({"kind":"made_by_orch"})
        };
        let mut conn = s.world.conn().await;
        write(
            &mut conn,
            &Write {
                actor: Actor::System,
                target: Target::Node(s.company.acme),
                key: "runtime.home",
                value: home,
                locked: Some(true),
                expected_revision: None,
                may_lock: true,
                origin: &Origin::default(),
            },
            s.world.now(),
        )
        .await?;
        drop(conn);
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule("ProviderDuplicate", "provider", source, "self", "read"),
                rule("ProviderDuplicate", "org", s.company.acme, "self", "write"),
            ],
        )
        .await?;
        let reply = s
            .world
            .app
            .call(ProviderDuplicate, &[&source])
            .token(&token.secret)
            .body(&DuplicateBody::default())
            .send_bytes()
            .await;
        if borrowed {
            assert!(
                !reply.status.is_success(),
                "a copy may not bypass destination locks or silently choose the existing account directory"
            );
            assert_eq!(s.count().await?, 2);
        } else {
            let made = id(&reply)?;
            assert_eq!(s.count().await?, 3);
            assert_eq!(
                orch_settings::provider_home_choice(&mut *s.world.conn().await, made).await?,
                Some(EnvironmentDataSource::MadeByOrch)
            );
            assert_eq!(
                orch_settings::credential_home_choice(&mut *s.world.conn().await, made).await?,
                Some(EnvironmentDataSource::MadeByOrch)
            );
        }
    }
    Ok(())
}
