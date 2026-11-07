//! Templates of both kinds: the ones the caller may use or manage, reading one, making one for an org or for the system, changing, switching off and deleting one that nothing was made from.
//!
//! A template is a recipe for units, environments or the runtimes of providers. Here it is data: it
//! is made, changed and kept, and no build is started. Who may make one where is the access engine's
//! to say; a driver the setting `templates.restricted_drivers` lists is saved by a system admin
//! alone, wherever the template is to be. The spec is kept as it is given: the driver of a host
//! reads it; the reported schema checks every save, and the driver also validates it before making a unit.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::nodes::{actor_name, invalid, view as node_view};
use crate::routes::resources::view::{
    Holding, PreparedAccess, accesses, accesses_prepared, addresses, parse, prepare_accesses,
};
use crate::routes::resources::{Home, LIST_LIMIT, begin_mutation, clean_name, record, require};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Held, ObjectRef};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{self as api, Template, UnitKind};
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::resources::binding as binding_rows;
use orch_db::queries::resources::routes::{self as queries, TemplateChange, TemplateRow};
use orch_db::{Conn, DbError};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

/// The longest a driver's name is: the name of a program's driver.
const DRIVER_MAX_CHARS: usize = 32;

mod collection_read;
mod creation;
mod import;
mod read;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            api::TemplateList,
            collection_read::prepare,
            collection_read::get,
        )
        .on_with_operation(
            api::TemplateCreate,
            crate::key_operations::native::effects::prepare::<creation::Templates>,
            creation::create,
        )
        .on_with_operation(
            api::TemplateImport,
            crate::key_operations::native::effects::prepare::<import::Import>,
            import::import,
        )
        .on(api::TemplateCheck, check)
        .on_with_optional_operation(api::TemplateGet, read::prepare, read::get)
        .on(api::TemplateUpdate, update)
        .on_with_operation(
            api::TemplateDelete,
            crate::key_operations::direct::declared,
            delete,
        )
        .on(api::CapabilityList, capabilities)
        .on(api::DriverList, drivers);
}

/// What the caller holds on a template, as the engine told it.
#[derive(Debug, Clone)]
struct Standing {
    admin_access: Option<orch_access_contract::roles::AdminReach>,
    admin_place: Option<orch_access_contract::roles::PlaceRef>,
    participating: bool,
    admin_only: Vec<Permission>,
    owner: bool,
    administers: bool,
    held: Vec<Held>,
}

fn unit_word(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Environment => "environment",
        UnitKind::Provider => "provider",
    }
}

/// A driver's name: lower case letters, digits, `-` and `_`.
fn clean_driver(path: &str, driver: &str) -> ApiResult<String> {
    let driver = driver.trim();
    let well_formed = !driver.is_empty()
        && driver.chars().count() <= DRIVER_MAX_CHARS
        && driver.starts_with(|first: char| first.is_ascii_lowercase())
        && driver.chars().all(|each| {
            each.is_ascii_lowercase() || each.is_ascii_digit() || matches!(each, '-' | '_')
        });
    if well_formed {
        Ok(driver.to_owned())
    } else {
        Err(invalid(
            path,
            "a driver is named in lower case letters, digits, `-` and `_`, beginning with a letter",
        ))
    }
}

/// A spec is an object: the recipe in the driver's own schema.
fn clean_spec(path: &str, spec: &Json) -> ApiResult<()> {
    if spec.is_object() {
        Ok(())
    } else {
        Err(invalid(path, "a configuration is an object"))
    }
}

/// The newest reported schema for this pool, including cached reports from offline hosts.
async fn validate_saved(conn: &mut Conn, driver: &str, kind: &str, spec: &Json) -> ApiResult<()> {
    let checked = check_spec(conn, driver, kind, spec).await?;
    if checked.valid {
        return Ok(());
    }
    Err(orch_api_contract::common::Problem::invalid(
        checked
            .errors
            .into_iter()
            .map(|error| orch_api_contract::common::FieldFault {
                path: format!("spec{}", error.pointer),
                message: error.message,
            })
            .collect(),
    )
    .into())
}

/// A preview and a save use exactly the same rules and the same newest reported schema.
async fn check_spec(
    conn: &mut Conn,
    driver: &str,
    kind: &str,
    spec: &Json,
) -> ApiResult<api::TemplateCheckReply> {
    if !spec.is_object() {
        return Ok(api::TemplateCheckReply {
            valid: false,
            errors: vec![api::TemplateSpecProblem {
                pointer: String::new(),
                message: "a configuration is an object".into(),
            }],
        });
    }
    let reported = binding_rows::host_drivers(conn).await?;
    let found = drivers_of(&reported)
        .into_iter()
        .find(|found| found.name == driver);
    let schema = found
        .as_ref()
        .and_then(|found| found.descriptor.get("spec_schemas"))
        .and_then(|schemas| schemas.get(kind))
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::SourceOffline,
                "this driver's schema for this pool has not been reported",
            )
        })?;
    let mut errors = match orch_plugin::schema::validate(schema, spec) {
        Ok(()) => Vec::new(),
        Err(orch_plugin::schema::Error::Value { faults }) => faults
            .into_iter()
            .map(|fault| api::TemplateSpecProblem {
                pointer: fault.path,
                message: fault.message,
            })
            .collect(),
        Err(orch_plugin::schema::Error::Schema { .. }) => {
            return Err(ApiError::internal("the driver's schema cannot be used"));
        }
    };
    if let Err(message) = orch_settings_contract::unit::template_commands(spec) {
        errors.push(api::TemplateSpecProblem {
            pointer: if spec.get("init").is_some_and(|value| !value.is_null()) {
                "/init"
            } else {
                "/after_up"
            }
            .into(),
            message,
        });
    } else if let Err(message) =
        orch_settings_contract::unit::template_plugins(spec, kind == "provider")
    {
        errors.push(api::TemplateSpecProblem {
            pointer: "/plugins".into(),
            message,
        });
    }
    Ok(api::TemplateCheckReply {
        valid: errors.is_empty(),
        errors,
    })
}

/// Checks public driver configuration for a signed-in writer; no resource or audit row is made.
async fn check(
    ctx: Ctx,
    (): (),
    body: api::TemplateCheckBody,
) -> ApiResult<api::TemplateCheckReply> {
    let driver = clean_driver("driver", &body.driver)?;
    let mut conn = ctx.app.db.acquire().await?;
    check_spec(&mut conn, &driver, unit_word(body.kind), &body.spec).await
}

pub(crate) struct PreparedListProjection {
    rows: Vec<TemplateRow>,
    standing: HashMap<Id, Standing>,
    access: PreparedAccess,
    sources: NameSources,
    bounds: NameBounds,
}

impl PreparedListProjection {
    pub(crate) fn name_sources(&self) -> &NameSources {
        &self.sources
    }

    pub(crate) fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
}

async fn template_views(
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<TemplateRow>,
    standing: &HashMap<Id, Standing>,
) -> ApiResult<Vec<Template>> {
    template_views_prepared(conn, principal, rows, standing, None).await
}

async fn template_views_prepared(
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<TemplateRow>,
    standing: &HashMap<Id, Standing>,
    prepared: Option<&PreparedListProjection>,
) -> ApiResult<Vec<Template>> {
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let addresses = addresses(conn, &ids).await?;
    let people: Vec<Id> = rows.iter().filter_map(|row| row.owner_id).collect();
    let people = refusals::user_refs(conn, &people).await?;
    let homes: Vec<Id> = rows.iter().filter_map(|row| row.home_id).collect();
    let homes = match prepared {
        Some(prepared) if prepared.bounds.limited => {
            node_view::refs_with_name_sources(
                conn,
                principal,
                &homes,
                Policy::default(),
                &prepared.bounds.nodes,
            )
            .await?
        }
        _ => node_view::refs(conn, principal, &homes, Policy::default()).await?,
    };
    let holdings: Vec<Holding<'_>> = rows
        .iter()
        .filter_map(|row| {
            standing.get(&row.id).map(|each| Holding {
                admin_only: &each.admin_only,
                admin_access: each.admin_access,
                admin_place: each.admin_place,
                participating: each.participating,
                id: row.id,
                owner: each.owner,
                administers: each.administers,
                held: &each.held,
            })
        })
        .collect();
    let access = match prepared {
        Some(prepared) => {
            accesses_prepared(
                conn,
                principal,
                &holdings,
                &prepared.access,
                Some(&prepared.bounds),
            )
            .await?
        }
        None => accesses(conn, principal, &holdings).await?,
    };
    rows.into_iter()
        .map(|row| {
            Ok(Template {
                id: row.id,
                address: addresses.get(&row.id).cloned().unwrap_or_default(),
                name: row.name.clone(),
                description: row.description.clone(),
                org_id: row.org_id,
                owner: row.owner_id.and_then(|owner| people.get(&owner).cloned()),
                home: row.home_id.and_then(|home| homes.get(&home).cloned()),
                kind: parse(&row.kind)?,
                driver: row.driver.clone(),
                spec: row.spec.clone(),
                units: row.units,
                enabled: row.enabled,
                revision: row.revision,
                access: access.get(&row.id).cloned().ok_or_else(|| {
                    ApiError::internal("what the caller holds on a template was not read")
                })?,
            })
        })
        .collect()
}

/// Bounded inventory rows reuse template rendering and its current permission frame.
pub(crate) async fn inventory(
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<Template>> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::templates(conn, &ids).await?;
    let standing = seen
        .iter()
        .map(|each| {
            (
                each.resource.id,
                Standing {
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    admin_only: each.admin_only.clone(),
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held.clone(),
                },
            )
        })
        .collect();
    template_views(conn, principal, rows, &standing).await
}

pub(crate) async fn prepare_list_projection(
    conn: &mut Conn,
    seen: &[resources::Seen],
) -> ApiResult<PreparedListProjection> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::templates(conn, &ids).await?;
    let standing: HashMap<_, _> = seen
        .iter()
        .map(|each| {
            (
                each.resource.id,
                Standing {
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    admin_only: each.admin_only.clone(),
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held.clone(),
                },
            )
        })
        .collect();
    prepare_rows_projection(conn, rows, standing).await
}

async fn prepare_rows_projection(
    conn: &mut Conn,
    rows: Vec<TemplateRow>,
    standing: HashMap<Id, Standing>,
) -> ApiResult<PreparedListProjection> {
    let holdings: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            standing.get(&row.id).map(|each| Holding {
                id: row.id,
                admin_access: each.admin_access,
                admin_place: each.admin_place,
                participating: each.participating,
                admin_only: &each.admin_only,
                owner: each.owner,
                administers: each.administers,
                held: &each.held,
            })
        })
        .collect();
    let access = prepare_accesses(conn, &holdings).await?;
    let mut sources = access.names.clone();
    sources
        .nodes
        .extend(rows.iter().filter_map(|row| row.home_id));
    Ok(PreparedListProjection {
        rows,
        standing,
        access,
        sources,
        bounds: NameBounds::default(),
    })
}

pub(crate) async fn show_list_prepared(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedListProjection,
) -> ApiResult<Vec<Template>> {
    template_views_prepared(
        conn,
        principal,
        prepared.rows.clone(),
        &prepared.standing,
        Some(prepared),
    )
    .await
}

async fn single_rows(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<(Vec<TemplateRow>, HashMap<Id, Standing>)> {
    let row = queries::templates(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let object = ObjectRef {
        kind: ResourceKind::Template.into(),
        id,
    };
    let effective = resources::effective(conn, principal, object, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let standing = HashMap::from([(
        id,
        Standing {
            admin_access: effective.admin_access,
            admin_place: effective.admin_place,
            participating: effective.participating,
            admin_only: effective.admin_only,
            owner: effective.owner,
            administers: effective.administers,
            held: effective.held,
        },
    )]);
    Ok((vec![row], standing))
}

pub(crate) async fn show(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Template> {
    let (rows, standing) = single_rows(conn, principal, id).await?;
    template_views(conn, principal, rows, &standing)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("a template was read and is not shown"))
}

pub(crate) async fn prepare_projection(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<PreparedListProjection> {
    let (rows, standing) = single_rows(conn, principal, id).await?;
    prepare_rows_projection(conn, rows, standing).await
}

fn home_of(row: &TemplateRow) -> Home {
    Home {
        node: row.home_id,
        org: row.org_id,
    }
}

/// Reads a template: whoever holds any permission on it.
async fn require_read(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<()> {
    queries::templates(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    require(
        conn,
        principal,
        ResourceKind::Template,
        id,
        Permission::TemplateView,
    )
    .await?;
    Ok(())
}

async fn restricted_driver(conn: &mut Conn, driver: &str) -> ApiResult<bool> {
    let restricted: Vec<String> = setting(conn, Place::System, keys::TEMPLATES_RESTRICTED_DRIVERS)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    Ok(restricted.iter().any(|name| name == driver))
}

/// The same creation and restricted-driver checks for a proposal and a saved template.
async fn creation_authority(
    conn: &mut Conn,
    principal: &Principal,
    home: Option<Id>,
    driver: &str,
) -> ApiResult<Home> {
    let selected = super::creation_home::require(
        conn,
        principal,
        ResourceKind::Template,
        home,
        "a template is at home in an org",
        "only a system admin makes a template of the system",
    )
    .await?;
    if restricted_driver(conn, driver).await?
        && !orch_authz::system_administration::on_system(
            principal,
            orch_access_contract::key_scope::KeyTargetKind::Object(
                orch_access_contract::permissions::ObjectKind::Template,
            ),
        )
    {
        return Err(refusals::system_admin_only(
            "this driver is kept to the admins of the system",
        ));
    }
    Ok(selected)
}

async fn stale(conn: &mut Conn, principal: &Principal, id: Id) -> ApiError {
    let current = show(conn, principal, id).await.ok();
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the template was changed since it was read",
    )
    .with("current", current)
}

/// Changes a template; running units are not affected until they are updated.
async fn update(ctx: Ctx, (): (), body: api::TemplateUpdateBody) -> ApiResult<Template> {
    let id = ctx.id("id")?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    if let Some(spec) = &body.spec {
        clean_spec("spec", spec)?;
    }
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = queries::templates(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    // Only what differs is a change: asking for what is there changes nothing, and moves nothing.
    let change = TemplateChange {
        name: name.as_deref().filter(|name| *name != row.name),
        description: body
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| *description != row.description),
        spec: body.spec.as_ref().filter(|spec| **spec != row.spec),
        enabled: body.enabled.filter(|enabled| *enabled != row.enabled),
    };
    if let Some(spec) = change.spec {
        validate_saved(&mut tx, &row.driver, &row.kind, spec).await?;
    }
    if change == TemplateChange::default() {
        if row.revision != body.expected_revision {
            return Err(stale(&mut tx, &principal, id).await);
        }
        return show(&mut tx, &principal, id).await;
    }
    if !queries::change_template(&mut tx, id, body.expected_revision, &change, ctx.now).await? {
        return Err(stale(&mut tx, &principal, id).await);
    }
    let who = actor_name(&ctx)?;
    if change.name.is_some() || change.description.is_some() || change.spec.is_some() {
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::TemplateUpdated),
            id,
            home_of(&row),
            &format!("{who} changed the template {}", row.name),
            json!({
                "template": id,
                "name": change.name,
                "description": change.description.is_some(),
                "spec": change.spec.is_some(),
            }),
        )
        .await?;
    }
    if let Some(enabled) = change.enabled {
        let (action, word) = if enabled {
            (
                orch_db::audit::action!(AuditAction::TemplateEnabled),
                "switched on",
            )
        } else {
            (
                orch_db::audit::action!(AuditAction::TemplateDisabled),
                "switched off",
            )
        };
        record(
            &ctx,
            &mut tx,
            action,
            id,
            home_of(&row),
            &format!("{who} {word} the template {}", row.name),
            json!({ "template": id }),
        )
        .await?;
    }
    let changed = show(&mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(changed)
}

/// Deletes a template that nothing was made from.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, _) = begin_mutation(&ctx).await?;
    let row = queries::templates(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if row.units > 0 {
        let problem = ApiError::new(
            ErrorCode::ResourceInUse,
            "units were made from this template and still stand",
        );
        return Err(if ctx.key_receipt_required() {
            authorization::problem_without_content(problem)
        } else {
            problem.with("units", row.units)
        });
    }
    queries::delete_template(&mut tx, id, ctx.now).await?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::TemplateDeleted),
        id,
        home_of(&row),
        &format!("{who} deleted the template {}", row.name),
        json!({ "template": id, "name": row.name }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ---- what the dialogs of a template and of an environment offer: capabilities and drivers ----

/// The capabilities an environment may be given, by key.
async fn capabilities(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::CapabilityDef>> {
    let mut conn = ctx.app.db.acquire().await?;
    binding_rows::capability_defs(&mut conn)
        .await?
        .into_iter()
        .map(|row| {
            Ok(api::CapabilityDef {
                driver_patch: row
                    .driver_patch
                    .as_object()
                    .map(|patches| {
                        patches
                            .iter()
                            .map(|(driver, patch)| (driver.clone(), patch.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                risk: parse(&row.risk)?,
                default_decision: parse(&row.default_decision)?,
                default_approver_scope: parse(&row.default_approver_scope)?,
                key: row.key,
                label: row.label,
                description: row.description,
                builtin: row.builtin,
                revision: row.revision,
            })
        })
        .collect()
}

/// The numbers a version is made of, to put versions in order: `1.10.0` comes after `1.9.0`.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|each: char| !each.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(u64::MAX))
        .collect()
}

/// The drivers the hosts report: for each name, how many hosts have it and what the newest build
/// of it declares. A host reports its drivers by name; two hosts may run two builds of one.
fn drivers_of(reported: &[Json]) -> Vec<api::DriverInfo> {
    let mut found: BTreeMap<String, (i64, Json)> = BTreeMap::new();
    for host in reported {
        let Some(drivers) = host.as_object() else {
            continue;
        };
        for (key, descriptor) in drivers {
            let name = descriptor
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or(key)
                .to_owned();
            let version = |each: &Json| {
                version_key(
                    each.get("version")
                        .and_then(Json::as_str)
                        .unwrap_or_default(),
                )
            };
            let entry = found.entry(name).or_insert_with(|| (0, descriptor.clone()));
            entry.0 += 1;
            if version(descriptor) > version(&entry.1) {
                entry.1 = descriptor.clone();
            }
        }
    }
    found
        .into_iter()
        .map(|(name, (hosts, descriptor))| api::DriverInfo {
            title: descriptor
                .get("title")
                .and_then(Json::as_str)
                .unwrap_or(&name)
                .to_owned(),
            name,
            hosts,
            descriptor,
        })
        .collect()
}

/// The drivers this system's hosts report, each with the descriptor the form of a template is
/// drawn from.
async fn drivers(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::DriverInfo>> {
    let mut conn = ctx.app.db.acquire().await?;
    let reported = binding_rows::host_drivers(&mut conn).await?;
    Ok(drivers_of(&reported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_driver_is_named_like_a_program_and_a_spec_is_an_object() {
        for good in ["docker", "host", "k8s-job", "my_driver"] {
            assert!(clean_driver("driver", good).is_ok(), "{good}");
        }
        for bad in ["", "Docker", "9p", "has space", &"d".repeat(33)] {
            assert!(clean_driver("driver", bad).is_err(), "{bad}");
        }
        assert!(clean_spec("spec", &json!({})).is_ok());
        assert!(clean_spec("spec", &json!([1])).is_err());
    }
}
