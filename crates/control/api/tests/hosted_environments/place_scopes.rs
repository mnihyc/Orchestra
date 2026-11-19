//! The native home/Template inputs and heterogeneous selected placement choices.

use super::*;
use orch_api_contract::environment::{NodePlaceList, PlaceListQuery};

const OPERATION: &str = "NodePlaceList";

fn inputs(s: &Scene) -> Vec<Json> {
    vec![
        rule(
            OPERATION,
            "topic",
            s.team.topic,
            "self",
            &["topic"],
            &["read"],
        ),
        rule(
            OPERATION,
            "template",
            s.team.template,
            "self",
            &["template"],
            &["read"],
        ),
    ]
}

fn subject(kind: &str, id: Id) -> Json {
    rule(OPERATION, kind, id, "self", &[kind], &["read"])
}

fn query(s: &Scene) -> PlaceListQuery {
    PlaceListQuery {
        template_id: Some(s.team.template),
        ..Default::default()
    }
}

async fn call(s: &Scene, token: &str, query: &PlaceListQuery) -> RawReply {
    s.world
        .app
        .call(NodePlaceList, &[&s.team.topic])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}

async fn shown(s: &Scene, token: &str, query: &PlaceListQuery) -> Vec<Place> {
    let reply = call(s, token, query).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn places_scope_preserves_native_mixed_bodies_and_independent_choice_kinds() {
    let s = Scene::new().await;
    let larger = a_host(
        &s.world,
        &s.team.c.admin,
        s.team.c.acme,
        "Larger native choice",
        32_768,
    )
    .await;
    let query = query(&s);
    let native = shown(&s, &s.team.c.admin.token, &query).await;
    assert_eq!(
        native
            .iter()
            .filter(|row| matches!(row, Place::Host(_)))
            .count(),
        2
    );
    assert_eq!(
        native
            .iter()
            .filter(|row| matches!(row, Place::Environment(_)))
            .count(),
        2
    );
    let mut rules = inputs(&s);
    rules.extend([
        subject("environment", s.first.base.id),
        subject("environment", s.second.base.id),
        subject("host", s.team.host),
        subject("host", larger),
        s.sources(OPERATION),
    ]);
    let reader = key(&s.world, s.team.c.admin.id(), rules).await;
    assert_eq!(shown(&s, &reader.secret, &query).await, native);

    for (kind, id) in [("host", s.team.host), ("environment", s.first.base.id)] {
        let mut rules = inputs(&s);
        rules.push(subject(kind, id));
        let reader = key(&s.world, s.team.c.admin.id(), rules).await;
        let selected = shown(&s, &reader.secret, &query).await;
        assert_eq!(selected.len(), 1);
        match &selected[0] {
            Place::Host(host) => {
                assert_eq!(kind, "host");
                assert_eq!(host.host.id, id);
                assert!(
                    host.automatic,
                    "ranking runs over the actual scoped candidate set"
                );
            }
            Place::Environment(environment) => {
                assert_eq!(kind, "environment");
                assert_eq!(environment.environment.id, id);
                assert!(
                    environment.host.is_none(),
                    "Host content has independent same-operation Read"
                );
                assert!(environment.machine.is_none());
            }
        }
    }
    let reader = key(&s.world, s.team.c.admin.id(), inputs(&s)).await;
    assert!(
        shown(&s, &reader.secret, &query).await.is_empty(),
        "the real selector proves an empty choice collection while retaining real inputs"
    );
}

#[tokio::test]
async fn places_scope_requires_actual_input_facts_own_operation_action_and_native_interact() {
    let s = Scene::new().await;
    let query = query(&s);
    let mut full = inputs(&s);
    full.push(subject("host", s.team.host));
    let reader = key(&s.world, s.team.c.admin.id(), full.clone()).await;
    assert_eq!(shown(&s, &reader.secret, &query).await.len(), 1);
    for rules in [
        vec![full[0].clone(), full[2].clone()],
        vec![full[1].clone(), full[2].clone()],
        vec![rule(
            "EnvironmentList",
            "org",
            s.team.c.acme,
            "subtree",
            &["host", "template", "topic"],
            &["read"],
        )],
        vec![rule(
            OPERATION,
            "org",
            s.team.c.acme,
            "subtree",
            &["host", "template", "topic"],
            &["manage"],
        )],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), rules).await;
        assert_eq!(
            call(&s, &denied.secret, &query).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let mut wrong = inputs(&s);
    wrong.push(rule(
        OPERATION,
        "environment",
        s.team.host,
        "self",
        &["host"],
        &["read"],
    ));
    let reader = key(&s.world, s.team.c.admin.id(), wrong).await;
    assert!(
        shown(&s, &reader.secret, &query).await.is_empty(),
        "a same UUID of another actual kind is not a Host"
    );
    let denied = key(&s.world, s.team.c.stranger.id(), full).await;
    assert_eq!(
        call(&s, &denied.secret, &query).await.status,
        StatusCode::FORBIDDEN,
        "scope cannot supply the native node.interact door"
    );
}

#[tokio::test]
async fn places_mount_names_come_from_actual_native_rows_and_the_same_operation() {
    use orch_api_contract::access::NewResourceAccess;
    use orch_api_contract::mounts::{MountCreate, MountCreateBody};
    let s = Scene::new().await;
    let mount = s
        .world
        .app
        .call(MountCreate, &[])
        .token(&s.team.c.admin.token)
        .body(&MountCreateBody {
            name: "native-placement-mount-marker".to_owned(),
            description: None,
            home_id: Some(s.team.c.acme),
            target: "/mnt/placement".to_owned(),
            read_only: Some(true),
            default_fate: None,
            access: Some(NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response");
    let unknown = new_id();
    let query = PlaceListQuery {
        template_id: Some(s.team.template),
        config: Some(json!({"env.mounts":{(mount.base.id.to_string()):true}}).to_string()),
    };
    let native = shown(&s, &s.team.c.admin.token, &query).await;
    let native = native
        .iter()
        .find_map(|row| {
            if let Place::Host(host) = row {
                Some(host)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        native.mounts.iter().any(|row| row.mount.id == mount.base.id
            && row.mount.name == "native-placement-mount-marker")
    );
    for grant in [false, true] {
        let mut rules = inputs(&s);
        rules.extend([
            subject("host", s.team.host),
            rule(
                "MountGet",
                "mount",
                mount.base.id,
                "self",
                &["mount"],
                &["read"],
            ),
            subject("mount", unknown),
        ]);
        if grant {
            rules.push(subject("mount", mount.base.id));
        }
        let reader = key(&s.world, s.team.c.admin.id(), rules).await;
        let selected = shown(&s, &reader.secret, &query).await;
        let host = selected
            .iter()
            .find_map(|row| {
                if let Place::Host(host) = row {
                    Some(host)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(host.mounts.len(), usize::from(grant));
        if grant {
            assert_eq!(host.mounts[0].mount.id, mount.base.id);
            assert_eq!(host.mounts[0].mount.name, "native-placement-mount-marker");
        }
        assert!(
            !host.mounts.iter().any(|row| row.mount.id == unknown),
            "a different UUID is never the selected native Mount source"
        );
    }
}

#[tokio::test]
async fn places_publication_keeps_the_actual_inherited_template_across_activity_waits() {
    for change in [false, true] {
        let s = Scene::new().await;
        let other = a_template(
            &s.world,
            &s.team.c.admin,
            s.team.c.acme,
            "Other placement template",
        )
        .await;
        s.world
            .set_at(
                orch_settings::Target::Node(s.team.topic),
                "env.template_id",
                json!(s.team.template),
            )
            .await;
        let mut rules = inputs(&s);
        rules.extend([subject("host", s.team.host), subject("template", other)]);
        let reader = key(&s.world, s.team.c.admin.id(), rules).await;
        assert_eq!(
            shown(&s, &reader.secret, &Default::default()).await.len(),
            1
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
        let home = s.team.topic;
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(NodePlaceList, &[&home]);
            call.token(&token)
                .query(&PlaceListQuery::default())
                .send_bytes()
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the native place body must reach the activity wait");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual placement activity INSERT wait");
        if change {
            s.world
                .set_at(
                    orch_settings::Target::Node(s.team.topic),
                    "env.template_id",
                    json!(other),
                )
                .await;
            assert_eq!(
                shown(&s, &s.team.c.admin.token, &Default::default())
                    .await
                    .len(),
                3,
                "the ordinary caller may still inspect choices using the new Template"
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
