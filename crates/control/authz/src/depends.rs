//! What a permission is no use without on another object, whether the people concerned have it, and giving it or asking for it in the same step.
//!
//! Talking to an agent needs the model; starting work needs something to run on and somewhere to
//! run. Those stay separate permissions on separate pages, and nobody should have to visit four
//! pages to make one thing work: the page that gives the first shows the second beside it, says
//! whether the people concerned have it, and offers to give it in the same step where the person
//! at the page may. [`of_thread`] does that for `thread.interact` and the provider its model is
//! run on, [`of_place`] for the Interact of a topic, a project or an org and what work there starts
//! from.
//!
//! The device behind it is the holder that names the work in a place: a resource opened to it
//! once, instead of to its people one by one. Each offer is a change of the other object's page,
//! made by the caller with their own `access.manage` there; where they lack it, the same step is a
//! request to whoever holds it, and nothing is given that its owner did not give.

use orch_access_contract::decisions::{MissingAccess, MissingOptions, ObjectRef};
use orch_access_contract::pages::PageKind;
use orch_access_contract::pages::{Entry, Holder, PageChange, RowChange};
use orch_access_contract::permissions::{NodePermission, Permission, Profile, ProviderUse};
use orch_access_contract::roles::{NodeKind, Role};
use orch_db::Conn;
use orch_types::Id;

use crate::engine::{Frame, Graph, Site};
use crate::giving::{Acting, Giving};
use crate::nodes::every_permission;
use crate::resources::{self, Resource, read_into, site_of};
use crate::usable::Policies;
use crate::{Error, Principal, missing, options, usable};

/// How far the holders of a permission also hold what it depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Open {
    /// All of them: through an entry for the work in a place, or each by themself.
    All,
    /// Some of them.
    Part,
    /// Nobody but its owners.
    Nobody,
}

/// Something on another object that a permission here is of no use without.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    /// The permission here, such as `thread.interact`.
    pub for_permission: Permission,
    /// The object it depends on.
    pub object: ObjectRef,
    /// That object's name.
    pub name: String,
    /// That object's first owner.
    pub owner: Option<Id>,
    /// The permission there, such as `provider.use`.
    pub permission: Permission,
    /// For a provider, the model and effort in use.
    pub options: Option<MissingOptions>,
    /// Whether the holders of the permission here have what it needs there.
    pub open: Open,
    /// The place whose entry opens it, when one does.
    pub through: Option<Id>,
    /// Whether the caller can give it: they manage that object's page.
    pub can_give: bool,
}

/// Whether an entry lets its holder run the model, which the provider's managers put in `groups`, at the
/// effort. An entry that is not in a Use row has no options and lets everything.
fn lets_run(
    entry: &Entry,
    permission: Permission,
    model: Option<&str>,
    groups: &[String],
    effort: Option<&str>,
) -> bool {
    if permission != Permission::ProviderUse {
        return true;
    }
    let granted: ProviderUse = options::read(permission, entry)
        .map(|read| options::of_provider(&read))
        .unwrap_or_default();
    model.is_none_or(|model| options::allows_model(&granted, model, groups))
        && effort.is_none_or(|effort| granted.allows_effort(effort))
}

/// Whether the holder `entry` covers every person the holder `held` stands for: the same holder,
/// everyone, or a role held in the same place that the held role includes. Under-approximate:
/// holders that cannot be compared without finding their people are not said to cover.
pub(crate) fn covers(entry: &Holder, held: &Holder) -> bool {
    match (entry, held) {
        (Holder::Everyone, _) => true,
        (
            Holder::Role {
                role: entry_role,
                at: entry_at,
            },
            Holder::Role { role, at },
        ) => entry_at == at && role.includes(*entry_role),
        (one, other) => one == other,
    }
}

/// Resolves a holder that names a role where the thing is to the place it is held at.
pub(crate) fn resolved(site: &Site, holder: Holder) -> Holder {
    match holder {
        Holder::Role { role, at: None } => Holder::Role {
            role,
            at: site.role_place,
        },
        other => other,
    }
}

/// A permission that is no use without another one on a resource: who holds the first, and what
/// the second is for.
struct Pairing<'a> {
    /// What holds the first permission: the thread, or the place that starts work.
    holder_site: &'a Site,
    /// The permission that is no use without the other.
    holder_permission: Permission,
    /// The resource.
    resource_site: &'a Site,
    /// The permission needed on the resource.
    permission: Permission,
    /// The path of the work that is to run on the resource, if it is a thread or a place.
    work: &'a [Id],
    /// The model it is to run.
    model: Option<&'a str>,
    /// The groups the resource's managers put that model in.
    groups: &'a [String],
    /// The effort it is to run at.
    effort: Option<&'a str>,
}

/// How far the holders of `holder_permission` on `holder_site` (and the levels above it, and its
/// owners) also have `permission` on the resource `resource_site` for the model and effort: and the
/// place whose entry opens it to all of them at once, when one does.
fn open_for(frame: &Frame<'_>, pairing: &Pairing<'_>) -> (Open, Option<Id>) {
    let Pairing {
        holder_site,
        holder_permission,
        resource_site,
        permission,
        work,
        model,
        groups,
        effort,
    } = *pairing;
    let now = frame.now();
    let mut entries: Vec<(Holder, bool)> = Vec::new();
    let mut rows = vec![permission];
    rows.extend(permission.held_through());
    for row in rows {
        for found in frame.chain(resource_site, row).holders(now) {
            entries.push((
                resolved(resource_site, found.entry.holder),
                lets_run(found.entry, row, model, groups, effort),
            ));
        }
    }
    // The resource's owners use it, and are covered by being who they are.
    for owner in frame.owner_entries(resource_site.id) {
        if owner.counts(now) {
            entries.push((owner.holder, true));
        }
    }
    // The work in a place that the work lies in opens it to everyone who gives that work input.
    for found in frame.chain(resource_site, permission).holders(now) {
        if found.entry.holder.is_work_on(work)
            && lets_run(found.entry, permission, model, groups, effort)
            && let Holder::Node { id } = found.entry.holder
        {
            return (Open::All, Some(id));
        }
    }
    let held = held_units(frame, holder_site, holder_permission);
    if held.is_empty() {
        return (Open::Nobody, None);
    }
    let covered = held
        .iter()
        .filter(|unit| {
            unit.iter().any(|holder| {
                entries
                    .iter()
                    .any(|(entry, lets)| *lets && covers(entry, holder))
            })
        })
        .count();
    let open = match covered {
        0 => Open::Nobody,
        count if count == held.len() => Open::All,
        _ => Open::Part,
    };
    (open, None)
}

/// Each unit is somebody (or some set of people) who holds the permission on the thing `holder_site`,
/// in the forms in which an entry elsewhere could cover them: those its row and the rows above it
/// name, and its owners, who hold everything on it; the owners of a place are Admins of it as well.
pub(crate) fn held_units(
    frame: &Frame<'_>,
    holder_site: &Site,
    holder_permission: Permission,
) -> Vec<Vec<Holder>> {
    let now = frame.now();
    let mut held: Vec<Vec<Holder>> = Vec::new();
    let mut rows = vec![holder_permission];
    rows.extend(holder_permission.held_through());
    for row in rows {
        for found in frame.chain(holder_site, row).holders(now) {
            let holder = resolved(holder_site, found.entry.holder);
            if matches!(
                holder,
                Holder::User { .. }
                    | Holder::Group { .. }
                    | Holder::Role { .. }
                    | Holder::Members { .. }
                    | Holder::Everyone
            ) && !held.iter().any(|unit| unit.contains(&holder))
            {
                held.push(vec![holder]);
            }
        }
    }
    // Whoever owns the thing that is talked to talks to it, and needs the same.
    let is_place = holder_site.kind.node().and_then(NodeKind::place).is_some();
    for owner in frame.owner_entries(holder_site.id) {
        if owner.counts(now) && !held.iter().any(|unit| unit.contains(&owner.holder)) {
            let mut forms = vec![owner.holder];
            if is_place {
                forms.push(Holder::Role {
                    role: Role::Admin,
                    at: Some(holder_site.id),
                });
            }
            held.push(forms);
        }
    }
    held
}

fn pick(found: &[Resource], id: Id) -> Result<Resource, Error> {
    found
        .iter()
        .find(|resource| resource.id == id)
        .cloned()
        .ok_or(Error::NotFound(id))
}

/// What `thread.interact` on the thread `thread` is no use without: the use of the provider it runs
/// on, with the model and effort it has, and whether that is open to everyone who can talk to the
/// thread, to some of them, or to nobody else; and whether the caller manages the provider's page,
/// which is what lets them open it in the same step.
pub async fn of_thread(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    provider: Id,
    model: &str,
    effort: Option<&str>,
    policies: &Policies,
) -> Result<Vec<Dependency>, Error> {
    let mut graph = Graph::load(conn, &[thread]).await?;
    let thread_facts = graph.get(thread).cloned().ok_or(Error::NotFound(thread))?;
    let thread_site = graph.site_of(thread).ok_or(Error::NotFound(thread))?;
    let found = read_into(conn, &mut graph, &[provider]).await?;
    let resource = pick(&found, provider)?;
    let site = site_of(&graph, &resource).ok_or(Error::NotFound(provider))?;
    let policy = policies.sharing_at(resource.home);
    let wanted = [
        Permission::ThreadInteract,
        Permission::ProviderUse,
        Permission::AccessManage,
    ];
    let groups = orch_db::queries::resources::groups_of(conn, &[(provider, model.to_owned())])
        .await?
        .remove(&(provider, model.to_owned()))
        .unwrap_or_default();
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        &[thread_site.clone(), site.clone()],
        &wanted,
    )
    .await?;
    let (open, through) = open_for(
        &frame,
        &Pairing {
            holder_site: &thread_site,
            holder_permission: Permission::ThreadInteract,
            resource_site: &site,
            permission: Permission::ProviderUse,
            work: &thread_facts.path,
            model: Some(model),
            groups: &groups,
            effort,
        },
    );
    Ok(vec![Dependency {
        for_permission: Permission::ThreadInteract,
        object: resource.object(),
        name: resource.name.clone(),
        owner: resource.first_owner,
        permission: Permission::ProviderUse,
        options: Some(MissingOptions {
            models: Some(vec![model.to_owned()]),
            efforts: effort.map(|effort| vec![effort.to_owned()]),
        }),
        open,
        through,
        can_give: can_give(
            &frame,
            &resource,
            &site,
            Permission::ProviderUse,
            Some(model),
            &groups,
            effort,
            policies.at(resource.home).keeps_to_its_owner(&resource),
        ),
    }])
}

/// What the settings of a place name for work to start from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Starting {
    /// The provider.
    pub provider: Option<Id>,
    /// The agent profile.
    pub profile: Option<Id>,
    /// The host a new environment is made on.
    pub host: Option<Id>,
    /// The template it is made from.
    pub template: Option<Id>,
    /// The default environment, which whoever starts a thread there has to manage.
    pub environment: Option<Id>,
}

/// What the ordinary giving and resource-use resolver both allow for this exact model and effort.
#[allow(
    clippy::too_many_arguments,
    reason = "the paired permission, its selected model and effort, and the resolved subscription policy"
)]
fn can_give(
    frame: &Frame<'_>,
    resource: &Resource,
    site: &Site,
    permission: Permission,
    model: Option<&str>,
    groups: &[String],
    effort: Option<&str>,
    kept: bool,
) -> bool {
    if !Giving::of(frame, Some(site), PageKind::Live, Acting::Ordinary)
        .can_give(permission, Profile::Any)
        || kept
    {
        return false;
    }
    if permission != Permission::ProviderUse {
        return true;
    }
    let Ok(Ok(grant)) = usable::access(frame, resource, site, None, false) else {
        return false;
    };
    let options = grant.provider();
    model.map_or(options.models.is_none(), |model| {
        options::allows_model(&options, model, groups)
    }) && effort.map_or(options.efforts.is_none(), |effort| {
        options.allows_effort(effort)
    })
}

/// Resolves place dependencies with the selected default model and effort, as work would start there.
pub async fn of_place_with_model(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    starting: &Starting,
    model: Option<&str>,
    effort: Option<&str>,
    policies: &Policies,
) -> Result<Vec<Dependency>, Error> {
    of_place_resolved(conn, principal, place, starting, model, effort, policies).await
}

/// What Interact on a topic, a project or an org, which starts work there, is no use without:
/// something to run on and somewhere to run. For each thing the settings of the place name, whether
/// it is open to everyone who holds Interact there, and whether it is open to the work in the place;
/// for the default environment, who manages it.
pub async fn of_place(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    starting: &Starting,
    policies: &Policies,
) -> Result<Vec<Dependency>, Error> {
    of_place_resolved(conn, principal, place, starting, None, None, policies).await
}

async fn of_place_resolved(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    starting: &Starting,
    model: Option<&str>,
    effort: Option<&str>,
    policies: &Policies,
) -> Result<Vec<Dependency>, Error> {
    let mut graph = Graph::load(conn, &[place]).await?;
    let facts = graph.get(place).cloned().ok_or(Error::NotFound(place))?;
    let place_site = graph.site_of(place).ok_or(Error::NotFound(place))?;
    let interact = NodePermission::Interact
        .on(facts.kind)
        .ok_or(Error::NotFound(place))?;
    let named: Vec<(Id, Permission)> = [
        (starting.provider, Permission::ProviderUse),
        (starting.profile, Permission::AgentUse),
        (starting.host, Permission::HostUse),
        (starting.template, Permission::TemplateUse),
        (starting.environment, Permission::EnvironmentManage),
    ]
    .into_iter()
    .filter_map(|(id, permission)| id.map(|id| (id, permission)))
    .collect();
    let ids: Vec<Id> = named.iter().map(|(id, _)| *id).collect();
    let found = read_into(conn, &mut graph, &ids).await?;
    let model_groups = if let (Some(provider), Some(model)) = (starting.provider, model) {
        orch_db::queries::resources::groups_of(conn, &[(provider, model.to_owned())])
            .await?
            .remove(&(provider, model.to_owned()))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut sites: Vec<(Resource, Site, Permission)> = Vec::new();
    for (id, permission) in named {
        let resource = pick(&found, id)?;
        let site = site_of(&graph, &resource).ok_or(Error::NotFound(id))?;
        sites.push((resource, site, permission));
    }
    let mut all: Vec<Site> = vec![place_site.clone()];
    all.extend(sites.iter().map(|(_, site, _)| site.clone()));
    let mut wanted = vec![interact, Permission::AccessManage];
    wanted.extend(sites.iter().map(|(_, _, permission)| *permission));
    // A single frame serves every dependency with the same resolved policy, even when its homes differ.
    let mut frames = std::collections::HashMap::new();
    let mut dependencies = Vec::new();
    for (resource, site, permission) in &sites {
        let policy = policies.sharing_at(resource.home);
        if let std::collections::hash_map::Entry::Vacant(e) = frames.entry(policy) {
            e.insert(Frame::load(conn, principal, policy, graph.clone(), &all, &wanted).await?);
        }
        if let Some(frame) = frames.get(&policy) {
            let (open, through) = open_for(
                frame,
                &Pairing {
                    holder_site: &place_site,
                    holder_permission: interact,
                    resource_site: site,
                    permission: *permission,
                    work: &facts.path,
                    model: (*permission == Permission::ProviderUse)
                        .then_some(model)
                        .flatten(),
                    groups: &model_groups,
                    effort: (*permission == Permission::ProviderUse)
                        .then_some(effort)
                        .flatten(),
                },
            );
            dependencies.push(Dependency {
                for_permission: interact,
                object: resource.object(),
                name: resource.name.clone(),
                owner: resource.first_owner,
                permission: *permission,
                options: (*permission == Permission::ProviderUse && model.is_some()).then(|| {
                    MissingOptions {
                        models: model.map(|model| vec![model.to_owned()]),
                        efforts: effort.map(|effort| vec![effort.to_owned()]),
                    }
                }),
                open,
                through,
                can_give: can_give(
                    frame,
                    resource,
                    site,
                    *permission,
                    model,
                    &model_groups,
                    effort,
                    policies.at(resource.home).keeps_to_its_owner(resource),
                ),
            });
        }
    }
    Ok(dependencies)
}

/// The profile a subscription's Use holders also need, as that profile's page currently gives it.
pub async fn of_provider(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    profile: Id,
    policies: &Policies,
) -> Result<Vec<Dependency>, Error> {
    let mut graph = Graph::default();
    let found = read_into(conn, &mut graph, &[provider, profile]).await?;
    let provider = pick(&found, provider)?;
    let profile = pick(&found, profile)?;
    let holder_site = site_of(&graph, &provider).ok_or(Error::NotFound(provider.id))?;
    let site = site_of(&graph, &profile).ok_or(Error::NotFound(profile.id))?;
    let frame = Frame::load(
        conn,
        principal,
        policies.sharing_at(profile.home),
        graph,
        &[holder_site.clone(), site.clone()],
        &[
            Permission::ProviderUse,
            Permission::AgentUse,
            Permission::AccessManage,
        ],
    )
    .await?;
    let (open, through) = open_for(
        &frame,
        &Pairing {
            holder_site: &holder_site,
            holder_permission: Permission::ProviderUse,
            resource_site: &site,
            permission: Permission::AgentUse,
            work: &[],
            model: None,
            groups: &[],
            effort: None,
        },
    );
    Ok(vec![Dependency {
        for_permission: Permission::ProviderUse,
        object: profile.object(),
        name: profile.name.clone(),
        owner: profile.first_owner,
        permission: Permission::AgentUse,
        options: None,
        open,
        through,
        can_give: can_give(
            &frame,
            &profile,
            &site,
            Permission::AgentUse,
            None,
            &[],
            None,
            false,
        ),
    }])
}

/// What the page that shows a dependency offers to do about it in the same step.
#[derive(Debug, Clone, PartialEq)]
pub enum Offer {
    /// The caller manages the other object's page: this change of it opens it to the work in the
    /// place, and the caller makes it with their own `access.manage` there.
    Give {
        /// The other object.
        object: ObjectRef,
        /// The change of its page.
        change: Box<PageChange>,
    },
    /// The caller does not: the same entry is asked of whoever can give it.
    Ask {
        /// What is asked, of whom, with the options: the item of an access request.
        item: Box<MissingAccess>,
        /// Whom the entry would name: the work in the place.
        holder: Holder,
    },
}

/// The offer for a dependency that is not open to all: an entry in the Use row of the other object
/// whose holder is the work in `place` (the thread itself, or a place above it), limited to the
/// model and effort in use, given by the caller where they manage that page, asked for otherwise.
pub async fn offer(
    conn: &mut Conn,
    principal: &Principal,
    dependency: &Dependency,
    place: Id,
    policies: &Policies,
) -> Result<Offer, Error> {
    let holder = Holder::Node { id: place };
    if dependency.can_give {
        let row = RowChange {
            permission: dependency.permission.as_str().to_owned(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![orch_access_contract::pages::EntryAdd {
                holder,
                options: dependency.options.as_ref().map(|wanted| {
                    serde_json::json!({ "models": wanted.models, "efforts": wanted.efforts })
                }),
                expires_at: None,
                note: None,
            }]),
            update: None,
            remove: None,
        };
        return Ok(Offer::Give {
            object: dependency.object,
            change: Box::new(PageChange {
                rows: Some(vec![row]),
                ..PageChange::default()
            }),
        });
    }
    let (graph, resource, site) = resources::site(conn, dependency.object).await?;
    let everything = every_permission([dependency.object.kind]);
    let policy = policies.at(resource.home).node_policy();
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    let mut items =
        missing::missing_access(conn, &frame, &site, &[dependency.permission], true).await?;
    let mut item = items.pop().ok_or(Error::NotFound(dependency.object.id))?;
    item.options.clone_from(&dependency.options);
    Ok(Offer::Ask {
        item: Box::new(item),
        holder,
    })
}
