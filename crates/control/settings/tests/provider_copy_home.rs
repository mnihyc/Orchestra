//! A new unsigned subscription can select an isolated keeper home without redirecting the original profile.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod support;
use orch_db::queries::{
    resources::{provider_runtime, testing::insert_machine},
    settings::testing::insert_provider,
};
use orch_driver_contract::runtime::EnvironmentDataSource;
use orch_types::new_id;
use serde_json::json;
use support::World;
#[tokio::test]
async fn an_initial_owned_credential_home_is_independent_of_the_profiles_borrowed_login_and_the_runtime_machine()
 {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let keeper = insert_machine(&mut conn, "Native credential keeper", "connected")
        .await
        .unwrap();
    let runtime = insert_machine(&mut conn, "Independent runtime", "connected")
        .await
        .unwrap();
    let agent = new_id();
    sqlx::query("INSERT INTO agents(id,name,owner_id,org_id,home_id,harness_kind,machine_id,profile) VALUES($1,'Native borrowed profile',$2,$3,$3,'fake',$4,$5)")
        .bind(agent).bind(w.ada).bind(w.department).bind(keeper).bind(json!({"attach":{"home":{"kind":"existing_directory","path":"/native/existing-account"}}})).execute(&mut *conn).await.unwrap();
    let source = new_id();
    let copy = new_id();
    for id in [source, copy] {
        insert_provider(
            &mut conn,
            id,
            "Independent account",
            w.ada,
            w.department,
            &json!({"runtime.home":{"kind":"made_by_orch"}}),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE providers SET kind='subscription',harness_kind='fake',agent_id=$2,key_machine_id=$3 WHERE id=$1").bind(id).bind(agent).bind(keeper).execute(&mut *conn).await.unwrap();
    }
    sqlx::query("UPDATE providers SET login_state=$2 WHERE id=$1")
        .bind(source)
        .bind(json!({"state":"completed"}))
        .execute(&mut *conn)
        .await
        .unwrap();
    let before = orch_settings::credential_home_choice(&mut conn, source)
        .await
        .unwrap();
    assert_eq!(
        before,
        Some(EnvironmentDataSource::ExistingDirectory {
            path: "/native/existing-account".into()
        })
    );
    assert!(
        !provider_runtime::set_initial_owned_credential_home(&mut conn, source, w.ada)
            .await
            .unwrap(),
        "an already signed-in account cannot be relabeled as a fresh copy"
    );
    assert!(
        !provider_runtime::set_initial_owned_credential_home(&mut conn, copy, new_id())
            .await
            .unwrap(),
        "the actual new creator must match"
    );
    assert!(
        provider_runtime::set_initial_owned_credential_home(&mut conn, copy, w.ada)
            .await
            .unwrap()
    );
    for machine in [None, Some(runtime), Some(keeper)] {
        sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
            .bind(copy)
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            orch_settings::credential_home_choice(&mut conn, copy)
                .await
                .unwrap(),
            Some(EnvironmentDataSource::MadeByOrch)
        );
        assert_eq!(
            orch_settings::credential_home_choice(&mut conn, source)
                .await
                .unwrap(),
            before
        );
    }
    assert!(
        provider_runtime::credential_home(&mut conn, source)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        provider_runtime::credential_home(&mut conn, copy)
            .await
            .unwrap(),
        Some(json!({"kind":"made_by_orch"}))
    );
    assert!(
        !provider_runtime::set_initial_owned_credential_home(&mut conn, copy, w.ada)
            .await
            .unwrap()
    );
}
