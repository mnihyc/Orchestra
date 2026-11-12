//! Environment MCP context keeps native definitions, governors, names and exact selected rows.

use super::*;
use orch_api_contract::environment::{EnvironmentMcp, EnvironmentMcpGet};
use orch_db::queries::resources::mcp::{self as rows, McpRow};

const OPERATION: &str = "EnvironmentMcpGet";

async fn server(s: &Scene, own: Option<Id>, machine: Option<Id>, name: &str) -> Id {
    let id = new_id();
    rows::insert(&mut *s.world.conn().await, &McpRow {
        id, name:name.to_owned(), description:format!("Native context {name}"),
        org_id:Some(s.team.c.acme), home_id:Some(s.team.c.acme), owner_id:Some(s.team.c.admin.id()),
        machine_id:machine, environment_id:own,
        spec:if machine.is_some() { json!({"transport":"stdio", "imported":{
            "source":"codex", "scope":"user", "directory":null, "name":"native-tool", "digest":"fixture-v1",
            "definition_fingerprint":orch_types::Hash::of(b"fixture MCP definition"),
            "at":Time::new(s.world.now()), "by":s.team.c.admin.id(), "tools":3,
        }}) } else { json!({"transport":"stdio", "command":["fixture-tool"], "secret_env":[]}) },
        imported:machine.is_some(), enabled:true, revision:1,
    }, s.world.now()).await.unwrap();
    if own.is_none() {
        start_resource(
            &mut *s.world.conn().await,
            ObjectRef {
                kind: ResourceKind::McpServer.into(),
                id,
            },
            s.team.c.admin.id(),
            NewAccess::OnlyMe,
            Time::new(s.world.now()),
        )
        .await
        .unwrap();
    }
    id
}

fn primary(s: &Scene) -> Json {
    rule(
        OPERATION,
        "environment",
        s.first.base.id,
        "self",
        &["environment"],
        &["read"],
    )
}

fn own(s: &Scene) -> Json {
    rule(
        OPERATION,
        "environment",
        s.first.base.id,
        "self",
        &["mcp_server"],
        &["read"],
    )
}

fn contexts(s: &Scene) -> Json {
    rule(
        OPERATION,
        "org",
        s.team.c.acme,
        "subtree",
        &[
            "org",
            "project",
            "topic",
            "thread",
            "document",
            "document_container",
            "provider",
            "agent",
            "environment",
            "host",
            "machine",
            "template",
            "mount",
            "skill",
        ],
        &["read"],
    )
}

async fn call(s: &Scene, secret: &str) -> RawReply {
    s.world
        .app
        .call(EnvironmentMcpGet, &[&s.first.base.id])
        .token(secret)
        .send_bytes()
        .await
}

async fn shown(s: &Scene, secret: &str) -> EnvironmentMcp {
    let reply = call(s, secret).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn environment_mcp_keeps_native_bodies_and_actual_environment_governed_own_servers() {
    let s = Scene::new().await;
    let first = server(&s, Some(s.first.base.id), None, "first-native-own-server").await;
    let second = server(&s, Some(s.first.base.id), None, "second-native-own-server").await;
    let native = shown(&s, &s.team.c.admin.token).await;
    assert_eq!(native.servers.len(), 2);
    assert!(native.servers.iter().all(|server| server.server.own));
    let reader = key(
        &s.world,
        s.team.c.admin.id(),
        vec![primary(&s), own(&s), contexts(&s)],
    )
    .await;
    let scoped = shown(&s, &reader.secret).await;
    assert_eq!(
        scoped, native,
        "own definitions retain the exact native context, state and unavailable reasons"
    );
    let ids: std::collections::BTreeSet<_> = scoped
        .servers
        .iter()
        .map(|server| server.server.id)
        .collect();
    assert_eq!(ids, [first, second].into_iter().collect());

    let direct = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            primary(&s),
            rule(
                OPERATION,
                "mcp_server",
                first,
                "self",
                &["mcp_server"],
                &["read"],
            ),
            rule(
                "McpServerGet",
                "environment",
                s.first.base.id,
                "self",
                &["mcp_server"],
                &["read"],
            ),
        ],
    )
    .await;
    assert!(
        shown(&s, &direct.secret).await.servers.is_empty(),
        "own servers have their real Environment governor; their opaque id or another operation cannot lend it"
    );
    for rules in [
        vec![
            rule(
                OPERATION,
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["manage"],
            ),
            own(&s),
        ],
        vec![
            rule(
                OPERATION,
                "environment",
                s.second.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
            own(&s),
        ],
        vec![
            rule(
                "EnvironmentGet",
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
            own(&s),
        ],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), rules).await;
        assert_eq!(call(&s, &denied.secret).await.status, StatusCode::FORBIDDEN);
    }
    let stranger = key(&s.world, s.team.c.stranger.id(), vec![primary(&s), own(&s)]).await;
    assert_eq!(
        call(&s, &stranger.secret).await.status,
        StatusCode::FORBIDDEN,
        "the real native Environment reading door remains required"
    );
}

#[tokio::test]
async fn environment_mcp_selects_exact_presets_and_bounds_origin_names_with_its_own_operation() {
    let s = Scene::new().await;
    let first = server(&s, None, None, "selected-preset-marker").await;
    let second = server(&s, None, None, "excluded-preset-marker").await;
    s.world
        .set_at(
            orch_settings::Target::Node(s.team.c.acme),
            "mcp.enabled",
            json!({(first.to_string()):true,(second.to_string()):true}),
        )
        .await;
    let native = shown(&s, &s.team.c.admin.token).await;
    assert_eq!(native.servers.len(), 2);
    let native_first = native
        .servers
        .iter()
        .find(|server| server.server.id == first)
        .unwrap();
    assert!(
        matches!(&native_first.from, Some(orch_api_contract::environment::McpFrom::Node(node)) if node.name.is_some()),
        "the ordinary producer really has the enabling node's protected name"
    );
    let preset = rule(
        OPERATION,
        "mcp_server",
        first,
        "self",
        &["mcp_server"],
        &["read"],
    );
    let narrow = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            primary(&s),
            preset.clone(),
            rule("NodeGet", "org", s.team.c.acme, "self", &["org"], &["read"]),
        ],
    )
    .await;
    let selected = shown(&s, &narrow.secret).await;
    assert_eq!(selected.servers.len(), 1);
    assert_eq!(selected.servers[0].server.id, first);
    assert!(
        selected.servers[0].from.is_none(),
        "NodeGet Read cannot lend this operation's origin name"
    );
    assert!(
        !serde_json::to_string(&selected)
            .unwrap()
            .contains("excluded-preset-marker")
    );
    let full = key(
        &s.world,
        s.team.c.admin.id(),
        vec![primary(&s), preset, contexts(&s)],
    )
    .await;
    let selected = shown(&s, &full.secret).await;
    assert_eq!(selected.servers.len(), 1);
    assert_eq!(selected.servers[0], *native_first);
}

#[tokio::test]
async fn environment_mcp_machine_context_requires_its_actual_machine_source_without_hiding_the_server()
 {
    let s = Scene::new().await;
    let machine = fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.team.c.admin.id(),
        "actual-mcp-machine-marker",
    )
    .await
    .unwrap();
    let imported = server(&s, None, Some(machine), "imported-source-marker").await;
    rows::enable(
        &mut *s.world.conn().await,
        s.first.base.id,
        imported,
        true,
        s.world.now(),
    )
    .await
    .unwrap();
    let native = shown(&s, &s.team.c.admin.token).await;
    let expected = native
        .servers
        .iter()
        .find(|server| server.server.id == imported)
        .unwrap();
    assert_eq!(expected.server.machine.as_ref().unwrap().id, machine);
    let preset = rule(
        OPERATION,
        "mcp_server",
        imported,
        "self",
        &["mcp_server"],
        &["read"],
    );
    let hidden = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            primary(&s),
            preset.clone(),
            rule(OPERATION, "host", machine, "self", &["machine"], &["read"]),
        ],
    )
    .await;
    let selected = shown(&s, &hidden.secret).await;
    assert_eq!(selected.servers.len(), 1);
    assert_eq!(selected.servers[0].server.id, imported);
    assert!(
        selected.servers[0].server.machine.is_none(),
        "same UUID with the wrong actual kind grants no name"
    );
    let full = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            primary(&s),
            preset,
            rule(
                OPERATION,
                "machine",
                machine,
                "self",
                &["machine"],
                &["read"],
            ),
        ],
    )
    .await;
    let selected = shown(&s, &full.secret).await;
    assert_eq!(selected.servers[0].server, expected.server);
    assert_eq!(selected.servers[0].state, expected.state);
    assert_eq!(selected.servers[0].reason, expected.reason);
}

#[tokio::test]
async fn environment_mcp_publication_cannot_replace_an_own_server_under_the_same_environment_governor()
 {
    for replace in [false, true] {
        let s = Scene::new().await;
        let old = server(
            &s,
            Some(s.first.base.id),
            None,
            "original-own-server-marker",
        )
        .await;
        let reader = key(&s.world, s.team.c.admin.id(), vec![primary(&s), own(&s)]).await;
        let allowed = shown(&s, &reader.secret).await;
        assert_eq!(allowed.servers.len(), 1);
        assert_eq!(allowed.servers[0].server.id, old);
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
        let environment = s.first.base.id;
        let secret = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentMcpGet, &[&environment]);
            call.token(&secret).send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the actual native server body must reach publication");
                let blocked:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual activity INSERT after native body preparation");
        if replace {
            sqlx::query("UPDATE mcp_servers SET deleted_at=$2 WHERE id=$1")
                .bind(old)
                .bind(s.world.now())
                .execute(&s.world.db.pool)
                .await
                .unwrap();
            let new = server(
                &s,
                Some(s.first.base.id),
                None,
                "replacement-own-server-marker",
            )
            .await;
            let current = shown(&s, &s.team.c.admin.token).await;
            assert_eq!(current.servers.len(), 1);
            assert_eq!(current.servers[0].server.id, new);
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if replace {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "a different current own-server id at the same admitted governor cannot validate old bytes"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("original-own-server-marker"));
        } else {
            assert_eq!(reply.status, StatusCode::OK);
            assert!(String::from_utf8_lossy(&reply.raw).contains("original-own-server-marker"));
        }
    }
}
