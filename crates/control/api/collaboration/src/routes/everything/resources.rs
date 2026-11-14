//! Resource-list filters and bounded rich rows, using the inventory's own frame and existing readers.

use std::collections::HashMap;

use std::collections::HashSet;

use crate::routes::resources::agents;

use crate::routes::resources::hosts;

use crate::routes::resources::machines;

use crate::routes::resources::mcp_readers;

use crate::routes::resources::mounts;

use crate::routes::resources::providers;

use crate::routes::resources::templates;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::everything::EverythingListQuery;

use orch_api_contract::everything::EverythingResource;

use orch_api_contract::everything::EverythingSort;

use orch_authz::Principal;

use orch_authz::everything::Projection;

use orch_authz::resources::Seen;

use orch_db::Conn;

use orch_db::queries::everything::Grant;

use orch_db::queries::everything::Row;

use orch_types::{Id, Time};
/// These projections affect the shared SQL filter/order before its LIMIT, never a client-side slice.
pub(super) async fn prepare(
    conn: &mut Conn,
    query: &EverythingListQuery,
    projection: &mut Projection,
) -> ApiResult<()> {
    if query.resource_details == Some(true) || query.resource_rows == Some(true) {
        let names: std::collections::BTreeMap<_, _> = projection
            .names
            .iter()
            .filter_map(|(id, step)| step.name.as_ref().map(|name| (id.to_string(), name)))
            .collect();
        projection.grants["resource_names"] =
            serde_json::to_value(names).map_err(ApiError::internal)?;
    }
    if query.sort == Some(EverythingSort::Load) {
        let ids: Vec<_> = projection
            .resources
            .iter()
            .filter(|each| {
                each.resource.kind == ResourceKind::Host
                    && each
                        .held
                        .iter()
                        .any(|held| held.permission == Permission::HostView)
            })
            .map(|each| each.resource.id)
            .collect();
        projection.grants["host_loads"] = hosts::inventory_loads(conn, &ids).await?;
    }
    Ok(())
}

/// Only the selected readable page is hydrated. No whole-list reader or pooled acquire runs here.
pub(super) async fn page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    kind: &str,
    rows: &[Row],
    grants: &[Grant],
    projection: &Projection,
) -> ApiResult<HashMap<String, EverythingResource>> {
    let ids: HashSet<Id> = rows
        .iter()
        .zip(grants)
        .filter(|(_, grant)| grant.view)
        .map(|(row, _)| row.id.parse().map_err(ApiError::internal))
        .collect::<ApiResult<_>>()?;
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let seen: Vec<Seen> = projection
        .resources
        .iter()
        .filter(|each| each.resource.kind.as_str() == kind && ids.contains(&each.resource.id))
        .cloned()
        .collect();
    let values: Vec<EverythingResource> = match kind {
        "agent" => agents::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Agent(Box::new(row)))
            .collect(),
        "provider" => providers::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Provider(Box::new(row)))
            .collect(),
        "host" => hosts::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Host(Box::new(row)))
            .collect(),
        "environment" => crate::routes::environments::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Environment(Box::new(row)))
            .collect(),
        "template" => templates::inventory(conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Template(Box::new(row)))
            .collect(),
        "mcp_server" => mcp_readers::views(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::McpServer(Box::new(row)))
            .collect(),
        "machine" => {
            let ids: Vec<_> = ids.into_iter().collect();
            let rows =
                orch_db::queries::resources::routes::physical_machines_among(conn, &ids).await?;
            machines::machine_views(ctx, conn, rows)
                .await?
                .into_iter()
                .map(|row| EverythingResource::Machine(Box::new(row)))
                .collect()
        }
        "skill" => crate::routes::resources::skills::inventory(conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Skill(Box::new(row)))
            .collect(),
        "mount" => {
            // Location visibility and source-removal policy stay in the mount's existing reader.
            let mut values = Vec::with_capacity(ids.len());
            for id in ids {
                values.push(EverythingResource::Mount(Box::new(
                    mounts::show(ctx, conn, principal, id).await?,
                )));
            }
            values
        }
        _ => return Err(ApiError::invalid("kinds", "Choose one resource kind.")),
    };
    Ok(keyed(values))
}

/// Only the facts the list draws or needs to offer its current acts. Editors read the native full view.
pub(super) fn list_row(
    view: &EverythingResource,
) -> orch_api_contract::resource_rows::ResourceListRow {
    use orch_api_contract::resource_rows::*;
    match view {
        EverythingResource::Provider(provider) => {
            let mut row = ProviderListRow::from(provider.as_ref());
            let mut thin = provider.as_ref().clone();
            thin.thin_for_list();
            row.facts = thin.facts;
            row.choice_usage = thin.choice_usage;
            ResourceListRow::Provider(Box::new(row))
        }
        EverythingResource::Agent(agent) => {
            let mut row = AgentListRow::from(agent.as_ref());
            row.profile =
                serde_json::json!({"harness_version":agent.profile.get("harness_version")});
            ResourceListRow::Agent(Box::new(row))
        }
        EverythingResource::Environment(environment) => {
            ResourceListRow::Environment(Box::new(EnvironmentListRow::from(environment.as_ref())))
        }
        EverythingResource::Host(host) => {
            let mut row = HostListRow::from(host.as_ref());
            for driver in &mut row.drivers {
                driver.runtimes.clear();
                driver.runtime_classes.clear();
                driver.availability.clear();
                driver.default_runtime = None;
            }
            ResourceListRow::Host(Box::new(row))
        }
        EverythingResource::Machine(machine) => {
            ResourceListRow::Machine(Box::new(MachineListRow::from(machine.as_ref())))
        }
        EverythingResource::Template(template) => {
            let mut row = TemplateListRow::from(template.as_ref());
            row.spec = serde_json::json!({"image":template.spec.get("image"),"runtime":template.spec.get("runtime"),
                "network":{"mode":template.spec.pointer("/network/mode")},
                "security":{"read_only_rootfs":template.spec.pointer("/security/read_only_rootfs"),"no_new_privileges":template.spec.pointer("/security/no_new_privileges")}});
            if template
                .spec
                .get("build")
                .is_some_and(serde_json::Value::is_object)
            {
                row.spec["build"] = serde_json::json!({});
            }
            if template
                .spec
                .get("archive")
                .is_some_and(serde_json::Value::is_object)
            {
                row.spec["archive"] = serde_json::json!({});
            }
            ResourceListRow::Template(Box::new(row))
        }
        EverythingResource::McpServer(server) => {
            let mut row = McpServerListRow::from(server.as_ref());
            row.spec = serde_json::json!({"transport":server.spec.get("transport")});
            ResourceListRow::McpServer(Box::new(row))
        }
        EverythingResource::Mount(mount) => {
            ResourceListRow::Mount(Box::new(MountListRow::from(mount.as_ref())))
        }
        EverythingResource::Skill(skill) => {
            ResourceListRow::Skill(Box::new(SkillListRow::from(skill.as_ref())))
        }
    }
}

fn keyed(values: Vec<EverythingResource>) -> HashMap<String, EverythingResource> {
    values
        .into_iter()
        .map(|value| {
            let id = match &value {
                EverythingResource::Agent(row) => row.base.id,
                EverythingResource::Provider(row) => row.base.id,
                EverythingResource::Host(row) => row.base.id,
                EverythingResource::Environment(row) => row.base.id,
                EverythingResource::Machine(row) => row.base.id,
                EverythingResource::Mount(row) => row.base.id,
                EverythingResource::Template(row) => row.id,
                EverythingResource::McpServer(row) => row.id,
                EverythingResource::Skill(row) => row.id,
            };
            (id.to_string(), value)
        })
        .collect()
}

#[path = "resources/scoped.rs"]
pub mod scoped;

/// The home's actual page is prepared before its names and groups join the inventory's source plan.
pub(in crate::routes::everything) struct PreparedSharing {
    pub page: orch_authz::pages::PageView,
    pub root: Option<Id>,
    pub names: crate::routes::access::page::NameBounds,
}

impl PreparedSharing {
    pub(in crate::routes::everything) fn name_sources(
        &self,
    ) -> crate::routes::access::page::NameSources {
        let mut entries = Vec::new();
        for row in &self.page.rows {
            entries.extend(row.entries.iter().cloned());
            if let Some(inherited) = &row.inherited {
                entries.extend(inherited.iter().map(|entry| entry.entry.clone()));
            }
        }
        crate::routes::access::page::entry_name_sources(&entries, self.page.role_place)
    }
}

pub(in crate::routes::everything) async fn prepare_sharing(
    conn: &mut Conn,
    principal: &Principal,
    home: Id,
    kind: &str,
) -> ApiResult<PreparedSharing> {
    crate::middleware::authorization::require_projected(
        conn,
        principal,
        home,
        crate::middleware::authorization::VIEW,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
        crate::key_operations::native::modern(principal.caller()),
    )
    .await?;
    let (_, site) = orch_authz::things::site(conn, orch_authz::things::Thing::Node(home))
        .await
        .map_err(ApiError::internal)?;
    let mut page = orch_authz::pages::read(
        conn,
        principal,
        orch_authz::pages::Target::Defaults(orch_access_contract::roles::PlaceRef::Node(home)),
        orch_authz::Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let kind = kind
        .parse::<orch_access_contract::permissions::ObjectKind>()
        .map_err(ApiError::internal)?;
    page.rows.retain(|row| row.permission.on().contains(&kind));
    Ok(PreparedSharing {
        page,
        root: site.root,
        names: Default::default(),
    })
}

/// A list shares one compact defaults summary per visible home; a refusal belongs to that heading only.
pub(super) async fn sharing(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    home: Id,
    kind: &str,
    prepared: Option<&PreparedSharing>,
) -> orch_api_contract::resource_rows::ResourceSharing {
    let result: ApiResult<orch_api_contract::access::PermissionPage> = async {
        if let Some(prepared) = prepared {
            let dependencies = Default::default();
            return crate::routes::access::page::permission_page_bounded(
                conn,
                principal,
                &prepared.page,
                prepared.root,
                Time::new(ctx.now),
                &prepared.names,
                &dependencies,
            )
            .await;
        }
        if crate::key_operations::native::modern(principal.caller()) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "This key cannot read the home's sharing summary.",
            ));
        }
        let prepared = prepare_sharing(conn, principal, home, kind).await?;
        crate::routes::access::page::permission_page(
            conn,
            principal,
            &prepared.page,
            prepared.root,
            Time::new(ctx.now),
        )
        .await
    }
    .await;
    match result {
        Ok(defaults) => orch_api_contract::resource_rows::ResourceSharing {
            home_id: home,
            rows: Some(defaults.rows),
            problem: None,
        },
        Err(error) => {
            let (problem, _) = error.into_parts_for(&ctx.request_id);
            orch_api_contract::resource_rows::ResourceSharing {
                home_id: home,
                rows: None,
                problem: Some(problem),
            }
        }
    }
}
