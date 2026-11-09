//! One effect-free native volume request and one existing dispatcher writer.
use super::*;
use crate::dispatch::{DispatchError, Dispatcher, StoredCommand};
use orch_engine::commands::{BeforeCommand, CommandAuthority, Outgoing};
use orch_link_contract::commands::{NodeVolumeEnsure, NodeVolumeRemove, SerialKey};

/// Actual selected native data-maker operands. This packet grants no authority.
pub(super) struct Prepared {
    id: Id,
    location: LocationRow,
    driver: String,
    remove: bool,
    deadline: Option<Time>,
}
/// The real accepted queue identity; waiting never repeats preparation or issuance.
pub(super) struct Pending {
    id: Id,
    remove: bool,
}

pub(super) async fn prepare(
    conn: &mut Conn,
    location: &LocationRow,
    remove: bool,
) -> ApiResult<Prepared> {
    let (driver, storages) = rows::data_driver(conn, location.machine_id)
        .await?
        .ok_or_else(|| invalid("kind", "this machine has not offered making data"))?;
    if let Some(storage) = &location.storage
        && !storages.as_array().is_some_and(|storages| {
            storages
                .iter()
                .any(|row| row.get("name").and_then(Json::as_str) == Some(storage))
        })
    {
        return Err(invalid("storage", "this host has no storage of that name"));
    }
    Ok(Prepared {
        id: orch_types::new_id(),
        location: location.clone(),
        driver,
        remove,
        deadline: None,
    })
}
fn issue_error(error: DispatchError) -> ApiError {
    match error {
        DispatchError::BeforeEffect(error) => match error.downcast::<ApiError>() {
            Ok(error) => *error,
            Err(error) => ApiError::internal(error),
        },
        other => ApiError::internal(other),
    }
}
impl Prepared {
    /// Retain the native command's original allowance before building its private carrier.
    pub(super) fn with_deadline(mut self, deadline: Time) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Native location facts retained before the data-maker wait, not reply-derived targets.
    pub(super) fn location(&self) -> &LocationRow {
        &self.location
    }
    /// The one typed native body, also available to its original-operation carrier.
    pub(super) fn command(&self) -> ApiResult<Outgoing> {
        let location = self.location();
        let labels = BTreeMap::from([("orch.mount_id".to_owned(), location.mount_id.to_string())]);
        let key = SerialKey::Volume(location.source.clone());
        if self.remove {
            Outgoing::new(
                self.id,
                location.machine_id,
                key,
                &NodeVolumeRemove {
                    driver: self.driver.clone(),
                    name: location.source.clone(),
                    storage: location.storage.clone(),
                    labels,
                },
            )
            .map(|mut command| {
                command.deadline = self.deadline;
                command
            })
            .map_err(ApiError::internal)
        } else {
            Outgoing::new(
                self.id,
                location.machine_id,
                key,
                &NodeVolumeEnsure {
                    driver: self.driver.clone(),
                    name: location.source.clone(),
                    storage: location.storage.clone(),
                    labels,
                },
            )
            .map(|mut command| {
                command.deadline = self.deadline;
                command
            })
            .map_err(ApiError::internal)
        }
    }

    /// The same native queue entrypoint runs the owner's opaque cut after its awaited footprint.
    pub(super) async fn queue(
        self,
        dispatcher: &Dispatcher,
        authority: Option<CommandAuthority>,
        before: BeforeCommand<'_>,
    ) -> ApiResult<Pending> {
        let command = self.command()?;
        let issued = dispatcher
            .issue_stored_before(
                StoredCommand {
                    id: Some(command.id),
                    machine: command.machine,
                    kind: command.kind.as_str().into(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    body: command.body,
                    authority,
                },
                before,
            )
            .await
            .map_err(issue_error)?;
        Ok(Pending {
            id: issued.id,
            remove: self.remove,
        })
    }
}
impl Pending {
    pub(super) async fn wait(self, dispatcher: &Dispatcher) -> ApiResult<()> {
        // Neither volume command is an answer-once observation; keep its native retained result.
        if self.remove {
            dispatcher
                .wait_for::<NodeVolumeRemove>(self.id)
                .await
                .map_err(ApiError::internal)?;
        } else {
            dispatcher
                .wait_for::<NodeVolumeEnsure>(self.id)
                .await
                .map_err(ApiError::internal)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "a test fails by panicking"
    )]
    use super::*;
    use crate::rig::Scene;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    async fn fixture(scene: &Scene) -> LocationRow {
        let request = scene
            .rig
            .world
            .app
            .call(api::MountCreate, &[])
            .token(&scene.ada.token)
            .body(&api::MountCreateBody {
                name: "native-volume".into(),
                description: None,
                home_id: Some(scene.tree.org.base.id),
                target: "/work/native-volume".into(),
                read_only: None,
                default_fate: None,
                access: None,
            });
        let mount = request.send().await.ok().into_full().unwrap().base.id;
        sqlx::query("UPDATE machines SET facts=jsonb_set(jsonb_set(facts,'{environment_descriptor}',jsonb_build_object('name','docker','data_storage',jsonb_build_object('made_by_orch','volume'))),'{storages}','[{\"name\":\"fast\"}]'::jsonb) WHERE id=$1").bind(scene.rig.machine_id).execute(scene.rig.db.pool()).await.unwrap();
        LocationRow {
            mount_id: mount,
            machine_id: scene.rig.machine_id,
            host_id: None,
            host_name: None,
            kind: "volume".into(),
            source: format!("orch-mount-{mount}"),
            storage: Some("fast".into()),
            data_kind: None,
            created_by: scene.ada.id(),
            created_at: scene.rig.world.now(),
        }
    }
    async fn count(scene: &Scene) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE machine_id=$1 AND kind IN ('node.volume.ensure','node.volume.remove')").bind(scene.rig.machine_id).fetch_one(scene.rig.db.pool()).await.unwrap()
    }
    #[tokio::test]
    async fn native_volume_preparation_freezes_the_actual_driver_storage_and_owner_labels_without_issuing()
     {
        let scene = Scene::new().await;
        let location = fixture(&scene).await;
        let before = count(&scene).await;
        for remove in [false, true] {
            let mut conn = scene.rig.db.acquire().await.unwrap();
            let prepared = prepare(&mut conn, &location, remove).await.unwrap();
            assert_eq!(prepared.location().machine_id, location.machine_id);
            assert_eq!(prepared.location().mount_id, location.mount_id);
            assert_eq!(prepared.driver, "docker");
            assert_eq!(prepared.remove, remove);
            assert_eq!(count(&scene).await, before);
        }
        let mut unknown = location.clone();
        unknown.storage = Some("not-offered".into());
        let mut conn = scene.rig.db.acquire().await.unwrap();
        assert!(prepare(&mut conn, &unknown, false).await.is_err());
        assert_eq!(count(&scene).await, before);
    }
    #[tokio::test]
    async fn native_volume_first_queue_cut_refuses_before_insert_and_positive_control_retains_one_exact_command()
     {
        let scene = Scene::new().await;
        let location = fixture(&scene).await;
        let dispatcher = scene.rig.world.app.app.link.dispatcher();
        for remove in [false, true] {
            let before = count(&scene).await;
            let checks = Arc::new(AtomicUsize::new(0));
            let called = Arc::clone(&checks);
            let prepared = {
                let mut conn = scene.rig.db.acquire().await.unwrap();
                prepare(&mut conn, &location, remove).await.unwrap()
            };
            let refused = prepared
                .queue(
                    &dispatcher,
                    None,
                    Box::new(move || {
                        called.fetch_add(1, Ordering::SeqCst);
                        Err(Box::new(ApiError::new(
                            ErrorCode::AuthzDenied,
                            "the native first queue cut closed",
                        )))
                    }),
                )
                .await;
            assert!(matches!(refused,Err(error)if error.code()==ErrorCode::AuthzDenied));
            assert_eq!(checks.load(Ordering::SeqCst), 1);
            assert_eq!(count(&scene).await, before);
            let prepared = {
                let mut conn = scene.rig.db.acquire().await.unwrap();
                prepare(&mut conn, &location, remove).await.unwrap()
            };
            let called = Arc::clone(&checks);
            let pending = prepared
                .queue(
                    &dispatcher,
                    None,
                    Box::new(move || {
                        called.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }),
                )
                .await
                .unwrap();
            assert_eq!(checks.load(Ordering::SeqCst), 2);
            assert_eq!(count(&scene).await, before + 1);
            let row =
                orch_db::queries::link::command(&mut *scene.rig.world.conn().await, pending.id)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(row.machine_id, location.machine_id);
            assert_eq!(
                row.kind,
                if remove {
                    "node.volume.remove"
                } else {
                    "node.volume.ensure"
                }
            );
            assert_eq!(
                row.serial_key(),
                SerialKey::Volume(location.source.clone()).wire().as_deref()
            );
            assert_eq!(
                row.body,
                json!({"driver":"docker","name":location.source,"storage":"fast","labels":{"orch.mount_id":location.mount_id.to_string()}})
            );
            dispatcher.cancel(pending.id).await.unwrap();
        }
    }
}
