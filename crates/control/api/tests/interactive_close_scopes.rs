//! Scoped Close uses the existing Environment door and preserves committed native cleanup.

mod native_close_support;
mod places;
mod rig;
mod support;
mod thread_history_scope;

use crate::support::FullPermissionPageReply;
use native_close_support::{
    PRIVATE, Proof, Surface, audited, body, executions, ordinary, receipt, scene,
};
use orch_types::new_id;
use serde_json::json;
use thread_history_scope::{key, narrow, rule};

async fn full(surface: Surface) -> Proof {
    let scene = scene(surface).await?;
    ordinary(surface, &scene).await?;
    let id = surface.make(&scene).await?;
    let selected = key(
        &scene.rig.world,
        scene.cai.id(),
        vec![
            json!({"target":{"kind":"environment","id":scene.on.environment,"reach":"self"},
        "allow":[{"operation":surface.operation(),"actions":["manage","read"]}]}),
        ],
    )
    .await?;
    let before = executions(&scene, surface);
    let value = body(&surface.close(&scene, id, &selected.secret).await)?;
    assert_eq!(value["state"], "closed");
    assert!(serde_json::to_string(&value)?.contains(PRIVATE));
    assert!(value.get("projection").is_none());
    assert_eq!(executions(&scene, surface), before + 1);
    assert_eq!(audited(&scene, surface, id).await?, 1);
    Ok(())
}
async fn management_only(surface: Surface) -> Proof {
    let scene = scene(surface).await?;
    ordinary(surface, &scene).await?;
    let id = surface.make(&scene).await?;
    // Another endpoint's Read is deliberately not this Close operation's response grant.
    let selected = key(
        &scene.rig.world,
        scene.cai.id(),
        vec![
            rule(
                surface.operation(),
                "environment",
                scene.on.environment,
                "self",
                "manage",
            ),
            rule(
                surface.read_operation(),
                "environment",
                scene.on.environment,
                "self",
                "read",
            ),
        ],
    )
    .await?;
    let before = executions(&scene, surface);
    receipt(
        surface,
        &surface.close(&scene, id, &selected.secret).await,
        id,
        scene.on.environment,
    )?;
    assert!(surface.closed(&scene, id).await?);
    receipt(
        surface,
        &surface.close(&scene, id, &selected.secret).await,
        id,
        scene.on.environment,
    )?;
    assert_eq!(
        executions(&scene, surface),
        before + 1,
        "replay must not ask the native program twice"
    );
    assert_eq!(audited(&scene, surface, id).await?, 1);
    Ok(())
}
async fn bounds(surface: Surface) -> Proof {
    let scene = scene(surface).await?;
    ordinary(surface, &scene).await?;
    let id = surface.make(&scene).await?;
    let peer = scene.rig.resources(&scene.bea, &scene.tree).await;
    let before = executions(&scene, surface);
    for (user, rules) in [
        (
            scene.cai.id(),
            vec![rule(
                surface.operation(),
                "environment",
                peer.environment,
                "self",
                "manage",
            )],
        ),
        (
            scene.cai.id(),
            vec![rule(
                surface.operation(),
                "environment",
                scene.on.environment,
                "self",
                "write",
            )],
        ),
        (
            scene.cai.id(),
            vec![rule(
                surface.operation(),
                "provider",
                scene.on.provider,
                "self",
                "manage",
            )],
        ),
        (
            scene.cai.id(),
            vec![rule(
                surface.read_operation(),
                "environment",
                scene.on.environment,
                "self",
                "read",
            )],
        ),
        (
            scene.bea.id(),
            vec![rule(
                surface.operation(),
                "environment",
                scene.on.environment,
                "self",
                "manage",
            )],
        ),
    ] {
        let selected = key(&scene.rig.world, user, rules).await?;
        let reply = surface.close(&scene, id, &selected.secret).await;
        assert_eq!(
            reply.problem().code,
            orch_api_contract::common::ErrorCode::AuthzDenied
        );
        assert!(!String::from_utf8_lossy(&reply.raw).contains(PRIVATE));
        assert!(!surface.closed(&scene, id).await?);
    }
    assert_eq!(executions(&scene, surface), before);
    assert_eq!(audited(&scene, surface, id).await?, 0);
    Ok(())
}

struct Release(std::sync::Arc<orch_testkit::doubles::machine::ScriptedHandler>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
async fn cleanup_after_commit(surface: Surface) -> Proof {
    let scene = scene(surface).await?;
    ordinary(surface, &scene).await?;
    // Fail an unbound baseline before installing a barrier the operation cannot reach.
    let preflight = surface.make(&scene).await?;
    let original = vec![
        json!({"target":{"kind":"environment","id":scene.on.environment,"reach":"self"},
        "allow":[{"operation":surface.operation(),"actions":["manage","read"]}]}),
    ];
    let preflight_key = key(&scene.rig.world, scene.cai.id(), original.clone()).await?;
    body(
        &surface
            .close(&scene, preflight, &preflight_key.secret)
            .await,
    )?;
    for mode in ["narrow", "revoke", "expire"] {
        let id = surface.make(&scene).await?;
        let selected = key(&scene.rig.world, scene.cai.id(), original.clone()).await?;
        let before = executions(&scene, surface);
        let _release = Release(scene.rig.machine.handler.clone());
        scene.rig.machine.handler.hold(surface.kind());
        let write = async {
            rig::eventually(
                "the accepted native close is waiting for its answer",
                || async { executions(&scene, surface) > before },
            )
            .await;
            assert!(
                surface.closed(&scene, id).await?,
                "closed row precedes accepted native cleanup"
            );
            assert_eq!(audited(&scene, surface, id).await?, 1);
            match mode {
                "narrow" => {
                    narrow(
                        &scene.rig.world,
                        &selected,
                        vec![rule(
                            surface.operation(),
                            "environment",
                            scene.on.environment,
                            "self",
                            "manage",
                        )],
                    )
                    .await?
                }
                "revoke" => {
                    assert!(
                        orch_db::queries::identity::revoke_api_token(
                            &mut *scene.rig.world.conn().await,
                            selected.id,
                            scene.cai.id(),
                            scene.rig.world.now()
                        )
                        .await?
                    );
                }
                _ => {
                    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                        .bind(selected.id)
                        .bind(scene.rig.world.now())
                        .execute(&scene.rig.world.db.pool)
                        .await?;
                }
            }
            scene.rig.machine.handler.release();
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        let (reply, written) = tokio::time::timeout(rig::PATIENCE, async {
            tokio::join!(surface.close(&scene, id, &selected.secret), write)
        })
        .await?;
        written?;
        receipt(surface, &reply, id, scene.on.environment)?;
        assert_eq!(executions(&scene, surface), before + 1);
        assert_eq!(audited(&scene, surface, id).await?, 1);
    }
    Ok(())
}
macro_rules! controls {
    ($full:ident,$receipt:ident,$bounds:ident,$cleanup:ident,$surface:expr) => {
        #[tokio::test]
        async fn $full() -> Proof {
            full($surface).await
        }
        #[tokio::test]
        async fn $receipt() -> Proof {
            management_only($surface).await
        }
        #[tokio::test]
        async fn $bounds() -> Proof {
            bounds($surface).await
        }
        #[tokio::test]
        async fn $cleanup() -> Proof {
            cleanup_after_commit($surface).await
        }
    };
}
controls!(
    browser_close_full_same_operation_read,
    browser_close_management_only_receipt,
    browser_close_scope_and_ordinary_bounds,
    browser_close_committed_cleanup_survives_read_loss,
    Surface::Browser
);
controls!(
    display_close_full_same_operation_read,
    display_close_management_only_receipt,
    display_close_scope_and_ordinary_bounds,
    display_close_committed_cleanup_survives_read_loss,
    Surface::Display
);

async fn bounded_thread_reference(surface: Surface) -> Proof {
    let scene = scene(surface).await?;
    let named = "Foreign Thread name needs this Close operation Read";
    let thread = scene.thread_by(&scene.ada, named).await.base.id;
    let sibling = scene
        .thread_by(&scene.ada, "Other named Thread")
        .await
        .base
        .id;
    let ordinary_id = surface.make_for(&scene, Some(thread)).await?;
    let ordinary_reply = surface.close(&scene, ordinary_id, &scene.cai.token).await;
    assert_eq!(
        ordinary_reply.status,
        http::StatusCode::OK,
        "ordinary linked-Thread {} using the actual session credential: {}",
        surface.operation(),
        String::from_utf8_lossy(&ordinary_reply.raw)
    );
    let ordinary = body(&ordinary_reply)?;
    assert_eq!(ordinary["thread"]["id"], json!(thread));
    assert_eq!(
        ordinary["thread"]["name"], named,
        "ordinary same-reader name premise"
    );
    let base = vec![
        rule(
            surface.operation(),
            "environment",
            scene.on.environment,
            "self",
            "manage",
        ),
        rule(
            surface.operation(),
            "environment",
            scene.on.environment,
            "self",
            "read",
        ),
    ];
    for source in [
        rule("ThreadSyncGet.read", "thread", thread, "self", "read"),
        rule(surface.operation(), "thread", sibling, "self", "read"),
        rule(surface.operation(), "thread", thread, "self", "read"),
    ] {
        let named_here = source["allow"][0]["operation"] == surface.operation()
            && source["target"]["id"] == json!(thread);
        let mut rules = base.clone();
        rules.push(source);
        let selected = key(&scene.rig.world, scene.cai.id(), rules).await?;
        let id = surface.make_for(&scene, Some(thread)).await?;
        let reply = surface.close(&scene, id, &selected.secret).await;
        let value = body(&reply)?;
        assert_eq!(value["state"], "closed");
        assert!(
            value.get("projection").is_none(),
            "missing optional Thread name must not discard Env Full"
        );
        assert!(serde_json::to_string(&value)?.contains(PRIVATE));
        if named_here {
            assert_eq!(value["thread"]["name"], named);
        } else {
            assert!(value["thread"].is_null());
            assert!(!String::from_utf8_lossy(&reply.raw).contains(named));
        }
    }
    // A credential's selected Thread Read still does not grant its account ordinary Thread View.
    scene
        .rig
        .world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"nodes", &thread],
        )
        .token(&scene.ada.token)
        .body(&orch_api_contract::access::PageChange {
            whole: Some(orch_api_contract::access::PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .full_permission_page()
        .ok();
    let mut rules = base;
    rules.push(rule(surface.operation(), "thread", thread, "self", "read"));
    let selected = key(&scene.rig.world, scene.cai.id(), rules).await?;
    let id = surface.make_for(&scene, Some(thread)).await?;
    let value = body(&surface.close(&scene, id, &selected.secret).await)?;
    assert_eq!(value["state"], "closed");
    assert!(value.get("projection").is_none());
    assert!(value["thread"].is_null());
    assert!(serde_json::to_string(&value)?.contains(PRIVATE));
    Ok(())
}
#[tokio::test]
async fn browser_close_full_bounds_optional_thread_names_to_own_operation() -> Proof {
    bounded_thread_reference(Surface::Browser).await
}
#[tokio::test]
async fn display_close_full_bounds_optional_thread_names_to_own_operation() -> Proof {
    bounded_thread_reference(Surface::Display).await
}

async fn legacy_full(surface: Surface) -> Proof {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token};
    let scene = scene(surface).await?;
    ordinary(surface, &scene).await?;
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let scopes = vec!["*".to_owned()];
    create_api_token(
        &mut *scene.rig.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: scene.cai.id(),
            name: "Legacy full Close guard origin",
            token_hash: &hash,
            scopes: &scopes,
            node_id: None,
            at: scene.rig.world.now(),
            expires_at: Some(scene.rig.world.now() + chrono::Duration::hours(1)),
        },
    )
    .await?;
    let id = surface.make(&scene).await?;
    let before = executions(&scene, surface);
    let reply = surface.close(&scene, id, &secret).await;
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "legacy * {} actual key: {}",
        surface.operation(),
        String::from_utf8_lossy(&reply.raw)
    );
    let value = body(&reply)?;
    assert_eq!(value["state"], "closed");
    assert!(value.get("projection").is_none());
    assert!(serde_json::to_string(&value)?.contains(PRIVATE));
    assert_eq!(executions(&scene, surface), before + 1);
    assert_eq!(audited(&scene, surface, id).await?, 1);
    Ok(())
}
#[tokio::test]
async fn browser_close_legacy_full_keeps_actual_guard_caller_origin() -> Proof {
    legacy_full(Surface::Browser).await
}
#[tokio::test]
async fn display_close_legacy_full_keeps_actual_guard_caller_origin() -> Proof {
    legacy_full(Surface::Display).await
}
