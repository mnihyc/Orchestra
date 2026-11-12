//! A native report's directory metadata is visible through current management, never the generic facts blob.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod places;
mod support;
use orch_api_contract::{
    resources::{Machine, MachineGet, MachineList},
    storage::{MachineDataInventory, MachineDataKind, MachineDataPart},
};
use orch_db::queries::{access, resources};
use orch_types::Id;
use serde_json::json;

async fn read_both(world: &support::World, person: &places::Who, machine: Id) -> [Machine; 2] {
    let individual = world
        .app
        .call(MachineGet, &[&machine])
        .token(&person.token)
        .send()
        .await
        .ok();
    let listed = world
        .app
        .call(MachineList, &[])
        .token(&person.token)
        .send()
        .await
        .ok()
        .into_iter()
        .find(|row| row.base.id == machine)
        .expect("the retained View permission still lists this actual machine");
    assert_eq!(individual.base.id, machine);
    [individual, listed]
}

#[tokio::test]
async fn machine_directory_metadata_is_typed_for_managers_and_absent_from_view_only_facts() {
    let world = support::World::new().await;
    let owner = places::person(&world, "data-owner").await;
    let manager = places::person(&world, "data-manager").await;
    let viewer = places::person(&world, "data-viewer").await;
    let admin = places::admin(&world, "data-admin").await;
    let mut conn = world.conn().await;
    let machine = resources::testing::insert_machine(&mut conn, "directory-report", "connected")
        .await
        .unwrap();
    let inventory = MachineDataInventory {
        observed_at: world.now().into(),
        complete: true,
        parts: vec![MachineDataPart {
            id: "sealed-part".into(),
            name: "secrets".into(),
            kind: MachineDataKind::SealedStore,
            bytes: Some(17),
            problem: None,
        }],
        unlisted_parts: 0,
        problem: None,
    };
    sqlx::query("UPDATE machines SET owner_id=$2,facts=$3 WHERE id=$1")
        .bind(machine)
        .bind(owner.id())
        .bind(json!({"data_directory":&inventory,"hostname":"metadata-only"}))
        .execute(&mut *conn)
        .await
        .unwrap();
    let mut management_entry = None;
    for (user, permission) in [
        (manager.id(), "machine.view"),
        (manager.id(), "machine.manage"),
        (viewer.id(), "machine.view"),
        (admin.id(), "machine.view"),
    ] {
        let entry = access::testing::add_entry(
            &mut conn,
            &access::PageKey::new("machine", machine, "live"),
            "any",
            permission,
            ("user", Some(user), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
        if permission == "machine.manage" {
            management_entry = Some(entry);
        }
    }
    drop(conn);
    for person in [&manager, &admin] {
        for seen in read_both(&world, person, machine).await {
            assert_eq!(seen.data_directory, Some(inventory.clone()));
            assert!(seen.facts.get("data_directory").is_none());
        }
    }

    // This manager is not the owner, so removing only this entry really withdraws Manage.
    let mut conn = world.conn().await;
    assert!(
        access::remove_entry(
            &mut conn,
            management_entry.expect("the explicit management entry was installed"),
            owner.id(),
            world.now(),
        )
        .await
        .unwrap()
    );
    access::bump_epoch(&mut conn).await.unwrap();
    drop(conn);
    places::admin_mode(&world, &admin, false).await;
    for person in [&viewer, &manager, &admin] {
        for visible in read_both(&world, person, machine).await {
            assert!(visible.data_directory.is_none());
            assert!(
                visible.facts.get("data_directory").is_none(),
                "the raw JSON field cannot bypass the typed management cut"
            );
            assert_eq!(visible.facts["hostname"], "metadata-only");
        }
    }
    places::admin_mode(&world, &admin, true).await;
    for visible in read_both(&world, &admin, machine).await {
        assert_eq!(visible.data_directory, Some(inventory.clone()));
        assert!(visible.facts.get("data_directory").is_none());
    }
}
