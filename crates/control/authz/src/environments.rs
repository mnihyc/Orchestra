//! An environment's page of two levels and four doors, whose Manage row takes entries on a person's machine as anywhere; managing as deciding where threads run in it; the defaults its page takes by where it runs; everybody who can reach into it, by its doors and by an agent there that they may talk to; and a shell as its opener's alone, closed by whoever manages the environment.
//!
//! An environment is a resource like any other, with two levels: looking at it, and having all of
//! it. Whoever runs a thread in one reaches all of it through that thread's agent, so there is no
//! permission to run a thread in an environment without managing it, and where a thread runs is its
//! managers' to say: putting a thread into it, giving it another directory there, taking it out,
//! and how far its agent is confined need `environment.manage` and nothing on the thread but
//! seeing it. Managing a thread gives none of that.
//!
//! Its page has four doors besides: Files, Terminal, Browsers and VNC, each to look or to reach in,
//! and whoever manages the environment holds the Manage of all four without being named in them.
//! They are the environment's and on no thread's page, because they open the whole room: whoever
//! holds `files.view` sees the files of every thread that runs there, whatever topics and orgs the
//! threads belong to. [`reach`] says who reaches into an environment and by what, its doors and the
//! agents there that people may talk to; [`reaches`] says how many people a thread's page does not
//! keep out of its files. A shell is its opener's alone; whoever manages the environment closes
//! it, and it is closed when what opened it stops holding.
//!
//! Creating one from a template on a host takes the use of both, each within the options of the
//! entry it is held by, and the narrower of the two limits counts ([`check_creation`]). The
//! defaults a thread's page follows are the row that fits where it runs, which the engine reads
//! from the environment the thread is in.

use std::collections::{BTreeMap, HashMap, HashSet};

use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{
    HostUse, NodePermission, ObjectKind, Options, Permission, ResourceKind, ResourceLimits,
    TemplateUse,
};
use orch_access_contract::roles::Role;
use orch_db::Conn;
use orch_db::queries::environments as rows;
use orch_types::Id;

use crate::engine::{Frame, Graph, Site};
use crate::holders;
use crate::resources::{self, Lacking, Need, Needs, assemble, read_into, site_of};
use crate::usable::{Policies, Use, access};
use crate::{Error, Policy, Principal};

mod reach_counts;
pub use reach_counts::reach_counts;

// ---- Deciding where threads run ------------------------------------------------------------------

/// A change of where a thread runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moving {
    /// The thread; none for a thread that is being made (created, or forked), which has nothing yet
    /// to be moved out of.
    pub thread: Option<Id>,
    /// The environment it runs in now, if any.
    pub from: Option<Id>,
    /// The existing environment it goes to or is put into, if any. The same as `from` for a change
    /// of directory or of how far its agent is confined. A new environment is
    /// [`check_creation`]'s.
    pub to: Option<Id>,
    /// Whether the request names all the threads of the environment together, one of which the
    /// caller may not see: then they need not be able to see the thread.
    pub all_of_the_environment: bool,
}

/// Whether the caller may decide where the thread runs: `environment.manage` on the environment it
/// is in (`thread.manage` instead, for a thread that is in none) and on the one it goes to; and the
/// thread is one they can see, unless the request names all the threads of an environment they
/// manage. Nothing is asked of the thread but seeing it, and nobody's access but the caller's.
pub async fn check_move(
    conn: &mut Conn,
    principal: &Principal,
    moving: &Moving,
    policy: Policy,
) -> Result<Decision, Error> {
    let environment = |id: Id| ObjectRef {
        kind: ObjectKind::Environment,
        id,
    };
    let mut items: Vec<Need> = Vec::new();
    match (moving.thread, moving.from) {
        (Some(_), Some(from)) => items.push(Need {
            object: environment(from),
            permissions: vec![Permission::EnvironmentManage],
            options: None,
        }),
        (Some(thread), None) => items.push(Need {
            object: ObjectRef {
                kind: ObjectKind::Thread,
                id: thread,
            },
            permissions: vec![Permission::ThreadManage],
            options: None,
        }),
        (None, _) => {}
    }
    if let Some(to) = moving.to
        && moving.from != Some(to)
    {
        items.push(Need {
            object: environment(to),
            permissions: vec![Permission::EnvironmentManage],
            options: None,
        });
    }
    if let Some(thread) = moving.thread
        && !moving.all_of_the_environment
    {
        items.push(Need {
            object: ObjectRef {
                kind: ObjectKind::Thread,
                id: thread,
            },
            permissions: vec![Permission::ThreadView],
            options: None,
        });
    }
    let Some(anchor) = items.first().map(|item| item.object) else {
        return Ok(Decision::Allowed {
            through: Vec::new(),
        });
    };
    let required = if anchor.kind == ObjectKind::Environment {
        Permission::EnvironmentManage
    } else {
        Permission::ThreadManage
    };
    resources::needs(
        conn,
        principal,
        &Needs {
            required,
            anchor,
            through: moving.thread,
            items,
        },
        policy,
    )
    .await
}

// ---- Creating one --------------------------------------------------------------------------------

/// What an environment made from a template is to be given, to be held against the entries it is
/// made through.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wants {
    /// The most it is given.
    pub resources: ResourceLimits,
    /// The capabilities it may ever be given.
    pub capabilities: Vec<String>,
    /// The source its template names: `image`, `build` or `archive`.
    pub source: Option<String>,
    /// The storage its volumes are created on; none: the host's default.
    pub storage: Option<String>,
}

/// Where a new environment comes to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Made from a template on a host.
    Host {
        /// The host.
        host: Id,
        /// The template.
        template: Id,
        /// What it is to be given.
        wants: Wants,
    },
    /// A directory of a machine the caller may use.
    Machine {
        /// The machine.
        machine: Id,
    },
}

/// An environment to be created, and what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEnvironment {
    /// The node whose settings it takes: the topic of the thread it is made for, or the node it is
    /// created on. The caller needs `node.interact` there.
    pub home: Id,
    /// Where it comes to be.
    pub place: Place,
    /// The mounts it attaches; the caller needs `mount.use` for each.
    pub mounts: Vec<Id>,
}

/// What a limit of an entry held an environment to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Limit {
    /// The host's entry allows so many environments at one time, and that many exist.
    TooManyEnvironments {
        /// What the entry allows.
        allowed: u32,
        /// How many exist that were made through it.
        made: i64,
    },
    /// The environment would be given more of a resource than the narrower of the two entries
    /// allows.
    ResourcesAbove {
        /// Which: `cpu_millis`, `memory_mb`, `pids` or `disk_gb`.
        resource: &'static str,
        /// What is allowed.
        allowed: i64,
        /// What was asked.
        wanted: i64,
    },
    /// The template's entry does not let the environment ever be given that capability.
    CapabilityNotAllowed {
        /// The capability.
        capability: String,
    },
    /// The host's entry does not let the template name that source.
    SourceNotAllowed {
        /// The source.
        source: String,
    },
    /// The host's entry does not let volumes be created on that storage.
    StorageNotAllowed {
        /// The storage.
        storage: String,
    },
    /// The host's entry lists the templates that may be used on it, and this is none of them.
    TemplateNotAllowedOnTheHost {
        /// The template.
        template: Id,
    },
}

/// What a creation was let through by, to be recorded on the environment: its ceiling stays what
/// these entries allowed until it is moved or its template is changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Created {
    /// The physical host or machine Use entry and its combined limits; the inner entry is
    /// none for an owner, who is held by no entry.
    pub host: Option<(Option<Id>, HostUse)>,
    /// The same for the template.
    pub template: Option<(Option<Id>, TemplateUse)>,
    /// The most the environment may be given: the narrower of the two limits.
    pub ceiling: ResourceLimits,
    /// The bounded physical-use audience whose capacity creation must reserve.
    pub capacity: Option<CreationCapacity>,
}

/// The exact physical-Use entries whose combined limit admitted this creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationCapacity {
    /// All entry identifiers counted by the combined allowance.
    pub entries: Vec<Id>,
    /// Maximum undeleted registrations through those entries.
    pub maximum: u32,
    /// The authorized existing environment, whose persisted reservation is not a new slot.
    pub existing: Option<Id>,
}

/// Rechecks the bounded admission under its existing permission-entry row owners.
/// Preview checks remain lock-free. Call only in the creation transaction, and
/// retain it through insertion of the environment and its entry provenance.
pub async fn reserve_capacity(conn: &mut Conn, created: &Created) -> Result<bool, Error> {
    let Some(capacity) = &created.capacity else {
        return Ok(true);
    };
    rows::lock_environment_entries(conn, &capacity.entries).await?;
    Ok(
        rows::other_environments_made_through(conn, &capacity.entries, capacity.existing).await?
            < i64::from(capacity.maximum),
    )
}

/// Whether an environment may be created.
#[derive(Debug, Clone, PartialEq)]
pub enum Creating {
    /// It may.
    Allowed(Box<Created>),
    /// The caller lacks something: what, thing by thing, and who can give it.
    Refused(Box<Refusal>),
    /// They hold what is needed, and the entries they hold it by do not let this environment be
    /// made as it is asked. The managers of the host or the template can widen them.
    Exceeds(Vec<Limit>),
    /// The directory's physical machine grants this caller no Use.
    NotYourMachine,
}

fn check_limit(
    resource: &'static str,
    allowed: Option<i64>,
    wanted: Option<i64>,
    exceeded: &mut Vec<Limit>,
) {
    if let (Some(allowed), Some(wanted)) = (allowed, wanted)
        && wanted > allowed
    {
        exceeded.push(Limit::ResourcesAbove {
            resource,
            allowed,
            wanted,
        });
    }
}

/// Whether the caller may create an environment: a directory needs the physical machine's
/// Use permission and `node.interact` where it takes settings; a template-based environment needs
/// host, `host.use` on the host and `template.use` on the template, each within the options of the
/// entry it is held by (a template of the creator's own counts, and the host's entry then says how
/// large it may be), `mount.use` for each mount, and `node.interact` on that node. The narrower
/// of the two limits counts.
pub async fn check_creation(
    conn: &mut Conn,
    principal: &Principal,
    ask: &NewEnvironment,
    policies: &Policies,
) -> Result<Creating, Error> {
    check_placement_inner(conn, principal, ask, policies, None).await
}

/// An existing environment retains its identity and home. Its current Manage
/// permission authorizes placement; creating unrelated work there is not required.
pub async fn check_existing_placement(
    conn: &mut Conn,
    principal: &Principal,
    ask: &NewEnvironment,
    policies: &Policies,
    existing: Id,
) -> Result<Creating, Error> {
    check_placement_inner(conn, principal, ask, policies, Some(existing)).await
}

async fn check_placement_inner(
    conn: &mut Conn,
    principal: &Principal,
    ask: &NewEnvironment,
    policies: &Policies,
    existing: Option<Id>,
) -> Result<Creating, Error> {
    let mut graph = Graph::load(conn, &[ask.home]).await?;
    let home = graph
        .get(ask.home)
        .cloned()
        .ok_or(Error::NotFound(ask.home))?;
    let home_site = graph.site_of(ask.home).ok_or(Error::NotFound(ask.home))?;
    let interact = NodePermission::Interact
        .on(home.kind)
        .ok_or(Error::NotFound(ask.home))?;
    let mut ids: Vec<Id> = ask.mounts.clone();
    ids.extend(existing);
    let machine_resource = if let Place::Machine { machine } = &ask.place {
        let location = rows::location_machine(conn, *machine)
            .await?
            .ok_or(Error::NotFound(*machine))?;
        Some(match location.host_id {
            Some(host) => (host, ResourceKind::Host),
            None => (*machine, ResourceKind::Machine),
        })
    } else {
        None
    };
    if let Place::Host { host, template, .. } = &ask.place {
        ids.extend([*host, *template]);
    }
    ids.extend(machine_resource.map(|(id, _)| id));
    let found = read_into(conn, &mut graph, &ids).await?;
    let pick = |id: Id, kind: ResourceKind| {
        found
            .iter()
            .find(|resource| resource.id == id && resource.kind == kind)
            .cloned()
            .ok_or(Error::NotFound(id))
    };
    let (admission_site, admission_permission) = if let Some(id) = existing {
        let resource = pick(id, ResourceKind::Environment)?;
        if resource.deleted || resource.home != Some(ask.home) {
            return Err(Error::NotFound(id));
        }
        (
            site_of(&graph, &resource).ok_or(Error::NotFound(id))?,
            Permission::EnvironmentManage,
        )
    } else {
        (home_site.clone(), interact)
    };
    let mut mounts = Vec::new();
    for mount in &ask.mounts {
        mounts.push(pick(*mount, ResourceKind::Mount)?);
    }
    let (host, template) = match &ask.place {
        Place::Host { host, template, .. } => (
            Some(pick(*host, ResourceKind::Host)?),
            Some(pick(*template, ResourceKind::Template)?),
        ),
        Place::Machine { .. } => {
            let (id, kind) = machine_resource.ok_or(Error::NotFound(ask.home))?;
            (Some(pick(id, kind)?), None)
        }
    };
    let sites: Vec<(crate::resources::Resource, Site)> = [host.as_ref(), template.as_ref()]
        .into_iter()
        .flatten()
        .chain(mounts.iter())
        .filter_map(|resource| site_of(&graph, resource).map(|site| (resource.clone(), site)))
        .collect();
    let wanted = [
        interact,
        Permission::HostUse,
        Permission::MachineUse,
        Permission::EnvironmentManage,
        Permission::TemplateUse,
        Permission::MountUse,
    ];
    let mut all_sites: Vec<Site> = vec![home_site.clone(), admission_site.clone()];
    all_sites.extend(sites.iter().map(|(_, site)| site.clone()));
    let policy = Policy::default();
    let frame = Frame::load(conn, principal, policy, graph, &all_sites, &wanted).await?;
    let path = home.path.clone();
    let mut lacking: Vec<Lacking> = Vec::new();
    if let Some(closing) = principal.suspension().closing(&home_site.orgs) {
        return Err(Error::Suspended { org: closing.org() });
    }
    if frame.ways(&admission_site, admission_permission).is_empty() {
        lacking.push(Lacking {
            site: admission_site.clone(),
            permissions: vec![admission_permission],
            options: None,
            closed: None,
        });
    }
    let mut held: HashMap<Id, Use> = HashMap::new();
    for (resource, site) in &sites {
        let kept = policies.at(resource.home).keeps_to_its_owner(resource);
        match access(&frame, resource, site, Some(&path), kept)? {
            Ok(granted) => {
                held.insert(resource.id, granted);
            }
            Err(_) if machine_resource.is_some_and(|(id, _)| id == resource.id) => {
                return Ok(Creating::NotYourMachine);
            }
            Err(_) => lacking.push(Lacking {
                site: site.clone(),
                permissions: vec![
                    crate::resources::use_permission(resource.kind).unwrap_or(Permission::HostUse),
                ],
                options: None,
                closed: None,
            }),
        }
    }
    if !lacking.is_empty() {
        let graph = frame.graph().clone();
        let refusal = assemble(
            conn,
            principal,
            policy,
            graph,
            &admission_site,
            &lacking,
            admission_permission,
        )
        .await?;
        return Ok(Creating::Refused(Box::new(refusal)));
    }
    // Use the same physical-entry allowance for directory registration and managed creation.
    if let Some((id, _)) = machine_resource {
        let granted = held.get(&id).ok_or(Error::NotFound(id))?;
        let options = match &granted.options {
            Options::HostUse(options) => options.clone(),
            _ => HostUse::default(),
        };
        if let Some(allowed) = options.max_environments {
            let made =
                rows::other_environments_made_through(conn, &granted.entries, existing).await?;
            if made >= i64::from(allowed) {
                return Ok(Creating::Exceeds(vec![Limit::TooManyEnvironments {
                    allowed,
                    made,
                }]));
            }
        }
        return Ok(Creating::Allowed(Box::new(Created {
            ceiling: options.resources_max.unwrap_or_default(),
            capacity: options.max_environments.map(|maximum| CreationCapacity {
                entries: granted.entries.clone(),
                maximum,
                existing,
            }),
            host: Some((granted.entry, options)),
            template: None,
        })));
    }
    // Within the options of the entries they hold it by.
    let Place::Host {
        host: host_id,
        template: template_id,
        wants,
    } = &ask.place
    else {
        return Ok(Creating::Allowed(Box::new(Created {
            host: None,
            template: None,
            ceiling: ResourceLimits::default(),
            capacity: None,
        })));
    };
    let host_use = held.get(host_id).ok_or(Error::NotFound(*host_id))?;
    let template_use = held.get(template_id).ok_or(Error::NotFound(*template_id))?;
    let host_options = match &host_use.options {
        Options::HostUse(options) => options.clone(),
        _ => HostUse::default(),
    };
    let template_options = match &template_use.options {
        Options::TemplateUse(options) => options.clone(),
        _ => TemplateUse::default(),
    };
    let mut exceeded: Vec<Limit> = Vec::new();
    if let Some(allowed) = host_options.max_environments {
        let made = rows::other_environments_made_through(conn, &host_use.entries, existing).await?;
        if made >= i64::from(allowed) {
            exceeded.push(Limit::TooManyEnvironments { allowed, made });
        }
    }
    let host_limits = host_options.resources_max.unwrap_or_default();
    let template_limits = template_options.resources_max.unwrap_or_default();
    let ceiling = host_limits.and(&template_limits);
    check_limit(
        "cpu_millis",
        ceiling.cpu_millis,
        wants.resources.cpu_millis,
        &mut exceeded,
    );
    check_limit(
        "memory_mb",
        ceiling.memory_mb,
        wants.resources.memory_mb,
        &mut exceeded,
    );
    check_limit("pids", ceiling.pids, wants.resources.pids, &mut exceeded);
    check_limit(
        "disk_gb",
        ceiling.disk_gb,
        wants.resources.disk_gb,
        &mut exceeded,
    );
    for capability in &wants.capabilities {
        if !template_options.allows_capability(capability) {
            exceeded.push(Limit::CapabilityNotAllowed {
                capability: capability.clone(),
            });
        }
    }
    if let Some(source) = &wants.source
        && !host_options.allows_source(source)
    {
        exceeded.push(Limit::SourceNotAllowed {
            source: source.clone(),
        });
    }
    if let Some(storage) = &wants.storage
        && !host_options.allows_storage(storage)
    {
        exceeded.push(Limit::StorageNotAllowed {
            storage: storage.clone(),
        });
    }
    if !host_options.allows_template(*template_id) {
        exceeded.push(Limit::TemplateNotAllowedOnTheHost {
            template: *template_id,
        });
    }
    if !exceeded.is_empty() {
        return Ok(Creating::Exceeds(exceeded));
    }
    Ok(Creating::Allowed(Box::new(Created {
        capacity: host_options
            .max_environments
            .map(|maximum| CreationCapacity {
                entries: host_use.entries.clone(),
                maximum,
                existing,
            }),
        host: Some((host_use.entry, host_options)),
        template: Some((template_use.entry, template_options)),
        ceiling,
    })))
}

// ---- Who reaches into one ------------------------------------------------------------------------

/// How a person reaches into an environment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reaching {
    /// They own it.
    Owner,
    /// They own the environment's physical supplying resource.
    SourceOwner,
    /// Its own page names them: for managing it, which holds all four doors, or for one of its four
    /// doors, of which the highest level they hold is named, once for each door.
    Page(Permission),
    /// A thread that runs there lets them talk to its agent (`thread.interact`), which does there
    /// whatever it is asked.
    Thread(Id),
}

/// A person who reaches into an environment, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reacher {
    /// The person.
    pub user: Id,
    /// How, each way once.
    pub through: Vec<Reaching>,
}

/// Everybody who reaches into an environment now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// The people, by identifier, each with the ways the reader may see.
    pub people: Vec<Reacher>,
    /// How many people reach it only through threads the reader may not view: counted, and not
    /// named.
    pub hidden: i64,
}

/// What a thread's page does not keep: its environment, which whoever looks into it reads, and how
/// much else is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaches {
    /// The environment the thread runs in.
    pub environment: Id,
    /// How many people look into it: those who hold its `files.view`, its owners among them.
    pub people: i64,
    /// How many other threads run there.
    pub threads: i64,
}

/// What an environment's own page lets somebody in by: managing it, and each of its four doors at
/// its two levels, the higher level of a door before the lower.
const PAGE_GATES: [Permission; 9] = [
    Permission::EnvironmentManage,
    Permission::FilesManage,
    Permission::FilesView,
    Permission::TerminalManage,
    Permission::TerminalView,
    Permission::BrowserManage,
    Permission::BrowserView,
    Permission::VncManage,
    Permission::VncView,
];

/// The four doors, each as its two levels: the Manage row, and the View row it holds.
const DOORS: [(Permission, Permission); 4] = [
    (Permission::FilesManage, Permission::FilesView),
    (Permission::TerminalManage, Permission::TerminalView),
    (Permission::BrowserManage, Permission::BrowserView),
    (Permission::VncManage, Permission::VncView),
];

/// What one person's rows of an environment's page say of how they reach into it: managing the
/// environment says all four doors, and the Manage of a door says its View.
fn highest(held: &HashSet<Permission>) -> Vec<Permission> {
    if held.contains(&Permission::EnvironmentManage) {
        return vec![Permission::EnvironmentManage];
    }
    DOORS
        .iter()
        .filter_map(|(manage, view)| {
            if held.contains(manage) {
                Some(*manage)
            } else if held.contains(view) {
                Some(*view)
            } else {
                None
            }
        })
        .collect()
}

/// The holders that name a person for `permission` on `site`, in its row and in the rows that hold
/// it, with a role that names no place resolved to where the thing is.
fn named_for(frame: &Frame<'_>, site: &Site, permission: Permission) -> Vec<Holder> {
    let now = frame.now();
    let mut named: Vec<Holder> = Vec::new();
    for row in std::iter::once(permission).chain(permission.held_through()) {
        named.extend(
            frame
                .chain(site, row)
                .holders(now)
                .into_iter()
                .map(|found| match found.entry.holder {
                    Holder::Role { role, at: None } => Holder::Role {
                        role,
                        at: site.role_place,
                    },
                    other => other,
                }),
        );
    }
    named.sort_by_key(|holder| format!("{holder:?}"));
    named.dedup();
    named
}

/// What an environment's `files.view` reaches, counted: how many people hold it, and how many other
/// threads work there. Nothing for a thread that is in no environment.
pub async fn reaches(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    policy: Policy,
) -> Result<Option<Reaches>, Error> {
    let Some((_, environment)) = rows::environments_of_threads(conn, &[thread])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let sharing = rows::threads_in(conn, &[environment]).await?;
    let others = sharing
        .iter()
        .filter(|other| other.thread_id != thread)
        .count();
    let object = ObjectRef {
        kind: ObjectKind::Environment,
        id: environment,
    };
    let (graph, _, site) = resources::site(conn, object).await?;
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &[Permission::FilesView],
    )
    .await?;
    let mut people: HashSet<Id> = HashSet::new();
    for holder in named_for(&frame, &site, Permission::FilesView) {
        people.extend(holders::people_of(conn, &frame, &site, &holder).await?);
    }
    people.extend(holders::owner_people(conn, &frame, site.id, site.root).await?);
    if let Some(source) = frame.ownership_source(&site) {
        let owners = holders::owner_people(conn, &frame, source.id, source.root).await?;
        people.extend(orch_db::queries::access::active_among(conn, &owners, &source.orgs).await?);
    }
    let candidates: Vec<Id> = people.into_iter().collect();
    let active: HashSet<Id> = orch_db::queries::access::active_among(conn, &candidates, &site.orgs)
        .await?
        .into_iter()
        .collect();
    Ok(Some(Reaches {
        environment,
        people: i64::try_from(active.len()).unwrap_or(i64::MAX),
        threads: i64::try_from(others).unwrap_or(i64::MAX),
    }))
}

/// Everybody who can reach into the environment `environment` now, and how: as its owners; through
/// its own page, by managing it or by one of its four doors; or through a thread that runs in it,
/// whose agent they may talk to and have act there (they also use the environment), which does there
/// whatever it is asked. Whatever topics and orgs the
/// threads belong to, and whether or not the person can see those threads at all. A person who
/// reaches it only through threads the reader may not view is counted and not named. Only people who
/// are not suspended where the thing that lets them in is are listed.
///
/// The statements depend on how many different holders the pages of its threads name, and not on
/// how many threads there are.
pub async fn reach(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    policy: Policy,
) -> Result<Reach, Error> {
    let object = ObjectRef {
        kind: ObjectKind::Environment,
        id: environment,
    };
    let (mut graph, _, env_site) = resources::site(conn, object).await?;
    let threads = rows::threads_in(conn, &[environment]).await?;
    let ids: Vec<Id> = threads.iter().map(|thread| thread.thread_id).collect();
    graph.extend(conn, &ids).await?;
    let thread_sites: Vec<Site> = ids.iter().filter_map(|id| graph.site_of(*id)).collect();
    let mut sites = vec![env_site.clone()];
    sites.extend(thread_sites.iter().cloned());
    let mut wanted = vec![
        Permission::ThreadView,
        Permission::ThreadInteract,
        Permission::EnvironmentUse,
    ];
    wanted.extend(PAGE_GATES);
    let frame = Frame::load(conn, principal, policy, graph, &sites, &wanted).await?;
    // Talking to an agent reaches in only for those who may use the environment: whoever its Use row (or
    // a level above it) names, and the drivers of the work in a place it names.
    let use_holders = named_for(&frame, &env_site, Permission::EnvironmentUse);
    let work_places: Vec<Id> = use_holders
        .iter()
        .filter_map(|holder| match holder {
            Holder::Node { id } => Some(*id),
            _ => None,
        })
        .collect();
    let mut env_users: HashSet<Id> = HashSet::new();
    for holder in use_holders
        .iter()
        .filter(|holder| !matches!(holder, Holder::Node { .. }))
    {
        env_users.extend(holders::people_of(conn, &frame, &env_site, holder).await?);
    }

    // What each thing lets in: the holders its page names for the gate and the levels above it, and
    // its owners.
    #[derive(Debug)]
    struct Door {
        site: usize,
        permission: Permission,
        holders: Vec<Holder>,
    }
    let mut doors: Vec<Door> = Vec::new();
    let mut gates: Vec<(usize, Permission)> = PAGE_GATES.iter().map(|gate| (0, *gate)).collect();
    gates.extend((1..sites.len()).map(|index| (index, Permission::ThreadInteract)));
    for (index, permission) in gates {
        doors.push(Door {
            site: index,
            permission,
            holders: named_for(&frame, &sites[index], permission),
        });
    }
    // The people behind each distinct holder, found once.
    let mut people_of: HashMap<(Holder, Option<Id>), Vec<Id>> = HashMap::new();
    for door in &doors {
        for holder in &door.holders {
            let site = &sites[door.site];
            let role_place = if matches!(holder, Holder::Role { at: None, .. }) {
                site.role_place
            } else {
                None
            };
            let key = (*holder, role_place);
            if let std::collections::hash_map::Entry::Vacant(e) = people_of.entry(key) {
                let people = holders::people_of(conn, &frame, &sites[door.site], holder).await?;
                e.insert(people);
            }
        }
    }
    // The owners of each thing, who count while they are members of its root org.
    let mut owners_of: Vec<Vec<Id>> = Vec::with_capacity(sites.len());
    for site in &sites {
        owners_of.push(holders::owner_people(conn, &frame, site.id, site.root).await?);
    }
    let source_owners = if let Some(source) = frame.ownership_source(&env_site) {
        let owners = holders::owner_people(conn, &frame, source.id, source.root).await?;
        orch_db::queries::access::active_among(conn, &owners, &source.orgs).await?
    } else {
        Vec::new()
    };
    env_users.extend(owners_of[0].iter().copied());
    env_users.extend(source_owners.iter().copied());
    // Whether somebody who may talk to the agent of the thread at `index` has it act in the environment.
    let uses_here = |index: usize, user: Id| -> bool {
        env_users.contains(&user)
            || sites[index]
                .path
                .as_ref()
                .is_some_and(|path| work_places.iter().any(|place| path.contains(place)))
    };
    // Who is not suspended where each thing lies, found once for each different way up.
    let mut candidates: HashSet<Id> = HashSet::new();
    for people in people_of.values().chain(owners_of.iter()) {
        candidates.extend(people.iter().copied());
    }
    candidates.extend(source_owners.iter().copied());
    let candidate_list: Vec<Id> = candidates.into_iter().collect();
    let mut active: HashMap<Vec<Id>, HashSet<Id>> = HashMap::new();
    for site in &sites {
        if !active.contains_key(&site.orgs) {
            let found =
                orch_db::queries::access::active_among(conn, &candidate_list, &site.orgs).await?;
            active.insert(site.orgs.clone(), found.into_iter().collect());
        }
    }
    let is_active = |index: usize, user: Id| -> bool {
        active
            .get(&sites[index].orgs)
            .is_some_and(|set| set.contains(&user))
    };

    // Who reaches through what: for an environment's own page, the rows that name them; for a
    // thread, the index of the thread among those that run there, which is what the reader's view of
    // it is asked by.
    let mut reaching: BTreeMap<Id, Vec<(Option<usize>, Reaching)>> = BTreeMap::new();
    for user in source_owners {
        if is_active(0, user) {
            reaching
                .entry(user)
                .or_default()
                .push((None, Reaching::SourceOwner));
        }
    }
    for user in &owners_of[0] {
        if is_active(0, *user) {
            reaching
                .entry(*user)
                .or_default()
                .push((None, Reaching::Owner));
        }
    }
    for door in &doors {
        for holder in &door.holders {
            let site = &sites[door.site];
            let role_place = if matches!(holder, Holder::Role { at: None, .. }) {
                site.role_place
            } else {
                None
            };
            for user in people_of.get(&(*holder, role_place)).into_iter().flatten() {
                if !is_active(door.site, *user) || (door.site > 0 && !uses_here(door.site, *user)) {
                    continue;
                }
                let way = if door.site == 0 {
                    Reaching::Page(door.permission)
                } else {
                    Reaching::Thread(sites[door.site].id)
                };
                reaching
                    .entry(*user)
                    .or_default()
                    .push((door.site.checked_sub(1), way));
            }
        }
    }
    // The owners of a thread hold every permission of its page, and so talk to its agent.
    for (index, owners) in owners_of.iter().enumerate().skip(1) {
        for user in owners {
            if is_active(index, *user) && uses_here(index, *user) {
                reaching
                    .entry(*user)
                    .or_default()
                    .push((Some(index - 1), Reaching::Thread(sites[index].id)));
            }
        }
    }
    // What the reader may see of it.
    let viewable: Vec<bool> = sites
        .iter()
        .skip(1)
        .map(|site| !frame.ways(site, Permission::ThreadView).is_empty())
        .collect();
    let mut people: Vec<Reacher> = Vec::new();
    let mut hidden = 0_i64;
    for (user, mut ways) in reaching {
        ways.sort();
        ways.dedup();
        // However many rows of its page name the person, they reach in by the highest of each door.
        let named: HashSet<Permission> = ways
            .iter()
            .filter_map(|(_, way)| match way {
                Reaching::Page(permission) => Some(*permission),
                _ => None,
            })
            .collect();
        let by_page = highest(&named);
        let mut shown: Vec<Reaching> = by_page.into_iter().map(Reaching::Page).collect();
        let mut hid_some = false;
        for (index, way) in ways {
            match (&way, index) {
                (Reaching::Page(_), _) => {}
                (_, Some(index)) if !viewable.get(index).copied().unwrap_or(false) => {
                    hid_some = true;
                }
                _ => shown.push(way),
            }
        }
        if shown.is_empty() {
            if hid_some {
                hidden += 1;
            }
            continue;
        }
        shown.sort();
        shown.dedup();
        people.push(Reacher {
            user,
            through: shown,
        });
    }
    Ok(Reach { people, hidden })
}

// ---- Shells --------------------------------------------------------------------------------------

/// An interactive shell, as far as who may do what to it is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shell {
    /// The environment it runs in.
    pub environment: Id,
    /// The thread it was opened beside, which only says where; none: from the environment's own
    /// page. It gives its opener nothing.
    pub thread: Option<Id>,
    /// Who opened it, and is the one who attaches to it.
    pub opened_by: Id,
}

/// What a shell was opened by, which it is closed without when that stops holding: `terminal.manage`
/// on the environment, whichever thread it was opened beside. The thread says only where it was
/// opened, and gives nothing.
fn door_of(shell: &Shell) -> (ObjectRef, Permission) {
    (
        ObjectRef {
            kind: ObjectKind::Environment,
            id: shell.environment,
        },
        Permission::TerminalManage,
    )
}

async fn holds_door(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    permission: Permission,
    policy: Policy,
) -> Result<bool, Error> {
    let decision = if object.kind.node().is_some() {
        crate::nodes::check(
            conn,
            principal,
            object.id,
            permission,
            crate::Act::Use,
            policy,
        )
        .await
    } else {
        resources::check(conn, principal, object, permission, policy).await
    };
    match decision {
        Ok(decision) => Ok(decision.is_allowed()),
        Err(Error::Suspended { .. } | Error::Archived { .. } | Error::NotFound(_)) => Ok(false),
        Err(other) => Err(other),
    }
}

/// Whether the caller may open a shell: `terminal.manage` on the environment, which whoever manages
/// it holds without being named. A thread it is opened beside gives nothing. The environment's own
/// switch and, on a person's machine, its local policy come beside this, and are not the page's.
pub async fn may_open_shell(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    policy: Policy,
) -> Result<Decision, Error> {
    let (object, permission) = door_of(&Shell {
        environment,
        thread: None,
        opened_by: principal.user(),
    });
    resources::check(conn, principal, object, permission, policy).await
}

/// Whether a person may attach to a shell: its opener alone, whatever anybody else holds. Others
/// see that it exists.
#[must_use]
pub fn may_attach(shell: &Shell, person: Id) -> bool {
    shell.opened_by == person
}

/// Whether a person may close a shell: its opener, and whoever manages the environment.
pub async fn may_close_shell(
    conn: &mut Conn,
    principal: &Principal,
    shell: &Shell,
    policy: Policy,
) -> Result<bool, Error> {
    if shell.opened_by == principal.user() {
        return Ok(true);
    }
    holds_door(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: shell.environment,
        },
        Permission::EnvironmentManage,
        policy,
    )
    .await
}

/// Whether a shell may stay open: what it was opened by still holds for its opener, `principal`.
/// A shell is closed when it stops, attached or not; this is asked of each open shell when the
/// authorization epoch changes.
pub async fn stays_open(
    conn: &mut Conn,
    principal: &Principal,
    shell: &Shell,
    policy: Policy,
) -> Result<bool, Error> {
    if principal.user() != shell.opened_by {
        return Ok(false);
    }
    let (object, permission) = door_of(shell);
    holds_door(conn, principal, object, permission, policy).await
}

/// What a read of what a closed shell printed comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// Whether the reader may read it.
    pub allowed: bool,
    /// Whether the read is audited: it is by anybody but the opener.
    pub audited: bool,
}

/// Who reads what a closed shell last printed: its opener; whoever manages the environment;
/// whoever holds `audit.view` on the thread it was opened from. A read by anybody but its opener is
/// audited.
pub async fn may_read_recording(
    conn: &mut Conn,
    principal: &Principal,
    shell: &Shell,
    policy: Policy,
) -> Result<Reading, Error> {
    if shell.opened_by == principal.user() {
        return Ok(Reading {
            allowed: true,
            audited: false,
        });
    }
    let manages = holds_door(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: shell.environment,
        },
        Permission::EnvironmentManage,
        policy,
    )
    .await?;
    let audits = match shell.thread {
        Some(thread) if !manages => {
            holds_door(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: thread,
                },
                Permission::AuditView,
                policy,
            )
            .await?
        }
        _ => false,
    };
    Ok(Reading {
        allowed: manages || audits,
        audited: manages || audits,
    })
}

/// The role a new environment's page starts with for the people of its home: as shipped the
/// Developers where it is managed and nobody on a person's machine. The page says so; this is only
/// the name of the default for the dialog that makes one.
#[must_use]
pub fn shipped_manager(placement: orch_access_contract::permissions::Placement) -> Option<Role> {
    Permission::EnvironmentManage.shipped(placement)
}
