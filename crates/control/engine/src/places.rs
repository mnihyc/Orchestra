//! Where an environment can run: the hosts its creator may use, capacity, what the units there declare they need (which reserves nothing), free room, when a host counts as full and whether it then warns or refuses, storages, mounts present, and scheduling.
//!
//! A host is a place for an environment made from a template when its creator may use it and the
//! template there (their entries on the two allow the environment as it would be made), its driver
//! makes the template, it carries the labels the template asks for, it offers the source the template
//! names, every mount the environment attaches has a place on it, and it has the storages the
//! environment names. What it has is measured in memory, the one thing that cannot be shared out
//! twice: a host is **full** when the memory its units declare they need reaches `hosts.full_percent`
//! of the memory it reports, and has **no room** when it declares less free memory than the new
//! environment needs. Both are warnings unless `hosts.when_full` (the host's own answer, else the
//! system's) says `block`: then the host is not usable, for a request that names it and for the
//! scheduler alike. Nothing is reserved: the figures are sums of statements, and the host decides
//! when the unit is made.

use crate::error::EngineError;
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::{Permission, ResourceKind, ResourceLimits};
use orch_api_contract::environment::{
    Amounts, HostReason, HostStorage, HostWarning, PlaceEnvironments, PlaceHost, PlaceMount,
    PlaceUsed, PlanHostBlocker, WhenFull,
};
use orch_api_contract::mounts::MountRef;
use orch_api_contract::resources::HostRef;
use orch_authz::environments::{
    Created, Creating, Limit, NewEnvironment, Place, Wants, check_creation,
};
use orch_authz::nodes::Page;
use orch_authz::usable::{Policies, ResourcePolicy};
use orch_authz::{Policy, Principal, resources};
use orch_db::Conn;
use orch_db::queries::resources::hosting::{self as rows, HostState, LoadRow, TemplateState};
use orch_settings::{Place as SettingsPlace, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};

/// How many hosts a list of places holds at most.
const HOSTS_MAX: usize = 500;
/// The fullness at which a host counts as full when nothing says otherwise, in percent: nine
/// tenths of its memory declared.
pub const FULL_PERCENT_DEFAULT: i64 = 90;
/// The minutes an environment must have been idle to count as idle on a host when nothing says
/// otherwise: the hour a recycle starts from.
pub const IDLE_MINUTES_DEFAULT: i64 = 60;
/// What a processor is in the thousandths the API counts it in.
const CPU_MILLIS_PER_CPU: i64 = 1_000;
/// The word of the source a template names when it names none of the three.
const SOURCE_NONE: &str = "image";

/// What an environment declares it needs: the least it is of use with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Needs {
    /// Thousandths of a processor.
    pub cpu_millis: i64,
    /// Mebibytes of memory.
    pub memory_mb: i64,
    /// Gibibytes of disk.
    pub disk_gb: i64,
}

fn number(value: Option<&Json>, key: &str) -> Option<i64> {
    value?.get(key)?.as_i64()
}

fn policy_descriptor(
    name: &str,
    value: &Json,
) -> Option<orch_driver_contract::descriptor::DriverDescriptor> {
    let mut value = value.clone();
    let members = value.as_object_mut()?;
    // Earlier host reports omit display metadata; none of these defaults grants an operation.
    members.entry("name").or_insert_with(|| Json::from(name));
    members
        .entry("version")
        .or_insert_with(|| Json::from("unknown"));
    members.entry("title").or_insert_with(|| Json::from(name));
    serde_json::from_value(value).ok()
}

/// What an environment declares it needs: its own `env.needs` over the template's `needs`; what both
/// leave out it counts with its limits, as an environment that states no smaller need.
#[must_use]
pub fn needs_of(spec: &Json, own: Option<&Json>, limits: Option<&Json>) -> Needs {
    let pick = |key: &str| {
        number(own, key)
            .or_else(|| number(spec.get("needs"), key))
            .or_else(|| number(limits, key))
            .or_else(|| number(spec.get("resources"), key))
            .unwrap_or(0)
    };
    Needs {
        cpu_millis: pick("cpu_millis"),
        memory_mb: pick("memory_mb"),
        disk_gb: pick("disk_gb"),
    }
}

/// The limits an environment's unit is made with: the template's `resources`, each member raised or
/// lowered by the environment's own `env.resources`, within the template's `resources_max`.
#[must_use]
pub fn limits_of(spec: &Json, own: Option<&Json>) -> Json {
    let mut limits = spec
        .get("resources")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(own) = own.and_then(Json::as_object) {
        for (key, value) in own {
            limits.insert(key.clone(), value.clone());
        }
    }
    if let Some(most) = spec.get("resources_max").and_then(Json::as_object) {
        for (key, ceiling) in most {
            if let (Some(wanted), Some(ceiling)) =
                (limits.get(key).and_then(Json::as_i64), ceiling.as_i64())
                && wanted > ceiling
            {
                limits.insert(key.clone(), Json::from(ceiling));
            }
        }
    }
    Json::Object(limits)
}

/// The source a docker-style spec names for the unit's file system.
#[must_use]
pub fn source_of(spec: &Json) -> &'static str {
    for source in ["image", "build", "archive"] {
        if spec.get(source).is_some_and(|value| !value.is_null()) {
            return match source {
                "build" => "build",
                "archive" => "archive",
                _ => "image",
            };
        }
    }
    SOURCE_NONE
}

/// What a new environment would be made with, worked out from its template and its own values.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    /// The template.
    pub template: TemplateState,
    /// What it declares it needs.
    pub needs: Needs,
    /// The limits of its unit.
    pub limits: Json,
    /// The source of its file system.
    pub source: &'static str,
    /// The labels a host must carry.
    pub labels: Vec<String>,
    /// The runtime class its spec names.
    pub runtime_class: Option<String>,
    /// The named storages its volumes are asked to be on.
    pub storages: Vec<String>,
    /// The mounts it attaches.
    pub mounts: Vec<Id>,
    /// The capabilities it is given from the start.
    pub capabilities: Vec<String>,
}

fn texts(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The value of a key of an environment that does not exist yet: what its home resolves, with its own
/// values laid over it (a map member by member, anything else replaced).
pub async fn resolved_for_new(
    conn: &mut Conn,
    home: Id,
    own: &Json,
    key: &str,
) -> Result<Option<Json>, EngineError> {
    let inherited: Option<Json> = get(conn, SettingsPlace::Node(home), key).await?;
    Ok(match (inherited, own.get(key)) {
        (Some(Json::Object(mut base)), Some(Json::Object(over))) => {
            for (name, value) in over {
                base.insert(name.clone(), value.clone());
            }
            Some(Json::Object(base))
        }
        (_, Some(over)) => Some(over.clone()),
        (inherited, None) => inherited,
    })
}

/// What a new environment of `template` with the own values `own`, at `home`, would be made with.
pub async fn wanted(
    conn: &mut Conn,
    home: Id,
    template: TemplateState,
    own: &Json,
) -> Result<Wanted, EngineError> {
    let needs_own = resolved_for_new(conn, home, own, keys::ENV_NEEDS).await?;
    let limits_own = resolved_for_new(conn, home, own, keys::ENV_RESOURCES).await?;
    let storage = resolved_for_new(conn, home, own, keys::ENV_STORAGE).await?;
    let mounts = resolved_for_new(conn, home, own, keys::ENV_MOUNTS).await?;
    let limits = limits_of(&template.spec, limits_own.as_ref());
    let needs = needs_of(&template.spec, needs_own.as_ref(), Some(&limits));
    let storages = storage
        .as_ref()
        .and_then(Json::as_object)
        .map(|named| {
            named
                .values()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let mounts = mounts
        .as_ref()
        .and_then(Json::as_object)
        .map(|named| {
            named
                .iter()
                .filter(|(_, mode)| **mode != Json::Bool(false))
                .filter_map(|(id, _)| id.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    Ok(Wanted {
        needs,
        limits,
        source: source_of(&template.spec),
        labels: texts(template.spec.get("host_labels")),
        // The runtime chosen for this environment, else the template's own.
        runtime_class: resolved_for_new(conn, home, own, keys::ENV_RUNTIME)
            .await?
            .and_then(|chosen| chosen.as_str().map(str::to_owned))
            .or_else(|| {
                template
                    .spec
                    .get("runtime")
                    .and_then(Json::as_str)
                    .map(str::to_owned)
            }),
        storages,
        mounts,
        capabilities: texts(template.spec.get("base_capabilities")),
        template,
    })
}

// ---------------------------------------------------------------- what a host has

fn capacity_of(host: &HostState) -> Amounts {
    let cpus = host.capacity.get("cpus");
    let cpu_millis = host
        .capacity
        .get("cpu_millis")
        .and_then(Json::as_i64)
        .or_else(|| {
            cpus.and_then(Json::as_i64)
                .map(|count| count.saturating_mul(CPU_MILLIS_PER_CPU))
                .or_else(|| {
                    cpus.and_then(Json::as_f64).map(|count| {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "a count of processors that a host reports is small"
                        )]
                        let millis = (count * 1_000.0).round() as i64;
                        millis
                    })
                })
        })
        .unwrap_or(0);
    Amounts {
        cpu_millis,
        memory_mb: number(Some(&host.capacity), "memory_mb").unwrap_or(0),
        disk_gb: number(Some(&host.capacity), "disk_gb").unwrap_or(0),
    }
}

/// Whether a runtime in `state` still holds the memory and the processors its declaration names.
fn holds_memory(state: &str) -> bool {
    matches!(state, "creating" | "running" | "stopping")
}

/// What the units on a host declare they need: processors and memory of those that hold them, disk of
/// every environment that has volumes there, running or stopped.
#[must_use]
pub fn declared_of(loads: &[LoadRow]) -> Amounts {
    let mut declared = Amounts {
        cpu_millis: 0,
        memory_mb: 0,
        disk_gb: 0,
    };
    for load in loads {
        let (needs_key, resources_key) = if load.kind == "provider" {
            (keys::RUNTIME_NEEDS, keys::RUNTIME_RESOURCES)
        } else {
            (keys::ENV_NEEDS, keys::ENV_RESOURCES)
        };
        let own = load
            .config
            .as_ref()
            .and_then(|config| config.get(needs_key));
        let limits = load
            .config
            .as_ref()
            .and_then(|config| config.get(resources_key));
        let limits = limits_of(&load.spec, limits);
        let needs = needs_of(&load.spec, own, Some(&limits));
        if holds_memory(&load.state) {
            declared.cpu_millis = declared.cpu_millis.saturating_add(needs.cpu_millis);
            declared.memory_mb = declared.memory_mb.saturating_add(needs.memory_mb);
        }
        declared.disk_gb = declared.disk_gb.saturating_add(needs.disk_gb);
    }
    declared
}

/// What the units on a host declare they need, with what each costs the host beyond its limits under its runtime: a guest
/// with a kernel of its own holds memory the unit's limit does not count (its kernel, its hypervisor), which the runtime's
/// class declares, so a host with many of them is full sooner.
#[must_use]
pub fn declared_with_overhead(loads: &[LoadRow], drivers: &Json) -> Amounts {
    let mut declared = declared_of(loads);
    // What each runtime the host reports costs, by every name it is known by: read once, not for every unit.
    let mut overheads: std::collections::BTreeMap<&str, i64> = std::collections::BTreeMap::new();
    for class in drivers
        .as_object()
        .into_iter()
        .flat_map(|drivers| drivers.values())
        .filter_map(|driver| driver.get("runtimes").and_then(Json::as_array))
        .flatten()
    {
        let Some(overhead) = class.get("memory_overhead_mb").and_then(Json::as_i64) else {
            continue;
        };
        let names = class.get("name").and_then(Json::as_str).into_iter().chain(
            class
                .get("aliases")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(Json::as_str),
        );
        for name in names {
            overheads.entry(name).or_insert(overhead);
        }
    }
    if overheads.is_empty() {
        return declared;
    }
    for load in loads.iter().filter(|load| holds_memory(&load.state)) {
        let runtime = load
            .config
            .as_ref()
            .and_then(|config| config.get(keys::ENV_RUNTIME))
            .and_then(Json::as_str)
            .or_else(|| load.spec.get("runtime").and_then(Json::as_str));
        let overhead = runtime
            .and_then(|runtime| overheads.get(runtime))
            .copied()
            .unwrap_or(0);
        declared.memory_mb = declared.memory_mb.saturating_add(overhead.max(0));
    }
    declared
}

// Group current units once; visiting every unit for each host multiplies scheduling work.
pub(crate) fn loads_by_host(
    loads: impl IntoIterator<Item = LoadRow>,
) -> std::collections::HashMap<Id, Vec<LoadRow>> {
    let mut grouped = std::collections::HashMap::<Id, Vec<LoadRow>>::new();
    for load in loads {
        grouped.entry(load.host_id).or_default().push(load);
    }
    grouped
}

fn free_of(capacity: &Amounts, declared: &Amounts) -> Amounts {
    Amounts {
        cpu_millis: (capacity.cpu_millis - declared.cpu_millis).max(0),
        memory_mb: (capacity.memory_mb - declared.memory_mb).max(0),
        disk_gb: (capacity.disk_gb - declared.disk_gb).max(0),
    }
}

/// Whether a host is full: the memory its units declare reaches `percent` of the memory it reports.
/// A host that has reported no memory is not judged.
#[must_use]
pub fn is_full(capacity: &Amounts, declared: &Amounts, percent: i64) -> bool {
    capacity.memory_mb > 0
        && declared.memory_mb.saturating_mul(100) >= capacity.memory_mb.saturating_mul(percent)
}

/// Whether a host declares less free memory than the environment needs. A host that has reported no
/// memory is not judged.
#[must_use]
pub fn lacks_room(capacity: &Amounts, declared: &Amounts, needs: &Needs) -> bool {
    capacity.memory_mb > 0 && capacity.memory_mb - declared.memory_mb < needs.memory_mb
}

fn storages_of(host: &HostState) -> Vec<HostStorage> {
    host.storages
        .as_array()
        .map(|reported| {
            reported
                .iter()
                .filter_map(|storage| serde_json::from_value(storage.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn descriptor_lists(host: &HostState, driver: &str, member: &str) -> Option<Vec<String>> {
    let descriptor = host.drivers.get(driver)?;
    Some(texts(descriptor.get(member)))
}

// ---------------------------------------------------------------- the list

/// What the list of places is asked for.
#[derive(Debug, Clone)]
pub struct Ask<'a> {
    /// The node whose settings the environment would take.
    pub home: Id,
    /// What the environment would be made with.
    pub wanted: &'a Wanted,
    /// The host it is on now, where it is on one.
    pub current: Option<Id>,
    /// The minutes without activity after which an environment on a host counts as idle there.
    pub minutes_idle: i64,
    /// The moment the list is made at.
    pub now: DateTime<Utc>,
}

/// A host a new environment could be made on, with the figures it was judged by.
#[derive(Debug, Clone, PartialEq)]
pub struct HostPlace {
    /// The place as the API shows it; the owner and the address of the host are filled in by whoever
    /// can read people and addresses.
    pub place: PlaceHost,
    /// Whose machine it is; none for a host of the system.
    pub owner: Option<Id>,
    /// The host's row.
    pub host: HostState,
    /// What the creator's entries on the host and the template come to, where the host can be
    /// used.
    pub created: Option<Created>,
}

impl HostPlace {
    /// Whether the host can be chosen.
    #[must_use]
    pub const fn usable(&self) -> bool {
        self.place.usable
    }
}

/// How full a host is allowed to be, by what the system and the host say: the answer of a host
/// that has one of its own, else the system's.
pub async fn system_when_full(conn: &mut Conn) -> Result<WhenFull, EngineError> {
    let word: Option<String> = get(conn, SettingsPlace::System, keys::HOSTS_WHEN_FULL).await?;
    Ok(match word.as_deref() {
        Some("block") => WhenFull::Block,
        _ => WhenFull::Warn,
    })
}

/// The fullness at which a host counts as full.
pub async fn system_full_percent(conn: &mut Conn) -> Result<i64, EngineError> {
    Ok(get(conn, SettingsPlace::System, keys::HOSTS_FULL_PERCENT)
        .await?
        .unwrap_or(FULL_PERCENT_DEFAULT))
}

/// The policy of the access engine at a node: whether the owners of a thing there may name
/// somebody from outside its root org.
pub async fn policies_at(conn: &mut Conn, node: Id) -> Result<Policies, EngineError> {
    let allowed: Option<bool> = get(
        conn,
        SettingsPlace::Node(node),
        keys::SHARING_ALLOW_EXTERNAL,
    )
    .await?;
    Ok(Policies::everywhere(ResourcePolicy {
        owners_name_outsiders: allowed.unwrap_or(true),
        shared_subscriptions: None,
    }))
}

fn reasons_of(limits: &[Limit]) -> Vec<HostReason> {
    let mut reasons: Vec<HostReason> = Vec::new();
    for limit in limits {
        let reason = match limit {
            Limit::TemplateNotAllowedOnTheHost { .. } => HostReason::TemplateNotAllowed,
            _ => HostReason::EntryLimit,
        };
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    reasons
}

/// The hosts the principal may use for a new environment as `ask` describes it, the one with the most
/// free memory first, each with whether it can be chosen, why not, what it warns about, and whether
/// the scheduler would choose it.
pub async fn host_places(
    conn: &mut Conn,
    principal: &Principal,
    ask: &Ask<'_>,
) -> Result<Vec<HostPlace>, EngineError> {
    host_places_inner(conn, principal, ask, None, None).await
}

/// Plans placement of this current environment, retaining only its own existing capacity slot.
pub async fn host_places_for_environment(
    conn: &mut Conn,
    principal: &Principal,
    ask: &Ask<'_>,
    environment: Id,
) -> Result<Vec<HostPlace>, EngineError> {
    host_places_inner(conn, principal, ask, Some(environment), None).await
}

/// A completed environment may retain its exact published host/template through
/// EnvironmentManage. It gains no HostUse or permission to create another environment.
/// Physical availability, driver/local policy, storage and mount checks remain shared.
pub async fn retained_host_places(
    conn: &mut Conn,
    principal: &Principal,
    ask: &Ask<'_>,
    environment: Id,
    host: Id,
) -> Result<Vec<HostPlace>, EngineError> {
    let Some(state) = orch_db::queries::environments::hosting::environment(conn, environment)
        .await?
        .filter(|state| {
            state.deleted_at.is_none()
                && state.generation > 0
                && state.home_node_id == ask.home
                && state.template_id == Some(ask.wanted.template.id)
        })
    else {
        return Ok(Vec::new());
    };
    if orch_db::queries::environments::hosting::published_host_of(conn, state.id).await?
        != Some(host)
        || !resources::check(
            conn,
            principal,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: environment,
            },
            Permission::EnvironmentManage,
            Policy::default(),
        )
        .await?
        .is_allowed()
    {
        return Ok(Vec::new());
    }
    host_places_inner(conn, principal, ask, Some(environment), Some(host)).await
}

async fn host_places_inner(
    conn: &mut Conn,
    principal: &Principal,
    ask: &Ask<'_>,
    existing: Option<Id>,
    retained_host: Option<Id>,
) -> Result<Vec<HostPlace>, EngineError> {
    let may_use = if let Some(host) = retained_host {
        vec![host]
    } else {
        let seen = resources::visible(
            conn,
            principal,
            ResourceKind::Host,
            Page {
                after: None,
                limit: HOSTS_MAX,
            },
            Policy::default(),
        )
        .await?;
        seen.items
            .iter()
            .filter(|each| {
                each.held.iter().any(|held| {
                    matches!(
                        held.permission,
                        Permission::HostUse | Permission::HostManage
                    )
                })
            })
            .map(|each| each.resource.id)
            .collect()
    };
    if may_use.is_empty() {
        return Ok(Vec::new());
    }
    let hosts = rows::hosts(conn, Some(&may_use)).await?;
    let machine_ids: Vec<Id> = hosts.iter().map(|host| host.machine_id).collect();
    let machine_policies: std::collections::HashMap<Id, orch_link_contract::policy::LocalPolicy> =
        orch_db::queries::environment_facts::policies(conn, &machine_ids)
            .await?
            .into_iter()
            .filter_map(|(id, policy)| {
                serde_json::from_value(policy)
                    .ok()
                    .map(|policy| (id, policy))
            })
            .collect();
    let mut loads = loads_by_host(rows::loads(conn, &may_use).await?);
    let system_when_full = system_when_full(conn).await?;
    let full_percent = system_full_percent(conn).await?;
    let policies = policies_at(conn, ask.home).await?;
    let wanted = ask.wanted;
    let mut places: Vec<HostPlace> = Vec::new();
    for host in hosts {
        let held = loads.remove(&host.id).unwrap_or_default();
        let capacity = capacity_of(&host);
        let declared = declared_with_overhead(&held, &host.drivers);
        let free = free_of(&capacity, &declared);
        let when_full = host.when_full.as_deref().map_or(system_when_full, |word| {
            if word == "block" {
                WhenFull::Block
            } else {
                WhenFull::Warn
            }
        });
        let mut reasons: Vec<HostReason> = Vec::new();
        let mut warnings: Vec<HostWarning> = Vec::new();
        let policy_refusal=machine_policies.get(&host.machine_id).map_or_else(
            ||Some("drivers (node role): this machine has not reported its policy; its operator decides which drivers it offers".into()),
            |policy| {
                let capabilities=orch_driver_contract::capabilities::CapabilityState {base:wanted.capabilities.clone(),..Default::default()};
                let template=wanted.template.id.to_string();
                host.drivers.get(&wanted.template.driver)
                    .and_then(|descriptor| policy_descriptor(&wanted.template.driver, descriptor))
                    .map_or_else(|| Some("the host has not reported a complete driver descriptor".into()), |descriptor| policy.drivers.check(&descriptor,&wanted.template.spec,Some(&template),&capabilities).err())
                    .or_else(||serde_json::from_value(wanted.limits.clone()).ok().and_then(|limits|policy.bounded_resources(limits).err()))
            });
        if policy_refusal.is_some() {
            reasons.push(HostReason::Driver);
        }
        // What the host is and has.
        if host.disabled || !host.schedulable || !host.online || host.machine_status != "active" {
            reasons.push(HostReason::Unschedulable);
        }
        let driver = &wanted.template.driver;
        let makes = host.drivers.get(driver).is_some_and(|descriptor| {
            let pools = descriptor.get("spec_schemas").and_then(Json::as_object);
            pools.is_none_or(|pools| pools.contains_key(&wanted.template.kind))
        });
        let classes = descriptor_lists(&host, driver, "runtime_classes").unwrap_or_default();
        let class_missing = wanted
            .runtime_class
            .as_ref()
            .is_some_and(|class| !classes.contains(class));
        if !makes || class_missing {
            reasons.push(HostReason::Driver);
        } else if let Some(sources) = descriptor_lists(&host, driver, "sources")
            && !sources.is_empty()
            && !sources.iter().any(|source| source == wanted.source)
        {
            reasons.push(HostReason::Source);
        }
        if !wanted
            .labels
            .iter()
            .all(|label| host.labels.contains(label))
        {
            reasons.push(HostReason::Labels);
        }
        let storages = storages_of(&host);
        if !wanted
            .storages
            .iter()
            .all(|asked| storages.iter().any(|each| &each.name == asked))
        {
            reasons.push(HostReason::Storage);
        }
        let present = rows::mounts_present(conn, &wanted.mounts, host.machine_id).await?;
        let names = rows::mount_names(conn, &wanted.mounts).await?;
        let mounts: Vec<PlaceMount> = wanted
            .mounts
            .iter()
            .map(|mount| PlaceMount {
                mount: MountRef {
                    id: *mount,
                    name: names
                        .iter()
                        .find(|(id, _)| id == mount)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_default(),
                    address: String::new(),
                },
                present: present.contains(mount),
            })
            .collect();
        if mounts.iter().any(|mount| !mount.present) {
            reasons.push(HostReason::Mount);
        }
        let created = if retained_host == Some(host.id) {
            None
        } else {
            // What the creator's entries allow.
            let ceiling = Wants {
                resources: ResourceLimits {
                    cpu_millis: number(Some(&wanted.limits), "cpu_millis"),
                    memory_mb: number(Some(&wanted.limits), "memory_mb"),
                    pids: number(Some(&wanted.limits), "pids"),
                    disk_gb: number(Some(&wanted.limits), "disk_gb"),
                },
                capabilities: wanted.capabilities.clone(),
                source: Some(wanted.source.to_owned()),
                storage: wanted.storages.first().cloned(),
            };
            let request = NewEnvironment {
                home: ask.home,
                place: Place::Host {
                    host: host.id,
                    template: wanted.template.id,
                    wants: ceiling,
                },
                mounts: wanted.mounts.clone(),
            };
            let decision = match existing {
                Some(environment) => {
                    orch_authz::environments::check_existing_placement(
                        conn,
                        principal,
                        &request,
                        &policies,
                        environment,
                    )
                    .await?
                }
                None => check_creation(conn, principal, &request, &policies).await?,
            };
            match decision {
                Creating::Allowed(created) => Some(*created),
                Creating::Exceeds(limits) => {
                    reasons.extend(reasons_of(&limits));
                    None
                }
                Creating::Refused(_) | Creating::NotYourMachine => continue,
            }
        };
        // How full it is.
        let full = is_full(&capacity, &declared, full_percent);
        let no_room = lacks_room(&capacity, &declared, &wanted.needs);
        for (holds, reason, warning) in [
            (full, HostReason::Full, HostWarning::Full),
            (no_room, HostReason::NoRoom, HostWarning::NoRoom),
        ] {
            if holds {
                if when_full == WhenFull::Block {
                    reasons.push(reason);
                } else {
                    warnings.push(warning);
                }
            }
        }
        let load = &host.load;
        let environments = PlaceEnvironments {
            running: i64::try_from(
                held.iter()
                    .filter(|load| load.kind == "environment" && load.state == "running")
                    .count(),
            )
            .unwrap_or(i64::MAX),
            idle: i64::try_from(
                held.iter()
                    .filter(|load| {
                        load.kind == "environment"
                            && load.state == "running"
                            && load
                                .last_activity_at
                                .is_none_or(|at| (ask.now - at).num_minutes() >= ask.minutes_idle)
                    })
                    .count(),
            )
            .unwrap_or(i64::MAX),
        };
        let usable = reasons.is_empty();
        places.push(HostPlace {
            owner: host.owner_id,
            place: PlaceHost {
                host: HostRef {
                    id: host.id,
                    name: host.name.clone(),
                    address: String::new(),
                },
                labels: host.labels.clone(),
                owner: None,
                capacity: capacity.clone(),
                declared,
                free,
                used: PlaceUsed {
                    cpu_percent: load.get("cpu_percent").and_then(Json::as_f64),
                    memory_mb: number(Some(load), "memory_used_mb"),
                    disk_gb: number(Some(load), "disk_used_gb"),
                },
                when_full,
                environments,
                storages,
                mounts,
                usable,
                reasons,
                policy_refusal,
                warnings,
                current: ask.current == Some(host.id),
                automatic: false,
            },
            host,
            created,
        });
    }
    places.sort_by(|a, b| {
        b.place
            .free
            .memory_mb
            .cmp(&a.place.free.memory_mb)
            .then_with(|| a.place.host.id.cmp(&b.place.host.id))
    });
    if let Some(index) = automatic(&places) {
        places[index].place.automatic = true;
    }
    Ok(places)
}

/// Which host the scheduler chooses when none is named: among the usable ones those that have room
/// before those that only warn, and the one that declares the most free memory first; the list is in
/// that order already. None when no host can be chosen.
#[must_use]
pub fn automatic(places: &[HostPlace]) -> Option<usize> {
    places
        .iter()
        .position(|place| place.usable() && place.place.warnings.is_empty())
        .or_else(|| places.iter().position(HostPlace::usable))
}

/// Why no host takes an environment, with the places that were looked at.
#[derive(Debug, Clone, PartialEq)]
pub struct NoHost {
    /// `no_host` when nothing could make it, `no_capacity` when a host would but blocks for want of
    /// room.
    pub code: &'static str,
    /// What to tell, naming what each host lacks.
    pub message: String,
    /// Only hosts the scheduler already authorized, with the facts that blocked this plan.
    pub blockers: Vec<PlanHostBlocker>,
}

/// The selected entry filled after its lock-free preview and before creation reserved it.
pub(crate) fn capacity_reached(place: &HostPlace) -> NoHost {
    let mut full = place.clone();
    full.place.usable = false;
    full.place.reasons.push(HostReason::EntryLimit);
    NoHost {
        code: "no_capacity",
        message: lacks(&full),
        blockers: host_blockers(&[full], Some(place.host.id)),
    }
}

fn host_blockers(places: &[HostPlace], named: Option<Id>) -> Vec<PlanHostBlocker> {
    places
        .iter()
        .filter(|place| !place.usable() && named.is_none_or(|id| place.host.id == id))
        .map(|place| PlanHostBlocker {
            host: place.place.host.clone(),
            online: place.host.online,
            offline_since: if place.host.online {
                None
            } else {
                place.host.offline_since.map(Time::new)
            },
            reasons: place.place.reasons.clone(),
            message: lacks(place),
        })
        .collect()
}

fn lacks(place: &HostPlace) -> String {
    let words: Vec<&str> = place
        .place
        .reasons
        .iter()
        .map(|reason| match reason {
            HostReason::Driver => "its driver cannot make this template",
            HostReason::Source => "it does not offer the source the template names",
            HostReason::Labels => "it lacks a label the template asks for",
            HostReason::Mount => "a mount has no place on it",
            HostReason::Storage => "it lacks a storage the environment names",
            HostReason::Full => "it is full",
            HostReason::NoRoom => "it declares less free memory than the environment needs",
            HostReason::Unschedulable if !place.host.online => "its machine is offline",
            HostReason::Unschedulable => "it takes no new environment",
            HostReason::EntryLimit => "the entry the creator uses on it allows no more",
            HostReason::TemplateNotAllowed => {
                "the entry the creator uses on it does not allow this template"
            }
        })
        .collect();
    format!("{}: {}", place.place.host.name, words.join(", "))
}

fn blocked_for_room(place: &HostPlace) -> bool {
    place.place.reasons.iter().any(|reason| {
        matches!(
            reason,
            HostReason::Full | HostReason::NoRoom | HostReason::EntryLimit
        )
    })
}

/// Chooses the host an environment is made on: the one named, which must be usable (another is never
/// used instead, a named host being named for a reason), or the one the scheduler chooses. A host
/// that would take it and blocks for want of room is `no_capacity`; any other lack is `no_host`.
pub fn choose(places: &[HostPlace], named: Option<Id>) -> Result<&HostPlace, NoHost> {
    if let Some(named) = named {
        return match places.iter().find(|place| place.place.host.id == named) {
            None => Err(NoHost {
                code: "no_host",
                message: "the host that was named is not one its creator may use".to_owned(),
                blockers: Vec::new(),
            }),
            Some(place) if place.usable() => Ok(place),
            Some(place) => Err(NoHost {
                code: if blocked_for_room(place) {
                    "no_capacity"
                } else {
                    "no_host"
                },
                message: lacks(place),
                blockers: host_blockers(places, Some(named)),
            }),
        };
    }
    if let Some(index) = automatic(places) {
        return Ok(&places[index]);
    }
    if places.is_empty() {
        return Err(NoHost {
            code: "no_host",
            message: "its creator may use no host".to_owned(),
            blockers: Vec::new(),
        });
    }
    let room_only = places.iter().all(|place| {
        place.place.reasons.iter().all(|reason| {
            matches!(
                reason,
                HostReason::Full | HostReason::NoRoom | HostReason::EntryLimit
            )
        })
    });
    Err(NoHost {
        code: if room_only { "no_capacity" } else { "no_host" },
        message: places.iter().map(lacks).collect::<Vec<_>>().join("; "),
        blockers: host_blockers(places, None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn load(kind: &str, state: &str, spec: Json, config: Option<Json>) -> LoadRow {
        LoadRow {
            host_id: Id::nil(),
            runtime_id: Id::nil(),
            kind: kind.to_owned(),
            state: state.to_owned(),
            environment_id: None,
            spec,
            config,
            last_activity_at: Some(Utc::now()),
        }
    }

    #[test]
    fn current_loads_are_visited_once_and_every_host_keeps_its_capacity_totals() {
        for count in [1_000_usize, 10_000] {
            let visits = std::cell::Cell::new(0);
            let source = (0..count)
                .map(|index| {
                    let mut row = load(
                        "environment",
                        "running",
                        json!({"needs":{"memory_mb":2,"disk_gb":1}}),
                        None,
                    );
                    row.host_id = Id::from_u128((index % 10) as u128);
                    row
                })
                .inspect(|_| visits.set(visits.get() + 1));
            let mut grouped = loads_by_host(source);
            assert_eq!(visits.get(), count);
            assert_eq!(grouped.len(), 10);
            for host in 0..10_u128 {
                let Some(rows) = grouped.remove(&Id::from_u128(host)) else {
                    panic!("every host must keep its loads");
                };
                assert_eq!(rows.len(), count / 10);
                let totals = declared_of(&rows);
                assert_eq!(totals.memory_mb, (count / 5) as i64);
                assert_eq!(totals.disk_gb, (count / 10) as i64);
            }
            assert!(grouped.is_empty());
        }
    }

    #[test]
    fn an_environment_declares_what_it_says_then_what_its_template_says_then_its_limits() {
        let spec = json!({
            "needs": { "memory_mb": 2048, "cpu_millis": 1000, "disk_gb": 10 },
            "resources": { "cpu_millis": 4000, "memory_mb": 8192, "disk_gb": 50 }
        });
        let own = json!({ "memory_mb": 512 });
        let needs = needs_of(&spec, Some(&own), None);
        assert_eq!(
            (needs.memory_mb, needs.cpu_millis, needs.disk_gb),
            (512, 1000, 10)
        );
        let bare = json!({ "resources": { "memory_mb": 4096 } });
        assert_eq!(
            needs_of(&bare, None, None).memory_mb,
            4096,
            "no smaller need stated: its limit counts"
        );
    }

    #[test]
    fn a_host_declares_the_memory_of_the_units_that_hold_it_and_the_disk_of_every_environment() {
        let spec = json!({ "needs": { "memory_mb": 2048, "cpu_millis": 1000, "disk_gb": 10 } });
        let declared = declared_of(&[
            load("environment", "running", spec.clone(), None),
            load("environment", "stopped", spec.clone(), None),
            load(
                "provider",
                "running",
                json!({ "resources": { "memory_mb": 1024, "cpu_millis": 500 } }),
                None,
            ),
        ]);
        assert_eq!(
            declared.memory_mb,
            2048 + 1024,
            "a stopped unit holds no memory"
        );
        assert_eq!(declared.cpu_millis, 1000 + 500);
        assert_eq!(
            declared.disk_gb, 20,
            "a stopped unit still has its volumes; this provider declares no disk need"
        );
    }

    #[test]
    fn provider_runtime_needs_override_its_limits_and_its_stopped_state_volume_still_counts_disk() {
        let spec = json!({"resources":{"memory_mb":4096,"cpu_millis":2000},"needs":{"disk_gb":3}});
        let own = json!({"runtime.needs":{"memory_mb":512,"cpu_millis":250,"disk_gb":2},"runtime.resources":{"memory_mb":8192}});
        let declared = declared_of(&[
            load("provider", "running", spec.clone(), Some(own.clone())),
            load("provider", "stopped", spec, Some(own)),
        ]);
        assert_eq!(
            (declared.memory_mb, declared.cpu_millis, declared.disk_gb),
            (512, 250, 4)
        );
    }

    #[test]
    fn a_host_is_full_at_the_share_of_its_memory_that_is_declared_and_has_no_room_below_what_is_needed()
     {
        let capacity = Amounts {
            cpu_millis: 8000,
            memory_mb: 10_000,
            disk_gb: 100,
        };
        let declared = |memory_mb| Amounts {
            cpu_millis: 0,
            memory_mb,
            disk_gb: 0,
        };
        assert!(!is_full(&capacity, &declared(8_999), 90));
        assert!(is_full(&capacity, &declared(9_000), 90));
        let needs = Needs {
            cpu_millis: 1000,
            memory_mb: 2048,
            disk_gb: 10,
        };
        assert!(!lacks_room(&capacity, &declared(7_952), &needs));
        assert!(lacks_room(&capacity, &declared(7_953), &needs));
        let unknown = Amounts {
            cpu_millis: 0,
            memory_mb: 0,
            disk_gb: 0,
        };
        assert!(
            !is_full(&unknown, &declared(5_000), 90)
                && !lacks_room(&unknown, &declared(5_000), &needs)
        );
    }

    #[test]
    fn the_limits_of_a_unit_are_its_templates_changed_by_its_own_and_kept_within_the_templates_most()
     {
        let spec = json!({
            "resources": { "cpu_millis": 4000, "memory_mb": 8192 },
            "resources_max": { "cpu_millis": 16000, "memory_mb": 16384 }
        });
        let limits = limits_of(
            &spec,
            Some(&json!({ "memory_mb": 32768, "cpu_millis": 2000 })),
        );
        assert_eq!(limits["memory_mb"], 16384);
        assert_eq!(limits["cpu_millis"], 2000);
    }
}
