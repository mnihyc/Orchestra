//! Real creation choices keep scope before their existing UUID windows and formatting.

use super::*;
use orch_api_core::testing::RawReply;
use orch_types::Json;
use thread_history_scope::{Proof, key, narrow, rule};

const OP: &str = "NodeBindingOptionsGet";
fn rules(s: &Scene) -> Vec<Json> {
    vec![
        rule(OP, "topic", s.tree.topic.base.id, "self", "read"),
        rule(OP, "environment", s.on.environment, "self", "read"),
    ]
}
fn query(s: &Scene) -> NodeBindingOptionsGetQuery {
    NodeBindingOptionsGetQuery {
        environment_id: Some(s.on.environment),
        ..Default::default()
    }
}
async fn get(s: &Scene, secret: &str, query: &NodeBindingOptionsGetQuery) -> RawReply {
    let request = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(secret)
        .query(query);
    request.send_bytes().await
}
fn shown(reply: &RawReply) -> Proof<BindingOptions> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_choices_require_their_exact_root_context_and_native_door_then_select_only_owned_read_namespaces()
-> Proof {
    let s = Scene::new().await;
    let (_, selected_provider) =
        another_provider(&s, "Selected scoped provider", &["separated"]).await;
    let (_, other_provider) =
        another_provider(&s, "Another provider namespace", &["separated"]).await;
    let mut grants = rules(&s);
    grants.push(rule(OP, "provider", selected_provider, "self", "read"));
    let selected = key(&s.rig.world, s.ada.id(), grants).await?;
    let actual = shown(&get(&s, &selected.secret, &query(&s)).await)?;
    assert_eq!(actual.providers.len(), 1);
    assert_eq!(actual.providers[0].provider.id, selected_provider);
    assert!(actual.providers[0].agents.is_empty());
    assert_eq!(actual.environments.len(), 1);
    assert_eq!(actual.environments[0].environment.id, s.on.environment);
    assert!(actual.hosts.is_empty());
    assert!(actual.machines.is_empty());
    assert!(
        actual
            .providers
            .iter()
            .all(|row| row.provider.id != other_provider)
    );
    for grants in [
        vec![rule(OP, "environment", s.on.environment, "self", "read")],
        vec![rule(OP, "topic", s.tree.topic.base.id, "self", "read")],
        vec![
            rule(
                "ThreadBindingOptionsGet",
                "topic",
                s.tree.topic.base.id,
                "self",
                "read",
            ),
            rule(OP, "environment", s.on.environment, "self", "read"),
        ],
        vec![
            rule(OP, "topic", s.tree.topic.base.id, "self", "manage"),
            rule(OP, "environment", s.on.environment, "self", "read"),
        ],
    ] {
        let bad = key(&s.rig.world, s.ada.id(), grants).await?;
        assert!(!get(&s, &bad.secret, &query(&s)).await.status.is_success());
    }
    let native = key(&s.rig.world, s.cai.id(), rules(&s)).await?;
    assert!(
        !get(&s, &native.secret, &query(&s))
            .await
            .status
            .is_success()
    );
    let empty = key(&s.rig.world, s.ada.id(), rules(&s)).await?;
    let empty = shown(&get(&s, &empty.secret, &query(&s)).await)?;
    assert!(empty.providers.is_empty());
    assert_eq!(empty.environments.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_environment_context_reads_the_actual_template_and_never_an_invented_nil_environment()
-> Proof {
    let s = Scene::new().await;
    let template = orch_settings::get::<Id>(
        &mut *s.rig.world.conn().await,
        orch_settings::Place::Node(s.tree.topic.base.id),
        orch_settings_contract::registry::keys::ENV_TEMPLATE_ID,
    )
    .await?
    .unwrap_or(orch_db::seeds::DEFAULT_TEMPLATE_ID);
    let new = NodeBindingOptionsGetQuery {
        environment_new: true,
        ..Default::default()
    };
    let bad = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(OP, "topic", s.tree.topic.base.id, "self", "read"),
            rule(OP, "environment", s.on.environment, "self", "read"),
        ],
    )
    .await?;
    assert!(!get(&s, &bad.secret, &new).await.status.is_success());
    let actual = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(OP, "topic", s.tree.topic.base.id, "self", "read"),
            rule(OP, "template", template, "self", "read"),
        ],
    )
    .await?;
    let actual = shown(&get(&s, &actual.secret, &new).await)?;
    assert!(actual.providers.is_empty());
    assert!(actual.environments.is_empty());
    assert!(actual.machines.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scope_refills_the_native_choice_window_instead_of_losing_an_allowed_provider_after_its_bound()
-> Proof {
    let s = Scene::new().await;
    // One beyond the actual native cap, not a new production scan budget. These are
    // the same native owned provider/profile rows as the ordinary option fixtures.
    let mut last = s.on.provider;
    for index in 0..=orch_api_core::routes::resources::LIST_LIMIT {
        last = another_provider(
            &s,
            &format!("Native window provider {index}"),
            &["separated"],
        )
        .await
        .1;
    }
    let ordinary = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .query(&query(&s))
        .send()
        .await
        .ok();
    assert!(!ordinary.providers.iter().any(|row| row.provider.id == last));
    let mut grants = rules(&s);
    grants.push(rule(OP, "provider", last, "self", "read"));
    let selected = key(&s.rig.world, s.ada.id(), grants).await?;
    let actual = shown(&get(&s, &selected.secret, &query(&s)).await)?;
    assert_eq!(actual.providers.len(), 1);
    assert_eq!(actual.providers[0].provider.id, last);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_choices_keep_the_original_provider_source_and_current_read_through_the_actual_activity_wait()
-> Proof {
    for mode in ["unchanged", "scope", "expire", "association"] {
        let s = Scene::new().await;
        let (_, provider) = another_provider(&s, "Retained scoped choice", &["separated"]).await;
        let mut grants = rules(&s);
        grants.push(rule(OP, "provider", provider, "self", "read"));
        let selected = key(&s.rig.world, s.ada.id(), grants).await?;
        assert_eq!(
            shown(&get(&s, &selected.secret, &query(&s)).await)?
                .providers
                .len(),
            1
        );
        let mut blocker = s.rig.db.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.rig.world.app.clone();
        let topic = s.tree.topic.base.id;
        let request_query = query(&s);
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(NodeBindingOptionsGet, &[&topic])
                .token(&secret)
                .query(&request_query);
            request.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(!pending.is_finished(), "the actual final activity wait must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        match mode {
            "scope" => narrow(&s.rig.world, &selected, rules(&s)).await?,
            "expire" => s.rig.world.advance(chrono::Duration::hours(2)),
            "association" => {
                sqlx::query("UPDATE providers SET key_machine_id=NULL,machine_id=NULL WHERE id=$1")
                    .bind(provider)
                    .execute(s.rig.db.pool())
                    .await?;
            }
            _ => {}
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "unchanged" {
            assert_eq!(shown(&reply)?.providers.len(), 1);
        } else {
            assert!(
                !reply.status.is_success(),
                "{mode} cannot lend a fresh candidate to the old body"
            );
        }
    }
    Ok(())
}
