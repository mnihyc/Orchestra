//! One native Mount factory is bounded under Get, List and Inventory's own operations.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::everything::{EverythingList, EverythingListQuery};
use orch_api_contract::mounts::{
    MountCreate, MountCreateBody, MountGet, MountList, MountListQuery,
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::mounts::{self as rows, LocationRow};
use orch_db::queries::{identity, resources::access_fixtures};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const OPERATIONS: [&str; 3] = ["MountGet", "MountList", "EverythingList.mine"];

#[derive(Clone, Copy)]
enum Entry {
    Get,
    List,
    Inventory,
}

struct Scene {
    world: World,
    company: resources::Company,
    mount: Id,
    host: Id,
    machine: Id,
}

impl Scene {
    async fn new(host_visible: bool) -> Self {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let mount = world
            .app
            .call(MountCreate, &[])
            .token(&company.viewer.token)
            .body(&MountCreateBody {
                name: "family-data".into(),
                description: Some("Intrinsic Mount description".into()),
                home_id: Some(company.acme),
                target: "/mnt/family".into(),
                read_only: Some(true),
                default_fate: None,
                access: Some(NewResourceAccess::OnlyMe),
            })
            .send()
            .await
            .ok()
            .base
            .id;
        let mut conn = world.conn().await;
        let host = access_fixtures::insert_resource(
            &mut conn,
            "host",
            Some(company.admin.id()),
            Some(company.acme),
            "Family Host",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Host.into(),
                id: host,
            },
            company.admin.id(),
            NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
            .bind(host)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        rows::set_location(
            &mut conn,
            &LocationRow {
                mount_id: mount,
                machine_id: machine,
                host_id: None,
                host_name: None,
                kind: "volume".into(),
                source: "private-volume-source".into(),
                storage: None,
                data_kind: None,
                created_by: company.admin.id(),
                created_at: world.now(),
            },
        )
        .await
        .unwrap();
        drop(conn);
        if host_visible {
            resources::name_person(&world, "host", host, "host.view", company.viewer.id()).await;
        }
        Self {
            world,
            company,
            mount,
            host,
            machine,
        }
    }

    async fn key(&self, rules: Vec<Json>) -> String {
        self.key_for(self.company.viewer.id(), rules).await
    }

    async fn key_for(&self, user: Id, rules: Vec<Json>) -> String {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        let secret = new_token(API_TOKEN_PREFIX);
        identity::create_api_token_scoped(
            &mut *self.world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: user,
                name: "Native family control",
                token_hash: &token_hash(&secret),
                scopes: &[],
                node_id: None,
                at: self.world.now(),
                expires_at: Some(self.world.now() + chrono::Duration::hours(1)),
            },
            Some(&serde_json::to_value(scope).unwrap()),
        )
        .await
        .unwrap();
        secret
    }

    async fn legacy_key(&self) -> String {
        let secret = new_token(API_TOKEN_PREFIX);
        identity::create_api_token_scoped(
            &mut *self.world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: self.company.viewer.id(),
                name: "Native legacy source control",
                token_hash: &token_hash(&secret),
                scopes: &["mount.view".into(), "host.view".into()],
                node_id: None,
                at: self.world.now(),
                expires_at: Some(self.world.now() + chrono::Duration::hours(1)),
            },
            None,
        )
        .await
        .unwrap();
        secret
    }

    fn rules(&self, machine_kind: &str, machine_id: Id, operations: &[&str]) -> Vec<Json> {
        vec![
            rule("mount", self.mount, "mount", operations),
            rule(machine_kind, machine_id, "machine", operations),
        ]
    }

    async fn read(&self, entry: Entry, token: &str) -> RawReply {
        match entry {
            Entry::Get => {
                self.world
                    .app
                    .call(MountGet, &[&self.mount])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Entry::List => {
                self.world
                    .app
                    .call(MountList, &[])
                    .token(token)
                    .query(&MountListQuery {
                        org_id: Some(self.company.acme),
                    })
                    .send_bytes()
                    .await
            }
            Entry::Inventory => {
                self.world
                    .app
                    .call(EverythingList, &[])
                    .token(token)
                    .query(&EverythingListQuery {
                        parent: Some("resources".into()),
                        kinds: Some("mount".into()),
                        resource_details: Some(true),
                        ..Default::default()
                    })
                    .send_bytes()
                    .await
            }
        }
    }

    fn item(&self, entry: Entry, reply: RawReply) -> Json {
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let value: Json = serde_json::from_slice(&reply.raw).unwrap();
        match entry {
            Entry::Get => value,
            Entry::List => value
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == self.mount.to_string())
                .unwrap()
                .clone(),
            Entry::Inventory => value["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == self.mount.to_string())
                .unwrap()["resource"]["value"]
                .clone(),
        }
    }
}

fn rule(kind: &str, id: Id, business: &str, operations: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[business],
        "allow":operations.iter().map(|operation| json!({"operation":operation,"actions":["read"]})).collect::<Vec<_>>()})
}

#[tokio::test]
async fn mount_get_list_and_inventory_share_intrinsic_content_and_canonical_physical_source_bounds()
{
    let s = Scene::new(true).await;
    let allowed = s.key(s.rules("host", s.host, &OPERATIONS)).await;
    let literal_machine = s.key(s.rules("machine", s.machine, &OPERATIONS)).await;
    for entry in [Entry::Get, Entry::List, Entry::Inventory] {
        let browser = s.item(entry, s.read(entry, &s.company.viewer.token).await);
        let key = s.item(entry, s.read(entry, &allowed).await);
        for field in [
            "description",
            "target",
            "read_only",
            "locations",
            "kept",
            "lost_locations",
            "attached_environments",
            "locations_hidden",
            "revision",
        ] {
            assert_eq!(key[field], browser[field], "native {field}");
        }
        assert_eq!(key["locations"][0]["machine"]["id"], s.machine.to_string());
        assert!(
            key["locations"][0]["source"].is_null(),
            "Host View does not reveal a native location source path"
        );
        assert_eq!(
            s.read(entry, &literal_machine).await.status,
            StatusCode::FORBIDDEN,
            "the physical Machine id cannot substitute for its Host governor"
        );
    }
}

#[tokio::test]
async fn native_hidden_locations_are_not_invented_as_family_sources_and_new_visibility_requires_real_coverage()
 {
    let s = Scene::new(false).await;
    let mount_only = s
        .key(vec![rule("mount", s.mount, "mount", &OPERATIONS)])
        .await;
    for entry in [Entry::Get, Entry::List, Entry::Inventory] {
        let value = s.item(entry, s.read(entry, &mount_only).await);
        assert_eq!(value["locations_hidden"], 1);
        assert!(value["locations"].as_array().unwrap().is_empty());
    }
    resources::name_person(&s.world, "host", s.host, "host.view", s.company.viewer.id()).await;
    for entry in [Entry::Get, Entry::List, Entry::Inventory] {
        assert_eq!(
            s.read(entry, &mount_only).await.status,
            StatusCode::FORBIDDEN,
            "an actually emitted source must be admitted under the same operation"
        );
    }
}

#[tokio::test]
async fn native_family_entry_points_do_not_borrow_another_operations_read_or_add_account_authority()
{
    let s = Scene::new(true).await;
    let only_get = s.key(s.rules("host", s.host, &["MountGet"])).await;
    assert_eq!(s.read(Entry::Get, &only_get).await.status, StatusCode::OK);
    for entry in [Entry::List, Entry::Inventory] {
        assert_eq!(s.read(entry, &only_get).await.status, StatusCode::FORBIDDEN);
    }
    let powerless = s
        .key_for(
            s.company.stranger.id(),
            s.rules("host", s.host, &OPERATIONS),
        )
        .await;
    assert_eq!(
        s.read(Entry::Get, &powerless).await.status,
        StatusCode::FORBIDDEN,
        "complete operation scope cannot add the ordinary Mount View authority"
    );
    // The scene's key owner still has only ordinary Host View, never Host Manage. No shared
    // family adapter exposes the private source path merely because all Read facets pass.
    let allowed = s.key(s.rules("host", s.host, &OPERATIONS)).await;
    for entry in [Entry::Get, Entry::List, Entry::Inventory] {
        let value = s.item(entry, s.read(entry, &allowed).await);
        assert!(value["locations"][0]["source"].is_null());
        assert_eq!(value["locations"][0]["can_remove"], false);
    }
}

#[tokio::test]
async fn ordinary_legacy_mount_reads_keep_their_native_foreign_source_door_without_new_operation_facets()
 {
    let s = Scene::new(true).await;
    let legacy = s.legacy_key().await;
    for entry in [Entry::Get, Entry::List, Entry::Inventory] {
        let browser = s.item(entry, s.read(entry, &s.company.viewer.token).await);
        let value = s.item(entry, s.read(entry, &legacy).await);
        for field in [
            "description",
            "target",
            "locations",
            "kept",
            "locations_hidden",
            "attached_environments",
        ] {
            assert_eq!(value[field], browser[field], "native legacy {field}");
        }
        assert_eq!(
            value["locations"][0]["machine"]["id"],
            s.machine.to_string()
        );
        assert!(value["locations"][0]["source"].is_null());
    }
}
