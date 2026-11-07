//! The native administered-node selection keeps each actual opening's own scope and association.

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity;
use orch_types::Json;

const OP: &str = "NodePortOpeningList";

fn rule(kind: &str, id: Id, operation: &str, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}

fn scopes(root: Id, environment: Id) -> Vec<Json> {
    vec![
        rule("org", root, OP, "manage"),
        rule("environment", environment, OP, "read"),
    ]
}

async fn key(world: &World, user: Id, rules: Vec<Json>) -> (Id, String) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native administered port window",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    (id, secret)
}

async fn list(world: &World, root: Id, secret: &str, query: NodePortOpeningListQuery) -> RawReply {
    world
        .app
        .call(NodePortOpeningList, &[&root])
        .token(secret)
        .query(&query)
        .send_bytes()
        .await
}

fn page(reply: RawReply) -> orch_api_contract::common::Page<PortOpening> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn outside_list_uses_native_admin_standing_exact_node_manage_and_selected_environment_reads()
{
    let (world, manager, colleague, env, project, _) = setup().await;
    let root = manager.person.personal_org;
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(manager.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let plan = review(
        &world,
        &manager,
        env,
        &request(9201, project, "selected native window"),
    )
    .await;
    let opening = confirm(&world, &manager, env, &plan, new_id())
        .await
        .opening;
    let (_, selected) = key(&world, manager.id(), scopes(root, env)).await;
    let actual = page(list(&world, root, &selected, NodePortOpeningListQuery::default()).await);
    assert_eq!(actual.items.len(), 1);
    assert_eq!(actual.items[0].id, opening.id);
    assert_eq!(actual.items[0].state, PortOpeningState::Stopped);
    assert!(
        actual.items[0]
            .place
            .as_ref()
            .is_none_or(|place| place.redacted || place.name.is_none())
    );
    for name_scope in [
        rule("project", project, "NodeGet.read", "read"),
        rule("project", project, OP, "read"),
    ] {
        let own = name_scope["allow"][0]["operation"] == OP;
        let mut grants = scopes(root, env);
        grants.push(name_scope);
        let (_, selected) = key(&world, manager.id(), grants).await;
        let actual = page(list(&world, root, &selected, NodePortOpeningListQuery::default()).await);
        assert_eq!(
            actual.items[0]
                .place
                .as_ref()
                .and_then(|place| place.name.as_deref()),
            own.then_some("Preview")
        );
    }
    for grants in [
        vec![
            rule("org", new_id(), OP, "manage"),
            rule("environment", env, OP, "read"),
        ],
        vec![
            rule("org", root, OP, "read"),
            rule("environment", env, OP, "read"),
        ],
        vec![
            rule("org", root, "NodeGet.read", "read"),
            rule("environment", env, OP, "read"),
        ],
        vec![
            rule("environment", env, OP, "manage"),
            rule("environment", env, OP, "read"),
        ],
    ] {
        let (_, denied) = key(&world, manager.id(), grants).await;
        assert_eq!(
            list(&world, root, &denied, NodePortOpeningListQuery::default())
                .await
                .status
                .as_u16(),
            403
        );
    }
    let (_, no_native) = key(&world, colleague.id(), scopes(root, env)).await;
    assert_eq!(
        list(
            &world,
            root,
            &no_native,
            NodePortOpeningListQuery::default()
        )
        .await
        .status
        .as_u16(),
        403
    );
    orch_db::queries::access::testing::add_entry(
        &mut *world.conn().await,
        &orch_db::queries::access::PageKey::new("node", root, "live"),
        "any",
        "org.manage",
        ("user", Some(colleague.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    assert_eq!(
        list(
            &world,
            root,
            &no_native,
            NodePortOpeningListQuery::default()
        )
        .await
        .status
        .as_u16(),
        403,
        "a page permission is not the native Admin role"
    );
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(colleague.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let native_admin = page(
        list(
            &world,
            root,
            &no_native,
            NodePortOpeningListQuery::default(),
        )
        .await,
    );
    assert_eq!(native_admin.items[0].id, opening.id);
    assert!(
        !native_admin.items[0].can.close,
        "ordinary key admission does not lend the browser's private override"
    );
}

#[tokio::test]
async fn outside_scope_filters_before_the_native_window_and_preserves_empty_and_query_filter_cursors()
 {
    let (world, manager, _, env, project, machine) = setup().await;
    let root = manager.person.personal_org;
    let other = world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&manager.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: root,
            name: "Second native selection".into(),
            root: "/srv/ports-core/other".into(),
            home_node_id: None,
            remote_write: None,
            terminal: None,
            access: Some(orch_api_contract::binding::NewEnvironmentAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
        .base
        .id;
    for (environment, port) in [(env, 9202), (other, 9203), (other, 9204)] {
        let plan = review(
            &world,
            &manager,
            environment,
            &request(port, project, "native page"),
        )
        .await;
        confirm(&world, &manager, environment, &plan, new_id()).await;
    }
    let (_, selected) = key(&world, manager.id(), scopes(root, other)).await;
    let first = page(
        list(
            &world,
            root,
            &selected,
            NodePortOpeningListQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await,
    );
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].environment_id, other);
    assert!(first.next_cursor.is_some());
    let second = page(
        list(
            &world,
            root,
            &selected,
            NodePortOpeningListQuery {
                limit: Some(1),
                cursor: first.next_cursor.clone(),
                ..Default::default()
            },
        )
        .await,
    );
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].environment_id, other);
    assert_ne!(first.items[0].id, second.items[0].id);
    assert!(second.next_cursor.is_none());
    let filtered = page(
        list(
            &world,
            root,
            &selected,
            NodePortOpeningListQuery {
                kind: Some(PortOpeningKind::Link),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await,
    );
    assert!(filtered.items.is_empty());
    assert_eq!(
        filtered.next_cursor, first.next_cursor,
        "native kind filtering remains after its selected window"
    );
    let (_, empty) = key(&world, manager.id(), vec![rule("org", root, OP, "manage")]).await;
    let actual = page(
        list(
            &world,
            root,
            &empty,
            NodePortOpeningListQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await,
    );
    assert!(actual.items.is_empty() && actual.next_cursor.is_none());
}

#[tokio::test]
async fn outside_host_rows_need_the_actual_publisher_read_without_a_fabricated_host() {
    let (world, manager, _, env, project, machine) = setup().await;
    let root = manager.person.personal_org;
    port_policy(&world, "port_host", "ask").await;
    let ports = orch_link_contract::policy::MachinePorts {
        allowed: vec![PortOpeningKind::Host],
        publish: Some(HostPorts {
            incoming_address: "native-publisher.example.test".into(),
            listen: "127.0.0.1".into(),
            ranges: vec![PortRange {
                from: 50100,
                to: 50102,
            }],
        }),
        limits: None,
    };
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports}',$2) WHERE id=$1")
        .bind(machine)
        .bind(serde_json::to_value(ports).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut wanted = request(9207, project, "the native physical publisher");
    wanted.kind = PortOpeningKind::Host;
    wanted.place_id = None;
    wanted.transports = Some(vec![PortTransport::Tcp]);
    wanted.host_port = Some(50100);
    let plan = review(&world, &manager, env, &wanted).await;
    assert!(plan.blockers.is_empty());
    let opened = confirm(&world, &manager, env, &plan, new_id())
        .await
        .opening;
    for publisher_scope in [
        None,
        Some(rule("machine", new_id(), OP, "read")),
        Some(rule("machine", machine, "MachineGet", "read")),
        Some(rule("machine", machine, OP, "read")),
    ] {
        let allowed = publisher_scope.as_ref().is_some_and(|value| {
            value["allow"][0]["operation"] == OP && value["target"]["id"] == json!(machine)
        });
        let mut grants = scopes(root, env);
        grants.extend(publisher_scope);
        let (_, selected) = key(&world, manager.id(), grants).await;
        let actual = page(list(&world, root, &selected, NodePortOpeningListQuery::default()).await);
        assert_eq!(actual.items.len(), usize::from(allowed));
        if allowed {
            assert_eq!(actual.items[0].id, opened.id);
            assert_eq!(actual.items[0].publisher.as_ref().unwrap().id, machine);
            assert!(actual.items[0].host.is_none());
        }
    }
}

#[tokio::test]
async fn outside_publication_keeps_each_original_opening_association_after_the_actual_activity_wait()
 {
    for mode in ["unchanged", "scope", "expire", "association"] {
        let (world, manager, _, env, project, _) = setup().await;
        let root = manager.person.personal_org;
        let org = orch_db::queries::hierarchy::with_ancestors(&mut *world.conn().await, &[project])
            .await
            .unwrap()
            .into_iter()
            .find(|node| node.id == project)
            .unwrap()
            .path[0];
        let other = make_project(&world, &manager, org, "Another retained opening place")
            .await
            .base
            .id;
        let mut ids = Vec::new();
        for (port, place) in [(9205, project), (9206, other)] {
            let plan = review(
                &world,
                &manager,
                env,
                &request(port, place, "retained native association"),
            )
            .await;
            ids.push(
                confirm(&world, &manager, env, &plan, new_id())
                    .await
                    .opening
                    .id,
            );
        }
        let mut grants = scopes(root, env);
        grants.extend([
            rule("project", project, OP, "read"),
            rule("project", other, OP, "read"),
        ]);
        let (key_id, selected) = key(&world, manager.id(), grants).await;
        assert_eq!(
            page(list(&world, root, &selected, NodePortOpeningListQuery::default()).await)
                .items
                .len(),
            2
        );
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(NodePortOpeningList, &[&root])
                .token(&selected)
                .query(&NodePortOpeningListQuery::default());
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!pending.is_finished(), "the actual activity wait must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        match mode {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key_id)
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expire" => world.advance(chrono::Duration::hours(2)),
            "association" => {
                sqlx::query("UPDATE port_openings SET place_id=CASE WHEN id=$1 THEN $3 ELSE $4 END WHERE id=$1 OR id=$2").bind(ids[0]).bind(ids[1]).bind(other).bind(project).execute(&world.db.pool).await.unwrap();
            }
            _ => {}
        }
        blocker.commit().await.unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if mode == "unchanged" {
            assert_eq!(page(actual).items.len(), 2);
        } else {
            assert!(
                !actual.status.is_success(),
                "{mode} cannot publish the original retained rows"
            );
        }
    }
}

#[tokio::test]
async fn overview_outside_uses_its_own_selected_environment_read_with_the_shared_native_window() {
    use orch_api_contract::nodes::{
        OrgOverviewGet, OrgOverviewGetQuery, OrgOverviewResponse, OverviewPart,
    };
    let (world, manager, _, env, project, _) = setup().await;
    let root = manager.person.personal_org;
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(manager.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    // A visible native child makes the personal overview's existing context
    // sections applicable; the port's foreign named place is a separate source.
    let child = make_project(&world, &manager, root, "Overview child")
        .await
        .base
        .id;
    let plan = review(
        &world,
        &manager,
        env,
        &request(9260, project, "overview native port window"),
    )
    .await;
    let opening = confirm(&world, &manager, env, &plan, new_id())
        .await
        .opening;
    for operation in [None, Some("EnvironmentGet.read"), Some("OrgOverviewGet")] {
        let mut grants = vec![
            rule("org", root, "OrgOverviewGet", "read"),
            rule("org", root, "OrgOverviewGet", "manage"),
            rule("project", child, "OrgOverviewGet", "read"),
        ];
        if let Some(operation) = operation {
            grants.push(rule("environment", env, operation, "read"));
        }
        let (_, secret) = key(&world, manager.id(), grants).await;
        let response = world
            .app
            .call(OrgOverviewGet, &[&root])
            .token(&secret)
            .query(&OrgOverviewGetQuery::default())
            .send()
            .await
            .ok();
        let OrgOverviewResponse::Overview { data } = response else {
            panic!("the complete overview was requested")
        };
        let OverviewPart::Ready { data: page } = data.outside_openings else {
            panic!("the native administered window remains available")
        };
        if operation == Some("OrgOverviewGet") {
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].id, opening.id);
            assert_eq!(page.items[0].state, PortOpeningState::Stopped);
        } else {
            assert!(
                page.items.is_empty(),
                "another operation cannot lend the environment read"
            );
        }
    }
}
