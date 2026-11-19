//! Actual mixed fleet subjects, unchanged native calculations and per-entry source witnesses.

use super::*;
use orch_api_contract::environment::{NodeEnvironmentListQuery, NodeEnvironmentListReply};

const OPERATION: &str = "NodeEnvironmentList";

fn environment(s: &Scene) -> Json {
    rule(
        OPERATION,
        "environment",
        s.first.base.id,
        "self",
        &["environment"],
        &["read"],
    )
}
fn host(s: &Scene) -> Json {
    rule(OPERATION, "host", s.team.host, "self", &["host"], &["read"])
}

async fn call(s: &Scene, token: &str, query: &NodeEnvironmentListQuery) -> RawReply {
    s.world
        .app
        .call(NodeEnvironmentList, &[&s.team.topic])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}

async fn shown(
    s: &Scene,
    token: &str,
    query: &NodeEnvironmentListQuery,
) -> NodeEnvironmentListReply {
    let reply = call(s, token, query).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn contexts(s: &Scene) -> Json {
    rule(
        OPERATION,
        "org",
        s.team.c.acme,
        "subtree",
        &[
            "host", "template", "provider", "org", "project", "topic", "thread",
        ],
        &["read"],
    )
}

async fn activity_wait(s: &Scene, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!request.is_finished(),"the actual fleet body must reach activity publication");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual native fleet activity INSERT wait");
}

#[tokio::test]
async fn fleet_scope_keeps_native_bodies_queries_and_independently_selected_host_aggregates() {
    let s = Scene::new().await;
    let native = shown(&s, &s.team.c.admin.token, &Default::default()).await;
    assert_eq!(native.entries.len(), 2);
    assert_eq!(native.hosts.len(), 1);
    let all = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            environment(&s),
            rule(
                OPERATION,
                "environment",
                s.second.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
            contexts(&s),
        ],
    )
    .await;
    assert_eq!(shown(&s, &all.secret, &Default::default()).await, native);
    let only_host = key(&s.world, s.team.c.admin.id(), vec![host(&s)]).await;
    let selected = shown(&s, &only_host.secret, &Default::default()).await;
    assert!(
        selected.entries.is_empty(),
        "a Host grant lends no Environment rows"
    );
    assert_eq!(
        selected.hosts, native.hosts,
        "native Host aggregate counts keep their actual Host governor"
    );

    let only_environment = key(&s.world, s.team.c.admin.id(), vec![environment(&s)]).await;
    let selected = shown(&s, &only_environment.secret, &Default::default()).await;
    assert_eq!(selected.entries.len(), 1);
    assert_eq!(selected.entries[0].environment.id, s.first.base.id);
    assert!(selected.hosts.is_empty());
    assert!(selected.entries[0].host_name.is_none());
    assert!(selected.entries[0].template.is_none());
    assert!(selected.entries[0].home.as_ref().unwrap().name.is_none());
    assert!(selected.entries[0].manageable);
    let query = NodeEnvironmentListQuery {
        status: Some(EnvironmentStatus::Offline),
        ..Default::default()
    };
    let filtered = shown(&s, &all.secret, &query).await;
    assert!(filtered.entries.is_empty());
    assert_eq!(
        filtered.hosts, native.hosts,
        "the original query filters Environment entries, not managed Hosts"
    );
}

#[tokio::test]
async fn fleet_scope_requires_its_own_action_actual_subject_kind_and_native_node_view() {
    let s = Scene::new().await;
    let reader = key(&s.world, s.team.c.admin.id(), vec![environment(&s)]).await;
    assert_eq!(
        shown(&s, &reader.secret, &Default::default())
            .await
            .entries
            .len(),
        1
    );
    for policy in [
        vec![rule(
            "EnvironmentList",
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        )],
        vec![rule(
            OPERATION,
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["manage"],
        )],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), policy).await;
        assert_eq!(
            call(&s, &denied.secret, &Default::default()).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let wrong = key(
        &s.world,
        s.team.c.admin.id(),
        vec![rule(
            OPERATION,
            "topic",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        )],
    )
    .await;
    let selected = shown(&s, &wrong.secret, &Default::default()).await;
    assert!(
        selected.entries.is_empty() && selected.hosts.is_empty(),
        "a same-id different-kind selector grants no subject"
    );
    let stranger = key(&s.world, s.team.c.stranger.id(), vec![environment(&s)]).await;
    assert_eq!(
        call(&s, &stranger.secret, &Default::default()).await.status,
        StatusCode::FORBIDDEN,
        "the ordinary native node.view door is not lent by a resource scope"
    );
}

#[tokio::test]
async fn fleet_publication_does_not_keep_manager_fields_after_only_native_manage_is_withdrawn() {
    for withdraw in [false, true] {
        let s = Scene::new().await;
        let entry = resources::name_person(
            &s.world,
            "environment",
            s.first.base.id,
            "environment.manage",
            s.team.c.viewer.id(),
        )
        .await;
        let reader = key(&s.world, s.team.c.viewer.id(), vec![environment(&s)]).await;
        assert!(shown(&s, &reader.secret, &Default::default()).await.entries[0].manageable);
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let node = s.team.topic;
        let secret = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(NodeEnvironmentList, &[&node]);
            call.token(&secret)
                .query(&NodeEnvironmentListQuery::default())
                .send_bytes()
                .await
        });
        activity_wait(&s, pid, &request).await;
        if withdraw {
            assert!(
                orch_db::queries::access::remove_entry(
                    &mut *s.world.conn().await,
                    entry,
                    s.team.c.admin.id(),
                    s.world.now()
                )
                .await
                .unwrap()
            );
            let current = shown(&s, &s.team.c.viewer.token, &Default::default()).await;
            assert!(
                !current
                    .entries
                    .iter()
                    .find(|row| row.environment.id == s.first.base.id)
                    .unwrap()
                    .manageable,
                "native View remains while native management metadata changes"
            );
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if withdraw {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
    }
}

#[tokio::test]
async fn fleet_publication_keeps_each_environment_template_association_even_when_the_union_still_matches()
 {
    for change in [false, true] {
        let s = Scene::new().await;
        let other = a_template(
            &s.world,
            &s.team.c.admin,
            s.team.c.acme,
            "Other fleet template",
        )
        .await;
        let reader = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                environment(&s),
                rule(
                    OPERATION,
                    "environment",
                    s.second.base.id,
                    "self",
                    &["environment"],
                    &["read"],
                ),
                contexts(&s),
            ],
        )
        .await;
        let before = shown(&s, &reader.secret, &Default::default()).await;
        assert_eq!(
            before
                .entries
                .iter()
                .find(|row| row.environment.id == s.first.base.id)
                .unwrap()
                .template
                .as_ref()
                .unwrap()
                .id,
            s.team.template
        );
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let node = s.team.topic;
        let secret = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(NodeEnvironmentList, &[&node]);
            call.token(&secret)
                .query(&NodeEnvironmentListQuery::default())
                .send_bytes()
                .await
        });
        activity_wait(&s, pid, &request).await;
        if change {
            sqlx::query("UPDATE environments SET template_id=$2 WHERE id=$1")
                .bind(s.first.base.id)
                .bind(other)
                .execute(&s.world.db.pool)
                .await
                .unwrap();
            let current = shown(&s, &s.team.c.admin.token, &Default::default()).await;
            assert_eq!(
                current
                    .entries
                    .iter()
                    .find(|row| row.environment.id == s.first.base.id)
                    .unwrap()
                    .template
                    .as_ref()
                    .unwrap()
                    .id,
                other
            );
            assert_eq!(
                current
                    .entries
                    .iter()
                    .find(|row| row.environment.id == s.second.base.id)
                    .unwrap()
                    .template
                    .as_ref()
                    .unwrap()
                    .id,
                s.team.template,
                "the old Template is still a selected readable source, but belongs to the other Environment"
            );
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if change {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
    }
}
