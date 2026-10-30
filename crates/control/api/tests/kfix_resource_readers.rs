//! The generic resource readers accept any current scoped permission without opening content.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::access::{
    EffectiveAccessGet, EffectiveAccessGetQuery, HistoryList, HistoryListQuery, PermissionPageGet,
    PermissionPageUpdate, PermissionPeopleList, PermissionPeopleListQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{CreateProvider, ProviderCreate, Upstream};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::queries::resources::access_fixtures;
use orch_db::secrets::MasterKey;
use serde_json::json;
use support::World;

#[tokio::test]
async fn access_management_alone_reads_pages_people_effective_access_and_history_of_every_resource_kind()
 {
    let world = World::new().await;
    let company = resources::company(&world).await;
    for (kind, object) in [
        ("provider", "providers"),
        ("agent", "agents"),
        ("environment", "environments"),
        ("host", "hosts"),
        ("template", "templates"),
        ("mcp_server", "mcp-servers"),
        ("mount", "mounts"),
    ] {
        let id = {
            let mut conn = world.conn().await;
            access_fixtures::insert_resource(
                &mut conn,
                kind,
                Some(company.boss.id()),
                Some(company.acme),
                "private-resource",
            )
            .await
            .unwrap()
        };
        resources::name_person(&world, kind, id, "access.own", company.boss.id()).await;
        world
            .app
            .call(PermissionPageUpdate, &[&object, &id])
            .token(&company.boss.token)
            .raw_body(&json!({"whole": "private"}))
            .send()
            .await
            .ok();
        resources::name_person(&world, kind, id, "access.manage", company.viewer.id()).await;
        // Whoever holds the row that changes the page and nothing else; no admin is such a reader: an
        // admin holds nothing as one with As admin off, and View and Manage natively with it on.
        for who in [&company.viewer] {
            let page = world
                .app
                .call(PermissionPageGet, &[&object, &id])
                .token(&who.token)
                .send()
                .await
                .ok();
            assert_eq!(page.object.id, id);
            assert!(
                page.rows
                    .iter()
                    .any(|row| row.permission == "access.manage")
            );
            let effective = world
                .app
                .call(EffectiveAccessGet, &[&object, &id])
                .token(&who.token)
                .query(&EffectiveAccessGetQuery { user_id: who.id() })
                .send()
                .await
                .ok();
            let held: Vec<&str> = effective
                .permissions
                .iter()
                .map(|held| held.permission.as_str())
                .collect();
            assert_eq!(held, ["access.manage"], "{kind}");
            macro_rules! listed_wire {
                ($endpoint:expr) => {
                    serde_json::to_value(
                        world
                            .app
                            .call($endpoint, &[])
                            .token(&who.token)
                            .send()
                            .await
                            .ok(),
                    )
                    .unwrap()
                };
            }
            let listed = match kind {
                "provider" => listed_wire!(orch_api_contract::resources::ProviderList),
                "agent" => listed_wire!(orch_api_contract::resources::AgentList),
                "environment" => listed_wire!(orch_api_contract::environment::EnvironmentList),
                "host" => listed_wire!(orch_api_contract::resources::HostList),
                "template" => listed_wire!(orch_api_contract::resources::TemplateList),
                "mcp_server" => listed_wire!(orch_api_contract::resources::McpServerList),
                "mount" => listed_wire!(orch_api_contract::mounts::MountList),
                _ => unreachable!(),
            };
            let listed = if kind == "provider" {
                &listed["items"]
            } else {
                &listed
            };
            let row = listed
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id.to_string())
                .unwrap();
            assert_eq!(row["content_hidden"], true, "{kind}");
            assert_eq!(row["private"], true, "{kind}");
            assert_eq!(row["kind"], kind);
            assert_eq!(row["name"], "private-resource");
            assert_eq!(row["parent_id"], company.acme.to_string());
            let members = row.as_object().unwrap();
            assert_eq!(
                members.len(),
                7,
                "a hidden {kind} has only its structural name and owner"
            );
            for member in [
                "status",
                "runtime",
                "config",
                "facts",
                "models",
                "profile",
                "descriptor",
                "load",
                "usage",
                "locations",
                "spec",
                "threads",
                "access",
            ] {
                assert!(
                    !members.contains_key(member),
                    "{kind} must omit {member}, including null previews and counts"
                );
            }
            macro_rules! denied_detail {
                ($endpoint:expr) => {
                    world
                        .app
                        .call($endpoint, &[&id])
                        .token(&who.token)
                        .send()
                        .await
                        .problem()
                        .code
                };
            }
            let code = match kind {
                "provider" => denied_detail!(orch_api_contract::resources::ProviderGet),
                "agent" => denied_detail!(orch_api_contract::resources::AgentGet),
                "environment" => denied_detail!(orch_api_contract::environment::EnvironmentGet),
                "host" => denied_detail!(orch_api_contract::resources::HostGet),
                "template" => denied_detail!(orch_api_contract::resources::TemplateGet),
                "mcp_server" => denied_detail!(orch_api_contract::resources::McpServerGet),
                "mount" => denied_detail!(orch_api_contract::mounts::MountGet),
                _ => unreachable!(),
            };
            assert_eq!(
                code,
                ErrorCode::AuthzDenied,
                "{kind} content is not granted by page management"
            );
            let people = world
                .app
                .call(PermissionPeopleList, &[&object, &id])
                .token(&who.token)
                .query(&PermissionPeopleListQuery {
                    holder: "admins".to_owned(),
                    limit: None,
                    cursor: None,
                })
                .send()
                .await
                .ok();
            assert!(
                people
                    .items
                    .iter()
                    .any(|person| person.id == company.admin.id())
            );
            world
                .app
                .call(HistoryList, &[&object, &id])
                .token(&who.token)
                .query(&HistoryListQuery::default())
                .send()
                .await
                .ok();
        }
        let denied = world
            .app
            .call(PermissionPageGet, &[&object, &id])
            .token(&company.stranger.token)
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::AuthzDenied, "{kind}");
    }
}

#[tokio::test]
async fn an_actual_provider_creation_leaves_its_home_admin_the_permission_page_without_provider_view()
 {
    install_master_key(MasterKey::new([7; 32]));
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = world
        .app
        .call(ProviderCreate, &[])
        .token(&company.boss.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Private key".to_owned(),
            home_id: company.acme,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("test-key".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
        .base
        .id;
    let page = world
        .app
        .call(PermissionPageGet, &[&"providers", &id])
        .token(&company.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(page.object.id, id);
    let effective = world
        .app
        .call(EffectiveAccessGet, &[&"providers", &id])
        .token(&company.admin.token)
        .query(&EffectiveAccessGetQuery {
            user_id: company.admin.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        effective
            .permissions
            .iter()
            .all(|held| held.permission != "provider.view")
    );
}
