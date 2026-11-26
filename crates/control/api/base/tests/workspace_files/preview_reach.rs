//! A file preview keeps the native reach of its bytes across a narrower entry.

use super::*;
use orch_access_contract::permissions::FileReach;
use orch_api_contract::workspace::{
    EnvironmentFilePreviewGet, EnvironmentFileStatQuery, FileSourceFacts, FileSourceKind,
    FileSourcePolicy,
};
use orch_db::queries::{access, storage};

struct MovedPreview {
    disk: Arc<workspace::Disk>,
    stat_facts: FileSourceFacts,
    entered: tokio::sync::Semaphore,
    release: tokio_util::sync::CancellationToken,
}

#[async_trait]
impl Machine for MovedPreview {
    fn cancel(&self, command: Id) {
        self.disk.cancel(command);
    }

    async fn run(&self, command: &orch_link_contract::commands::Cmd) -> Result<Json, CommandError> {
        if command.kind == CommandKind::FsFetch.as_str() {
            assert_eq!(command.body["access"]["view"], "machine");
        }
        let mut result = self.disk.run(command).await?;
        if command.kind == CommandKind::FsStat.as_str() {
            result["facts"] = serde_json::to_value(&self.stat_facts)
                .map_err(|error| CommandError::new(MachineCode::Internal, error.to_string()))?;
        } else if command.kind == CommandKind::FsFetch.as_str() {
            // A link can change target after stat; the read reports the scope it actually opened.
            result["minimum_reach"] = json!(FileReach::Machine);
            // Hold the result after its admitted transfer, before the caller can receive a preview.
            self.entered.add_permits(1);
            self.release.cancelled().await;
        }
        Ok(result)
    }
}

#[tokio::test]
async fn a_preview_does_not_return_machine_bytes_when_its_entry_is_narrowed_while_fetching() {
    let mut site = Site::new().await;
    site.world
        .set("storage.relay_transfers", json!(false))
        .await;
    site.disk
        .put("web/preview.txt", b"private native preview bytes");
    let machine = Arc::new(MovedPreview {
        disk: site.disk.clone(),
        stat_facts: FileSourceFacts {
            policy: FileSourcePolicy {
                kind: FileSourceKind::Workspace,
                workspace_exclusions: true,
                writable: true,
            },
            inclusion: None,
            watch: None,
            observed_at: orch_types::Time::new(site.world.now()),
        },
        entered: tokio::sync::Semaphore::new(0),
        release: Default::default(),
    });
    site.power_on_with(machine.clone());
    let entry = new_id();
    let mut conn = site.world.conn().await;
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id: entry,
            page: &access::PageKey::new("environment", site.environment, "live"),
            profile: "any",
            permission: "files.view",
            holder_kind: "user",
            holder_id: Some(site.eve.id()),
            holder_role: None,
            options: &json!({"reach":"machine"}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(site.ada.id()),
            at: site.world.now(),
        },
    )
    .await
    .unwrap();
    access::bump_epoch(&mut conn).await.unwrap();
    drop(conn);

    let response = async {
        site.app()
            .call(EnvironmentFilePreviewGet, &[&site.environment])
            .token(&site.eve.token)
            .query(&EnvironmentFileStatQuery {
                thread: None,
                path: "web/preview.txt".to_owned(),
                mount: None,
            })
            .send_bytes()
            .await
    };
    tokio::pin!(response);
    tokio::time::timeout(workspace::PATIENCE, async {
        tokio::select! {
            permit = machine.entered.acquire() => permit.unwrap().forget(),
            reply = &mut response => panic!("the preview answered before its native fetch: {:?}", reply.status),
        }
    })
    .await
    .unwrap();
    let mut conn = site.world.conn().await;
    assert!(
        access::rewrite_entry(&mut conn, entry, &json!({"reach":"roots"}), None, "", None)
            .await
            .unwrap()
    );
    access::bump_epoch(&mut conn).await.unwrap();
    drop(conn);
    machine.release.cancel();
    let reply = tokio::time::timeout(workspace::PATIENCE, response)
        .await
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap()["code"],
        "files.reach_denied"
    );
    let cached = storage::cached_latest(
        &mut *site.world.conn().await,
        site.environment,
        "web/preview.txt",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cached.minimum_reach, FileReach::Machine);
}
