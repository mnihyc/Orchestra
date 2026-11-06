//! Existing real native-close fixtures shared by integration and deterministic projection cuts.
#![allow(
    dead_code,
    reason = "each integration/library test uses a subset of the unchanged shared fixture"
)]

use crate::rig;
use orch_api_core::testing::RawReply;
use orch_link_contract::commands::{CommandKind, CommandResult, Empty};
use orch_types::{Id, Json, Time, new_id};
use rig::Scene;
use serde_json::json;

pub(crate) type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub(crate) const PRIVATE: &str = "private-close-metadata";

#[derive(Clone, Copy)]
pub(crate) enum Surface {
    Browser,
    Display,
}
impl Surface {
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::Browser => "BrowserClose",
            Self::Display => "DisplayClose",
        }
    }
    pub(crate) fn read_operation(self) -> &'static str {
        match self {
            Self::Browser => "EnvironmentBrowserList",
            Self::Display => "EnvironmentDisplayList",
        }
    }
    pub(crate) fn permission(self) -> &'static str {
        match self {
            Self::Browser => "browser.manage",
            Self::Display => "vnc.manage",
        }
    }
    pub(crate) fn kind(self) -> CommandKind {
        match self {
            Self::Browser => CommandKind::BrowserClose,
            Self::Display => CommandKind::DisplayClose,
        }
    }
    pub(crate) fn action(self) -> &'static str {
        match self {
            Self::Browser => "browser.closed",
            Self::Display => "display.closed",
        }
    }
    pub(crate) async fn close(self, scene: &Scene, id: Id, token: &str) -> RawReply {
        match self {
            Self::Browser => {
                scene
                    .rig
                    .world
                    .app
                    .call(orch_api_contract::browsers::BrowserClose, &[&id])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Self::Display => {
                scene
                    .rig
                    .world
                    .app
                    .call(orch_api_contract::displays::DisplayClose, &[&id])
                    .token(token)
                    .send_bytes()
                    .await
            }
        }
    }
    pub(crate) async fn make(self, scene: &Scene) -> Proof<Id> {
        self.make_for(scene, None).await
    }
    pub(crate) async fn make_for(self, scene: &Scene, thread: Option<Id>) -> Proof<Id> {
        let id = new_id();
        let name = format!("{PRIVATE}-{id}");
        let mut conn = scene.rig.world.conn().await;
        match self {
            Self::Browser => {
                use orch_db::queries::browsers::{NewBrowser, insert, opened};
                insert(
                    &mut conn,
                    &NewBrowser {
                        id,
                        environment: scene.on.environment,
                        generation: 1,
                        profile: Some(&name),
                        origin: "started",
                        person: Some(scene.ada.id()),
                        thread,
                        preset: Some("headless"),
                        display: None,
                        note: Some(PRIVATE),
                    },
                    scene.rig.world.now(),
                )
                .await?;
                opened(
                    &mut conn,
                    id,
                    "http://127.0.0.1:9222",
                    "scripted-browser",
                    None,
                    scene.rig.world.now(),
                )
                .await?
                .ok_or("browser reservation did not open")?;
            }
            Self::Display => {
                use orch_db::queries::displays::{NewDisplay, insert, opened};
                insert(
                    &mut conn,
                    &NewDisplay {
                        id,
                        environment: scene.on.environment,
                        generation: 1,
                        name: &name,
                        origin: "started",
                        person: Some(scene.ada.id()),
                        thread,
                        note: Some(PRIVATE),
                        password: None,
                    },
                    scene.rig.world.now(),
                )
                .await?;
                opened(
                    &mut conn,
                    id,
                    "127.0.0.1:5901",
                    Some(":1"),
                    None,
                    Some("800x600"),
                    scene.rig.world.now(),
                )
                .await?
                .ok_or("display reservation did not open")?;
            }
        }
        Ok(id)
    }
    pub(crate) async fn closed(self, scene: &Scene, id: Id) -> Proof<bool> {
        let mut conn = scene.rig.world.conn().await;
        Ok(match self {
            Self::Browser => {
                orch_db::queries::browsers::get(&mut conn, id)
                    .await?
                    .ok_or("browser absent")?
                    .state
                    == "closed"
            }
            Self::Display => {
                orch_db::queries::displays::get(&mut conn, id)
                    .await?
                    .ok_or("display absent")?
                    .state
                    == "closed"
            }
        })
    }
}

pub(crate) async fn scene(surface: Surface) -> Proof<Scene> {
    let scene = Scene::new().await;
    scene.rig.machine.handler.on(surface.kind(), move |_| {
        Ok(match surface {
            Surface::Browser => CommandResult::BrowserClose(Empty {}),
            Surface::Display => CommandResult::DisplayClose(Empty {}),
        })
    });
    sqlx::query("UPDATE environments SET browser=true,vnc=true WHERE id=$1")
        .bind(scene.on.environment)
        .execute(&scene.rig.world.db.pool)
        .await?;
    // A Viewer is named only at the requested door. It is not an Environment manager.
    scene
        .rig
        .lets_use(
            "environment",
            scene.on.environment,
            surface.permission(),
            &scene.cai,
            json!({}),
        )
        .await;
    let mut conn = scene.rig.world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(scene.cai.id()),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let decision = orch_authz::resources::check(
        &mut conn,
        &principal,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: scene.on.environment,
        },
        orch_access_contract::permissions::Permission::EnvironmentManage,
        orch_authz::Policy::default(),
    )
    .await?;
    assert!(
        !decision.is_allowed(),
        "the door-only fixture must not secretly manage the Environment"
    );
    drop(conn);
    Ok(scene)
}

pub(crate) fn body(reply: &RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
pub(crate) fn receipt(surface: Surface, reply: &RawReply, id: Id, environment: Id) -> Proof {
    let value = body(reply)?;
    assert_eq!(value["projection"], "receipt");
    assert_eq!(
        value[match surface {
            Surface::Browser => "browser_id",
            Surface::Display => "display_id",
        }],
        json!(id)
    );
    assert_eq!(value["environment_id"], json!(environment));
    assert_eq!(value["closed"], true);
    let raw = String::from_utf8_lossy(&reply.raw);
    for content in [
        PRIVATE,
        "127.0.0.1",
        "profile",
        "endpoint",
        "opened_by",
        "geometry",
        "password",
        "opened_at",
        "closed_at",
        "note",
    ] {
        assert!(!raw.contains(content), "receipt disclosed {content}");
    }
    Ok(())
}
pub(crate) fn executions(scene: &Scene, surface: Surface) -> usize {
    scene
        .rig
        .machine
        .handler
        .executions()
        .iter()
        .filter(|entry| entry.kind == surface.kind())
        .count()
}
pub(crate) async fn audited(scene: &Scene, surface: Surface, id: Id) -> Proof<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action=$1 AND object_id=$2")
            .bind(surface.action())
            .bind(id)
            .fetch_one(&scene.rig.world.db.pool)
            .await?,
    )
}
pub(crate) async fn ordinary(surface: Surface, scene: &Scene) -> Proof {
    let id = surface.make(scene).await?;
    let before = executions(scene, surface);
    let ordinary_reply = surface.close(scene, id, &scene.cai.token).await;
    assert_eq!(
        ordinary_reply.status,
        http::StatusCode::OK,
        "ordinary {} using the actual session credential: {}",
        surface.operation(),
        String::from_utf8_lossy(&ordinary_reply.raw)
    );
    let value = body(&ordinary_reply)?;
    assert_eq!(value["state"], "closed");
    assert!(serde_json::to_string(&value)?.contains(PRIVATE));
    assert!(surface.closed(scene, id).await?);
    assert_eq!(executions(scene, surface), before + 1);
    assert_eq!(audited(scene, surface, id).await?, 1);
    Ok(())
}
