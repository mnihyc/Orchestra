//! Agent profiles: the ones the caller may use or manage, reading one, making one in an org, changing, switching off and deleting one that no thread is bound to.
//!
//! A profile says which harness to run and how; it has no runtime of its own. Whoever writes one
//! is trusted by every provider it may be bound to, so a profile is made by whoever makes the
//! rules of its org, and the access engine decides that with every other question here: who sees
//! a profile, who may use it, who manages it, and what a refusal says is missing.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::admin::catalogue::builds_listed;
use crate::routes::downloads::payload_dir;
use crate::routes::nodes::{actor_name, invalid, placed, view as node_view};
use crate::routes::resources::machines::registrable_machine as own_machine;
use crate::routes::resources::providers::registration_home;
use crate::routes::resources::view::{Holding, accesses, addresses, machine_refs};
use crate::routes::resources::{Home, LIST_LIMIT, begin_mutation, clean_name, record, require};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Held, ObjectRef};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_agent_contract::facts::AgentProfile;
use orch_api_contract::admin::HarnessBuild;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::resources::{self as api, Agent, AgentRef};
use orch_authz::nodes::Page;
use orch_authz::ownership::{self, NewAccess};
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::resources::binding::{self as binding_rows, AgentKindRow};
use orch_db::queries::resources::routes::{self as queries, AgentChange, AgentRow, NewAgent};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The members an agent profile may have: which build to run, how the harness is reached, what
/// is merged over the adapter's configuration and the variables it finds.
const PROFILE_MEMBERS: [&str; 4] = ["harness_version", "attach", "config", "env"];

/// The longest a harness kind is: a name of an adapter.
const HARNESS_KIND_MAX_CHARS: usize = 64;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::AgentList, list)
        .on(api::OrgAgentCreate, create)
        .on(api::AgentGet, get)
        .on(api::AgentUpdate, update)
        .on(api::AgentDelete, delete)
        .on(api::HarnessKindList, harness_kinds)
        .on(api::HarnessExecutorList, harness_executors)
        .on(api::MachineAgentCreate, machine_agent_create);
}

/// The profile as the API reads a body: only the members a profile has, and the right kind of
/// each. The adapter declares how it can be reached, independently of the machine's kind.
fn clean_profile(path: &str, profile: &Json, _machine: bool) -> ApiResult<Json> {
    let Some(members) = profile.as_object() else {
        return Err(invalid(path, "a profile is an object"));
    };
    if let Some(unknown) = members
        .keys()
        .find(|name| !PROFILE_MEMBERS.contains(&name.as_str()))
    {
        return Err(invalid(
            &format!("{path}.{unknown}"),
            "a profile has no such member",
        ));
    }
    let mut schema =
        serde_json::to_value(schemars::schema_for!(AgentProfile)).map_err(ApiError::internal)?;
    schema["additionalProperties"] = json!(false);
    schema["properties"]["config"] = json!({"type":["object","null"]});
    if let Some(attach) = schema.pointer_mut("/$defs/Attach") {
        attach["additionalProperties"] = json!(false);
    }
    super::validation::value(path, &schema, profile)?;
    let parsed: AgentProfile = serde_json::from_value(profile.clone())
        .map_err(|_| invalid(path, "a profile member has the wrong type"))?;
    if parsed
        .attach
        .as_ref()
        .and_then(|attach| attach.home.as_ref())
        .and_then(|home| home.directory())
        .is_some_and(|path| !std::path::Path::new(path).is_absolute())
    {
        return Err(invalid(
            &format!("{path}.attach.home.path"),
            "an existing harness home is an absolute path checked against the runtime machine's environment.roots",
        ));
    }
    serde_json::to_value(&parsed).map_err(ApiError::internal)
}

/// A harness kind as an adapter names itself: lower case, digits, `-` and `_`.
fn clean_harness_kind(path: &str, kind: &str) -> ApiResult<String> {
    let kind = kind.trim();
    let well_formed = !kind.is_empty()
        && kind.chars().count() <= HARNESS_KIND_MAX_CHARS
        && kind.starts_with(|first: char| first.is_ascii_lowercase())
        && kind.chars().all(|each| {
            each.is_ascii_lowercase() || each.is_ascii_digit() || matches!(each, '-' | '_')
        });
    if well_formed {
        Ok(kind.to_owned())
    } else {
        Err(invalid(
            path,
            "a harness kind is lower case letters, digits, `-` and `_`, beginning with a letter",
        ))
    }
}

/// The descriptor a profile shows: what a harness last declared, with the members the system
/// sets over it replacing the same members; and the names of those members.
fn descriptor_shown(descriptor: &Json, overrides: &Json) -> (Json, Vec<String>) {
    let Some(set) = overrides.as_object().filter(|set| !set.is_empty()) else {
        return (descriptor.clone(), Vec::new());
    };
    let mut shown = descriptor.as_object().cloned().unwrap_or_default();
    let mut names: BTreeSet<String> = BTreeSet::new();
    for (name, value) in set {
        shown.insert(name.clone(), value.clone());
        names.insert(name.clone());
    }
    // Show the same recursive, typed merge used by execution and write validation.
    // A legacy invalid value remains visible in the raw fallback so it can be repaired.
    let effective = serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(
        descriptor.clone(),
    )
    .and_then(|base| base.with_overrides(overrides))
    .and_then(serde_json::to_value);
    if let Ok(effective) = effective {
        return (effective, names.into_iter().collect());
    }
    (Json::Object(shown), names.into_iter().collect())
}

/// What the caller holds on a profile, as the engine told it.
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

/// Profiles as the API shows them, for any number of them in the same few statements.
async fn agent_views(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<AgentRow>,
    standing: &HashMap<Id, Standing>,
) -> ApiResult<Vec<Agent>> {
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let addresses = addresses(conn, &ids).await?;
    let people: Vec<Id> = rows.iter().filter_map(|row| row.owner_id).collect();
    let people = refusals::user_refs(conn, &people).await?;
    let homes: Vec<Id> = rows.iter().filter_map(|row| row.home_id).collect();
    let homes = node_view::refs(conn, principal, &homes, Policy::default()).await?;
    let machines: Vec<Id> = rows.iter().filter_map(|row| row.machine_id).collect();
    let machines = machine_refs(&ctx.app, conn, &machines).await?;
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
    let access = accesses(conn, principal, &holdings).await?;
    let associations: Vec<(Id, Id)> = sqlx::query_as(
        "SELECT agent_id,id FROM providers WHERE agent_id=ANY($1) AND deleted_at IS NULL
         UNION SELECT DISTINCT t.agent_id,t.provider_id FROM threads t JOIN nodes n ON n.id=t.node_id
         WHERE t.agent_id=ANY($1) AND t.provider_id IS NOT NULL AND n.deleted_at IS NULL",
    ).bind(&ids).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let provider_ids: Vec<Id> = associations.iter().map(|(_, provider)| *provider).collect();
    let readable: Vec<Id> =
        resources::effective_among(conn, principal, &provider_ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .filter(|holding| {
                holding
                    .held
                    .iter()
                    .any(|held| held.permission == Permission::ProviderView)
            })
            .map(|holding| holding.object.id)
            .collect();
    let providers = crate::routes::resources::view::provider_refs(conn, &readable, &[]).await?;
    let models = queries::models_of(conn, &readable).await?;
    rows.into_iter()
        .map(|row| {
            let (descriptor, overridden) =
                descriptor_shown(&row.descriptor, &row.descriptor_overrides);
            Ok(Agent {
                providers: Some(
                    providers
                        .iter()
                        .filter(|(_, provider)| associations.contains(&(row.id, provider.id)))
                        .map(|(_, provider)| provider.clone())
                        .collect(),
                ),
                models: Some(
                    models
                        .iter()
                        .filter(|model| associations.contains(&(row.id, model.provider_id)))
                        .map(|model| {
                            Ok(api::AgentModel {
                                provider_id: model.provider_id,
                                model: crate::routes::resources::providers::model_view(model)?,
                            })
                        })
                        .collect::<ApiResult<_>>()?,
                ),
                base: AgentRef {
                    address: addresses.get(&row.id).cloned().unwrap_or_default(),
                    machine: row
                        .machine_id
                        .and_then(|machine| machines.get(&machine).cloned()),
                    id: row.id,
                    name: row.name.clone(),
                    harness: crate::programs::shipped_name(
                        &row.harness_kind,
                        Some(&row.descriptor),
                    ),
                    harness_kind: row.harness_kind.clone(),
                    disabled: row.disabled,
                },
                owner: row.owner_id.and_then(|owner| people.get(&owner).cloned()),
                org_id: row.org_id,
                home: row.home_id.and_then(|home| homes.get(&home).cloned()),
                profile: row.profile.clone(),
                descriptor,
                descriptor_overridden: overridden,
                revision: row.revision,
                created_at: Time::new(row.created_at),
                updated_at: Time::new(row.updated_at),
                access: access.get(&row.id).cloned().ok_or_else(|| {
                    ApiError::internal("what the caller holds on a profile was not read")
                })?,
            })
        })
        .collect()
}

/// Bounded inventory rows reuse the very same bulk view and standings from its permission frame.
pub(crate) async fn inventory(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<Agent>> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::agents(conn, &ids).await?;
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
    agent_views(ctx, conn, principal, rows, &standing).await
}

/// One profile as the caller sees it, with what they hold on it. The caller holds something on
/// it already, or this is not asked.
pub(crate) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Agent> {
    let row = queries::agents(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let object = ObjectRef {
        kind: ResourceKind::Agent.into(),
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
    agent_views(ctx, conn, principal, vec![row], &standing)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("a profile was read and is not shown"))
}

/// The profiles the caller may use or manage.
async fn list(
    ctx: Ctx,
    query: api::AgentListQuery,
    (): (),
) -> ApiResult<Vec<orch_api_contract::common::Listed<Agent>>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let page = Page {
        after: None,
        limit: LIST_LIMIT,
    };
    let seen = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Agent,
        page,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    // To use or to manage: looking at a profile is not enough to be offered it.
    let mut seen: Vec<_> = seen
        .items
        .into_iter()
        .filter(|each| {
            each.administers
                || each.held.iter().any(|held| {
                    matches!(
                        held.permission,
                        Permission::AgentView
                            | Permission::AgentUse
                            | Permission::AgentManage
                            | Permission::AccessManage
                    )
                })
        })
        .collect();
    if let Some(org) = query.org_id {
        let homes: Vec<Id> = seen.iter().filter_map(|each| each.resource.home).collect();
        let inside = queries::homes_under(&mut conn, org, &homes).await?;
        // A profile of the system belongs to every org.
        seen.retain(|each| each.resource.home.is_none_or(|home| inside.contains(&home)));
    }
    let private = super::hidden_items(&mut conn, &principal, &mut seen).await?;
    let ids: Vec<Id> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::agents(&mut conn, &ids).await?;
    let standing: HashMap<Id, Standing> = seen
        .into_iter()
        .map(|each| {
            (
                each.resource.id,
                Standing {
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    admin_only: each.admin_only,
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held,
                },
            )
        })
        .collect();
    Ok(super::listed(
        agent_views(&ctx, &mut conn, &principal, rows, &standing).await?,
        private,
    ))
}

/// Makes a profile in an org, the caller's own. Whoever may make it is told by the access engine
/// before this runs: they hold `settings.manage` on the org.
async fn create(ctx: Ctx, (): (), body: api::OrgAgentCreateBody) -> ApiResult<Agent> {
    let org = ctx.id("id")?;
    let name = clean_name("name", &body.name)?;
    let harness_kind = clean_harness_kind("harness_kind", &body.harness_kind)?;
    let profile = clean_profile("profile", &body.profile, false)?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let (place, kind) = placed(&mut tx, org).await?;
    if kind != NodeKind::Org {
        return Err(ApiError::not_found());
    }
    let id = orch_types::new_id();
    queries::insert_agent(
        &mut tx,
        &NewAgent {
            id,
            name: &name,
            owner_id: principal.user(),
            home_id: org,
            harness_kind: &harness_kind,
            machine_id: None,
            profile: &profile,
        },
        ctx.now,
    )
    .await?;
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Agent.into(),
            id,
        },
        principal.user(),
        NewAccess::Defaults,
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::AgentCreated),
        id,
        Home {
            node: Some(org),
            org: Some(place.org),
        },
        &format!("{who} added the agent profile {name}"),
        json!({ "agent": id, "name": name, "harness_kind": harness_kind }),
    )
    .await?;
    let made = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(made)
}

/// Reads a profile: whoever holds any permission on it.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Agent> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    queries::agents(&mut conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    require(
        &mut conn,
        &principal,
        ResourceKind::Agent,
        id,
        Permission::AgentView,
    )
    .await?;
    show(&ctx, &mut conn, &principal, id).await
}

/// Changes a profile, or switches it off or on.
async fn update(
    ctx: Ctx,
    query: orch_api_contract::resource_facts::ResourcePauseQuery,
    body: api::AgentUpdateBody,
) -> ApiResult<Agent> {
    let id = ctx.id("id")?;
    let note = super::impact::note(query.note.as_deref())?;
    if note.is_some() && body.disabled != Some(true) {
        return Err(invalid(
            "note",
            "is supplied only when switching the profile off",
        ));
    }
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let overrides = match &body.descriptor_overrides {
        Some(set) if !set.is_object() => {
            return Err(invalid(
                "descriptor_overrides",
                "the members to override are an object",
            ));
        }
        other => other.clone(),
    };
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = queries::agents(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if let Some(overrides) = &overrides
        && overrides
            .as_object()
            .is_some_and(|members| !members.is_empty())
    {
        let descriptor = serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(row.descriptor.clone())
            .map_err(|_| invalid("descriptor_overrides", "The harness must declare a valid descriptor before its fields can be overridden."))?;
        descriptor.with_overrides(overrides).map_err(|_| {
            invalid(
                "descriptor_overrides",
                "These values do not match the harness descriptor.",
            )
        })?;
    }
    let profile = body
        .profile
        .as_ref()
        .map(|profile| clean_profile("profile", profile, row.machine_id.is_some()))
        .transpose()?;
    let change = AgentChange {
        name: name.as_deref(),
        profile: profile.as_ref(),
        descriptor_overrides: overrides.as_ref(),
        disabled: body.disabled,
    };
    if change == AgentChange::default() {
        // Nothing is asked to change: the profile is as it is, at the revision it is.
        if row.revision != body.expected_revision {
            return Err(stale(&ctx, &mut tx, &principal, id).await);
        }
        return show(&ctx, &mut tx, &principal, id).await;
    }
    if !queries::change_agent(&mut tx, id, body.expected_revision, &change, ctx.now).await? {
        return Err(stale(&ctx, &mut tx, &principal, id).await);
    }
    let home = Home {
        node: row.home_id,
        org: row.org_id,
    };
    let who = actor_name(&ctx)?;
    let describes = name.is_some() || profile.is_some() || overrides.is_some();
    if describes {
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::AgentUpdated),
            id,
            home,
            &format!("{who} changed the agent profile {}", row.name),
            json!({
                "agent": id,
                "name": name,
                "profile": profile.is_some(),
                "descriptor_overrides": overrides.is_some(),
            }),
        )
        .await?;
    }
    let mut stopping = None;
    let mut resumed = Vec::new();
    if let Some(disabled) = body.disabled.filter(|disabled| *disabled != row.disabled) {
        let target = orch_api_contract::resource_facts::FactTarget {
            object: orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Agent,
                id,
            },
            model_id: None,
        };
        if disabled {
            stopping = Some(
                ctx.app
                    .engine
                    .stop_resource_in(
                        &mut tx,
                        orch_engine::resource_pause::ResourceStop {
                            target,
                            name: &row.name,
                            by: principal.user(),
                            note,
                            deleted: false,
                        },
                        ctx.now,
                    )
                    .await
                    .map_err(ApiError::internal)?,
            );
        } else {
            if orch_engine::resource_facts::activity::stopping(&mut tx, &target)
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::PlanBlocked,
                    "Waiting for the accepted stop to finish",
                ));
            }
            let finished = ctx
                .app
                .engine
                .enable_resource_waiting_in(
                    &mut tx,
                    orch_engine::resource_pause::ResourceResume {
                        target: &target,
                        name: &row.name,
                        by: principal.user(),
                        run_waiting: query.run_waiting.unwrap_or(false),
                    },
                    ctx.now,
                )
                .await
                .map_err(ApiError::internal)?;
            resumed.extend(finished.wakes);
        }
        sqlx::query("UPDATE agents SET pause_note=$2 WHERE id=$1")
            .bind(id)
            .bind(if disabled { note } else { None })
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        let (action, word) = if disabled {
            (
                orch_db::audit::action!(AuditAction::AgentDisabled),
                "switched off",
            )
        } else {
            (
                orch_db::audit::action!(AuditAction::AgentEnabled),
                "switched on",
            )
        };
        record(
            &ctx,
            &mut tx,
            action,
            id,
            home,
            &format!("{who} {word} the agent profile {}", row.name),
            json!({ "agent": id }),
        )
        .await?;
    }
    let changed = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    for thread in resumed {
        ctx.app.engine.wake(thread);
    }
    if let Some((job, finished)) = stopping {
        for thread in finished.wakes {
            ctx.app.engine.wake(thread);
        }
        ctx.app
            .engine
            .stop_resource_now(job.id)
            .await
            .map_err(ApiError::internal)?;
    }
    // The previous end may have failed just before this repair committed. Its durable work
    // needs this event even when a restart's first attempt raced the old declaration.
    if overrides
        .as_ref()
        .is_some_and(|overrides| overrides != &row.descriptor_overrides)
        && let Err(error) = ctx.app.engine.wake_agent_terminal_work(id).await
    {
        tracing::warn!(%id, %error, "a repaired agent profile's terminal work awaits durable recovery");
    }
    Ok(changed)
}

/// The answer to a change that was made against a revision that is out of date: what the profile
/// is now.
async fn stale(ctx: &Ctx, conn: &mut Conn, principal: &Principal, id: Id) -> ApiError {
    let current = show(ctx, conn, principal, id).await.ok();
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the profile was changed since it was read",
    )
    .with("current", current)
}

/// Ends a profile while retaining its bound threads and immutable turn history.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = queries::agents(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let target = orch_api_contract::resource_facts::FactTarget {
        object: orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Agent,
            id,
        },
        model_id: None,
    };
    let (job, finished) = ctx
        .app
        .engine
        .stop_resource_in(
            &mut tx,
            orch_engine::resource_pause::ResourceStop {
                target,
                name: &row.name,
                by: principal.user(),
                note: None,
                deleted: true,
            },
            ctx.now,
        )
        .await
        .map_err(ApiError::internal)?;
    queries::delete_agent(&mut tx, id, ctx.now).await?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::AgentDeleted),
        id,
        Home {
            node: row.home_id,
            org: row.org_id,
        },
        &format!("{who} deleted the agent profile {}", row.name),
        json!({ "agent": id, "name": row.name }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    for thread in finished.wakes {
        ctx.app.engine.wake(thread);
    }
    ctx.app
        .engine
        .stop_resource_now(job.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

// ---- the adapters the system has: the list the dialogs of a profile and of a provider read ----

/// The numbers a version is made of, to put versions in order: `0.160.1` comes after `0.99.0`.
/// Anything that is no digit separates two numbers.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|each: char| !each.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(u64::MAX))
        .collect()
}

/// What is known of one harness kind while the list is made.
#[derive(Default)]
struct KindSeen {
    /// The name of the system's own profile of the adapter, which it was seeded as.
    system_name: Option<String>,
    /// What that profile's harness declared, when it did.
    system_descriptor: Option<Json>,
    /// What the harness of the profile that was last changed declared, whoever's the profile is.
    newest: Option<(chrono::DateTime<chrono::Utc>, Json)>,
    /// The builds of the harness that machines report, the one a descriptor names, and those the system lists.
    versions: BTreeSet<String>,
    /// The builds the system lists, by version: whether any of them is the harness's default for a system and
    /// a processor.
    listed: BTreeMap<String, bool>,
}

/// The adapters the system has, from the profiles it keeps and from what the machines report of
/// themselves: a kind that a profile runs or a machine's connector can drive, with the builds
/// the system lists (`listed`) and those found on machines, and what its adapter declared last. The
/// profile of the system itself speaks for its kind; where it never reported, the newest report of
/// any profile of the kind does.
fn harness_kinds_of(
    programs: &orch_agent_programs::Programs,
    profiles: &[AgentKindRow],
    adapter_kinds: &[String],
    found: &[(String, String)],
    listed: &[HarnessBuild],
) -> Vec<api::HarnessKindInfo> {
    let mut kinds: BTreeMap<String, KindSeen> = BTreeMap::new();

    for profile in profiles {
        let seen = kinds.entry(profile.harness_kind.clone()).or_default();
        let declared = profile
            .descriptor
            .as_object()
            .is_some_and(|members| !members.is_empty());
        if profile.of_the_system {
            seen.system_name.get_or_insert_with(|| profile.name.clone());
            if declared {
                seen.system_descriptor
                    .get_or_insert_with(|| profile.descriptor.clone());
            }
        }
        if declared
            && seen
                .newest
                .as_ref()
                .is_none_or(|(at, _)| *at < profile.updated_at)
        {
            seen.newest = Some((profile.updated_at, profile.descriptor.clone()));
        }
    }
    let text =
        |value: &Json, member: &str| value.get(member).and_then(Json::as_str).map(str::to_owned);
    for kind in adapter_kinds {
        kinds.entry(kind.clone()).or_default();
    }
    for (kind, version) in found {
        // A harness no adapter drives is no adapter of the system's.
        if let Some(seen) = kinds.get_mut(kind) {
            seen.versions.insert(version.clone());
        }
    }
    for build in listed {
        // A harness no adapter drives is no adapter of the system's.
        if let Some(seen) = kinds.get_mut(&build.kind) {
            *seen.listed.entry(build.version.clone()).or_insert(false) |= build.is_default;
            seen.versions.insert(build.version.clone());
        }
    }
    kinds
        .into_iter()
        .map(|(kind, mut seen)| {
            let declared = seen
                .system_descriptor
                .take()
                .or_else(|| seen.newest.take().map(|(_, descriptor)| descriptor));
            let reported = declared.clone().unwrap_or_else(|| json!({}));
            let harness = reported.get("harness");
            if let Some(version) = harness.and_then(|harness| text(harness, "version")) {
                seen.versions.insert(version);
            }
            // What the profile's harness declared says first, then the manifest, then the name of the system's
            // profile of the kind, and the kind.
            let mut named = crate::programs::harness_name(programs, &kind, declared.as_ref());
            let title = if named.name == kind {
                seen.system_name.take().unwrap_or_else(|| kind.clone())
            } else {
                named.name.clone()
            };
            named.name.clone_from(&title);
            let typed = declared
                .as_ref()
                .and_then(|descriptor| {
                    serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(
                        descriptor.clone(),
                    )
                    .ok()
                })
                .map(orch_agent_contract::descriptor::AgentDescriptor::normalized);
            let program = programs
                .program(&kind)
                .map(|manifest| crate::programs::program_info(programs, manifest, typed.as_ref()));
            let mut versions: Vec<String> = seen.versions.into_iter().collect();
            versions.sort_by(|first, second| {
                version_key(second)
                    .cmp(&version_key(first))
                    .then_with(|| second.cmp(first))
            });
            // With no profile naming a build, the default the system lists is what gets run, and where it
            // lists none the newest one is.
            let listed_default = seen.listed.values().any(|default| *default);
            let builds = versions
                .into_iter()
                .enumerate()
                .map(|(index, version)| api::HarnessKindBuild {
                    is_default: if listed_default {
                        seen.listed.get(&version) == Some(&true)
                    } else {
                        index == 0
                    },
                    version,
                })
                .collect();
            api::HarnessKindInfo {
                kind,
                title,
                harness: named,
                builds,
                // As the descriptor type writes it where the report reads as one, and as it was reported where it
                // is partial.
                descriptor: typed
                    .as_ref()
                    .and_then(|descriptor| serde_json::to_value(descriptor).ok())
                    .or(declared),
                program,
            }
        })
        .collect()
}

/// The adapters the system has, built in and external. Anybody who is signed in reads it: it says
/// what can be run, and nobody's machine by name.
async fn harness_kinds(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::HarnessKindInfo>> {
    let mut conn = ctx.app.db.acquire().await?;
    let profiles = binding_rows::agent_kind_summaries(&mut conn).await?;
    let adapter_kinds = binding_rows::machine_adapter_kinds(&mut conn).await?;
    let found = binding_rows::machine_harness_versions(&mut conn).await?;
    let listed = builds_listed(&payload_dir(&ctx.app.config), &mut conn).await?;
    let programs = orch_db::programs::registry(&mut conn).await?;
    Ok(harness_kinds_of(
        &programs,
        &profiles,
        &adapter_kinds,
        &found,
        &listed,
    ))
}

/// The executors the system knows, with the tool sets of each and what an exec node can offer of it. They are the
/// executors this build ships: no machine reports one yet, so nothing is read from the database.
async fn harness_executors(
    _ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Vec<orch_api_contract::programs::ExecutorInfo>> {
    Ok(crate::programs::executors(
        orch_agent_programs::Programs::builtin(),
    ))
}

// ---- a profile for a harness that a person's machine drives ----

/// Whether a machine reported that its connector can drive the adapter of `kind`. One that reported
/// nothing has not, and there is nothing to register on it.
pub(crate) fn machine_reports(facts: &Json, kind: &str) -> bool {
    let adapter = facts
        .get("adapters")
        .and_then(Json::as_array)
        .and_then(|adapters| {
            adapters
                .iter()
                .find(|each| each.get("kind").and_then(Json::as_str) == Some(kind))
        });
    let Some(adapter) = adapter else {
        return false;
    };
    // A registered external adapter may supply the whole program. A built-in adapter alone
    // is not evidence of an installed/allowed native binary on that machine.
    if adapter.get("needs_binary").and_then(Json::as_bool) == Some(false) {
        return true;
    }
    let installed = facts.get("harnesses").and_then(Json::as_array);
    let allowed = facts
        .pointer("/policy/agent.harnesses")
        .and_then(Json::as_array);
    installed.into_iter().flatten().any(|program| {
        program.get("kind").and_then(Json::as_str) == Some(kind)
            && program
                .get("path")
                .and_then(Json::as_str)
                .is_some_and(|path| {
                    !path.is_empty()
                        && allowed.into_iter().flatten().any(|entry| {
                            entry.get("kind").and_then(Json::as_str) == Some(kind)
                                && entry.get("path").and_then(Json::as_str) == Some(path)
                        })
                })
    })
}

/// Registers a profile for a harness that a machine of the caller's drives, in their domain or an org
/// inside it. The profile is the caller's own, as the machine is. What the machine did not report it
/// can drive is not registered on it.
async fn machine_agent_create(
    ctx: Ctx,
    (): (),
    body: api::MachineAgentCreateBody,
) -> ApiResult<Agent> {
    let name = clean_name("name", &body.name)?;
    let harness_kind = clean_harness_kind("harness_kind", &body.harness_kind)?;
    let profile = clean_profile("profile", body.profile.as_ref().unwrap_or(&json!({})), true)?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let now = ctx.now;
    let machine = own_machine(&ctx, &mut tx, ctx.id("id")?).await?;
    if !machine.is_active() {
        return Err(ApiError::not_found());
    }
    crate::backplane::require_facts(&machine.facts)?;
    let place = registration_home(&mut tx, &principal, body.home_id, "home_id").await?;
    if !machine.roles.iter().any(|role| role == "agent")
        || !machine_reports(&machine.facts, &harness_kind)
    {
        return Err(invalid(
            "harness_kind",
            "this machine did not report that it can run this harness",
        ));
    }
    let id = orch_types::new_id();
    queries::insert_agent(
        &mut tx,
        &NewAgent {
            id,
            name: &name,
            owner_id: principal.user(),
            home_id: body.home_id,
            harness_kind: &harness_kind,
            machine_id: Some(machine.id),
            profile: &profile,
        },
        now,
    )
    .await?;
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Agent.into(),
            id,
        },
        principal.user(),
        NewAccess::Defaults,
        Time::new(now),
    )
    .await
    .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        AuditAction::AgentCreated,
        id,
        Home {
            node: Some(body.home_id),
            org: Some(place.org),
        },
        &format!(
            "{who} added the agent profile {name} for the harness {harness_kind} on the machine {}",
            machine.name
        ),
        json!({ "agent": id, "name": name, "harness_kind": harness_kind, "machine": machine.id }),
    )
    .await?;
    let made = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_builtin_adapter_requires_a_reported_allowed_program_but_a_self_contained_adapter_does_not()
    {
        let mut facts = json!({"adapters":[{"kind":"native-example","needs_binary":true}]});
        assert!(!machine_reports(&facts, "native-example"));
        facts["harnesses"] = json!([{"kind":"native-example","path":"/opt/native"}]);
        assert!(!machine_reports(&facts, "native-example"));
        facts["policy"] =
            json!({"agent.harnesses":[{"kind":"native-example","path":"/opt/other"}]});
        assert!(!machine_reports(&facts, "native-example"));
        facts["policy"]["agent.harnesses"] =
            json!([{"kind":"native-example","path":"/opt/native"}]);
        assert!(machine_reports(&facts, "native-example"));
        assert!(machine_reports(
            &json!({"adapters":[{"kind":"external-example","needs_binary":false}]}),
            "external-example"
        ));
    }

    #[test]
    fn the_system_overrides_a_member_of_the_descriptor_and_says_which() {
        let (shown, overridden) = descriptor_shown(
            &json!({ "title": "Codex", "streaming": true }),
            &json!({ "title": "Team Codex" }),
        );
        assert_eq!(shown, json!({ "title": "Team Codex", "streaming": true }));
        assert_eq!(overridden, ["title"]);
        let (shown, overridden) = descriptor_shown(&json!({ "title": "Codex" }), &json!({}));
        assert_eq!(shown, json!({ "title": "Codex" }));
        assert!(overridden.is_empty());
    }

    #[test]
    fn a_profile_has_four_members_and_the_same_home_choice_on_every_machine() {
        assert!(clean_profile("profile", &json!({ "harness_version": "1.2" }), false).is_ok());
        assert!(clean_profile("profile", &json!({ "colour": "red" }), false).is_err());
        assert!(clean_profile("profile", &json!("codex"), false).is_err());
        let attached = json!({ "attach": { "mode": "daemon" } });
        assert!(clean_profile("profile", &attached, false).is_ok());
        assert!(clean_profile("profile", &attached, true).is_ok());
        for machine in [false, true] {
            for home in [
                json!({"kind":"made_by_orch"}),
                json!({"kind":"existing_directory","path":"/srv/harness"}),
            ] {
                assert!(
                    clean_profile("profile", &json!({"attach":{"home":home}}), machine).is_ok()
                );
            }
            assert!(clean_profile("profile",&json!({"attach":{"home":{"kind":"existing_directory","path":"../someone-else"}}}),machine).is_err());
        }
    }

    #[test]
    fn a_harness_kind_is_what_an_adapter_names_itself() {
        for good in ["codex", "claude-code", "fake_2"] {
            assert!(clean_harness_kind("harness_kind", good).is_ok(), "{good}");
        }
        for bad in ["", "Codex", "2fa", "has space", &"x".repeat(65)] {
            assert!(clean_harness_kind("harness_kind", bad).is_err(), "{bad}");
        }
    }
}
