//! The explicit home choice and the credential keeper's physical policy decide native sign-in permission.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::queries::settings::testing::insert_provider;
use orch_driver_contract::runtime::EnvironmentDataSource;
use orch_settings::{credential_home, provider_home, provider_sign_in};
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn an_unconfigured_harness_retains_its_default_and_permission_uses_the_adapters_home_fact() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let provider = new_id();
    insert_provider(
        &mut conn,
        provider,
        "Existing login",
        w.ada,
        w.department,
        &json!({}),
    )
    .await
    .unwrap();
    let keeper = insert_machine(&mut conn, "Keeper", "connected")
        .await
        .unwrap();
    let agent = new_id();
    sqlx::query("INSERT INTO agents(id,name,owner_id,org_id,home_id,harness_kind,profile) VALUES($1,'Native profile',$2,$3,$3,'fake','{}')")
        .bind(agent).bind(w.ada).bind(w.department).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE providers SET kind='subscription',harness_kind='fake',agent_id=$3,machine_id=$2,key_machine_id=$2 WHERE id=$1")
        .bind(provider).bind(keeper).bind(agent).execute(&mut *conn).await.unwrap();
    assert_eq!(
        orch_settings::provider_home_choice(&mut conn, provider)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        orch_settings::credential_home_choice(&mut conn, provider)
            .await
            .unwrap(),
        None
    );
    for owned in [false, true] {
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(keeper)
            .bind(json!({"adapters":[{"kind":"fake","default_home_owned":owned}],"policy":{"provider.signin":false}}))
            .execute(&mut *conn).await.unwrap();
        let support = provider_sign_in(&mut conn, provider).await.unwrap();
        assert_eq!((support.home_owned, support.allowed), (owned, owned));
    }
    sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1")
        .bind(provider)
        .bind(json!({"runtime.home":{"kind":"made_by_orch"}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        orch_settings::provider_home_choice(&mut conn, provider)
            .await
            .unwrap(),
        Some(EnvironmentDataSource::MadeByOrch)
    );
}
use support::World;

#[tokio::test]
async fn home_ownership_and_policy_are_independent_of_the_machine_or_providers_display_placement() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let provider = new_id();
    insert_provider(
        &mut conn,
        provider,
        "Account",
        w.ada,
        w.department,
        &json!({}),
    )
    .await
    .unwrap();
    let machine = insert_machine(&mut conn, "Keeper", "connected")
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET machine_id=$2,key_machine_id=$2 WHERE id=$1")
        .bind(provider)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    for placement in ["connected", "managed"] {
        for owned in [true, false] {
            for allowed in [true, false] {
                let source = if owned {
                    json!({"kind":"made_by_orch"})
                } else {
                    json!({"kind":"existing_directory","path":"/allowed/home"})
                };
                sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1")
                    .bind(provider)
                    .bind(json!({"runtime.home":source}))
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                sqlx::query("UPDATE machines SET placement=$3,facts=$2 WHERE id=$1")
                    .bind(machine)
                    .bind(json!({"policy":{"provider.signin":allowed}}))
                    .bind(placement)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                assert_eq!(
                    matches!(
                        provider_home(&mut conn, provider).await.unwrap(),
                        EnvironmentDataSource::MadeByOrch
                    ),
                    owned
                );
                let support = provider_sign_in(&mut conn, provider).await.unwrap();
                assert_eq!(support.home_owned, owned);
                assert_eq!(
                    support.allowed,
                    owned || allowed,
                    "{placement}, owned={owned}, policy={allowed}"
                );
                assert_eq!(support.policy_allowed, Some(allowed));
            }
        }
    }
}

#[tokio::test]
async fn a_units_empty_or_permissive_local_policy_never_replaces_its_physical_keepers_policy() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let provider = new_id();
    insert_provider(
        &mut conn,
        provider,
        "Account",
        w.ada,
        w.department,
        &json!({}),
    )
    .await
    .unwrap();
    let child = insert_machine(&mut conn, "Unit", "managed").await.unwrap();
    let physical = insert_machine(&mut conn, "Physical keeper", "connected")
        .await
        .unwrap();
    let host = new_id();
    sqlx::query("INSERT INTO hosts (id,machine_id,name) VALUES ($1,$2,'Keeper')")
        .bind(host)
        .bind(physical)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET host_id=$2,facts=$3 WHERE id=$1")
        .bind(child)
        .bind(host)
        .bind(json!({"policy":{"provider.signin":true}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(physical)
        .bind(json!({"policy":{"provider.signin":false}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE providers SET machine_id=$2,key_machine_id=$2,runtime_config=$3 WHERE id=$1",
    )
    .bind(provider)
    .bind(child)
    .bind(json!({"runtime.home":{"kind":"existing_directory","path":"/allowed/home"}}))
    .execute(&mut *conn)
    .await
    .unwrap();
    assert!(!provider_sign_in(&mut conn, provider).await.unwrap().allowed);
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(physical)
        .bind(json!({"policy":{"provider.signin":true}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(provider_sign_in(&mut conn, provider).await.unwrap().allowed);
}

#[tokio::test]
async fn a_separate_runtime_never_redirects_the_native_home_or_supplies_its_keepers_sign_in_permission()
 {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let provider = new_id();
    let agent = new_id();
    insert_provider(
        &mut conn,
        provider,
        "Split account",
        w.ada,
        w.department,
        &json!({}),
    )
    .await
    .unwrap();
    let keeper = insert_machine(&mut conn, "Native keeper", "connected")
        .await
        .unwrap();
    let runtime = insert_machine(&mut conn, "Independent runtime", "connected")
        .await
        .unwrap();
    sqlx::query("INSERT INTO agents(id,name,owner_id,org_id,home_id,harness_kind,machine_id,profile) VALUES($1,'Native profile',$2,$3,$3,'fake',$4,$5)")
        .bind(agent).bind(w.ada).bind(w.department).bind(keeper)
        .bind(json!({"attach":{"mode":"spawn","binary":null,"home":{"kind":"existing_directory","path":"/keeper/account"}}}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE providers SET kind='subscription',harness_kind='fake',agent_id=$2,machine_id=$3,key_machine_id=$4,runtime_config=$5 WHERE id=$1")
        .bind(provider).bind(agent).bind(runtime).bind(keeper).bind(json!({"runtime.home":{"kind":"made_by_orch"}}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(runtime)
        .bind(json!({"policy":{"provider.signin":true}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(keeper)
        .bind(json!({"policy":{"provider.signin":false}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        provider_home(&mut conn, provider).await.unwrap(),
        EnvironmentDataSource::MadeByOrch
    );
    assert_eq!(
        credential_home(&mut conn, provider).await.unwrap(),
        EnvironmentDataSource::ExistingDirectory {
            path: "/keeper/account".into()
        }
    );
    let support = provider_sign_in(&mut conn, provider).await.unwrap();
    assert!(!support.home_owned);
    assert_eq!(support.policy_allowed, Some(false));
    assert!(!support.allowed);
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(keeper)
        .bind(json!({"policy":{"provider.signin":true}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(provider_sign_in(&mut conn, provider).await.unwrap().allowed);
}
